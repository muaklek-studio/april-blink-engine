//! Scripting conveniences on [`SceneTree`]: typed access to other scripts and components,
//! groups, timers, and swapping scripts on hot reload.

use crate::class;
use crate::components::NodeInfo;
use crate::handle::NodeHandle;
use crate::property::ScriptPropertyDesc;
use crate::registry::ScriptRegistry;
use crate::scene_tree::SceneTree;
use crate::script::{NodeScript, ScriptSlot};
use crate::signal::Signal;
use crate::SceneError;
use ab_ecs::{Component, Ref, RefMut};
use tracing::{info, warn};

/// Groups a node belongs to (Godot's `add_to_group`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Groups(pub Vec<String>);

/// Counts down and emits [`Timer::TIMEOUT`] (Godot's `Timer` node). Timers advance with the
/// frame delta at the start of `process`, only while their node is in the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Timer {
    /// Seconds from start to timeout.
    pub wait_time: f32,
    /// Stop after the first timeout instead of restarting.
    pub one_shot: bool,
    /// Start as soon as the node is in the tree.
    pub autostart: bool,
    pub paused: bool,
    time_left: f32,
    running: bool,
    autostarted: bool,
    /// Free the node after a one-shot timeout ([`SceneTree::create_timer`]).
    free_on_timeout: bool,
}

impl Timer {
    /// Signal emitted when the time is up.
    pub const TIMEOUT: &'static str = "timeout";

    pub fn new(wait_time: f32) -> Self {
        Self {
            wait_time,
            one_shot: false,
            autostart: false,
            paused: false,
            time_left: 0.0,
            running: false,
            autostarted: false,
            free_on_timeout: false,
        }
    }

    pub fn one_shot(mut self) -> Self {
        self.one_shot = true;
        self
    }

    pub fn autostart(mut self) -> Self {
        self.autostart = true;
        self
    }

    /// (Re)starts the countdown from `wait_time`.
    pub fn start(&mut self) {
        self.time_left = self.wait_time;
        self.running = true;
        // Started or stopped by hand (say in `ready`): autostart has nothing left to do.
        self.autostarted = true;
    }

    pub fn stop(&mut self) {
        self.running = false;
        self.autostarted = true;
        self.time_left = 0.0;
    }

    pub fn is_stopped(&self) -> bool {
        !self.running
    }

    pub fn time_left(&self) -> f32 {
        self.time_left
    }
}

impl SceneTree {
    // =========================================================================================
    // Typed access
    // =========================================================================================

    /// The node's first script that is a `T` (Unity's `GetComponent<T>()`, Godot's
    /// `GetNode<Player>()`). `None` while that script is running (e.g. asking for your own
    /// script from inside a callback); the node's other scripts are reachable meanwhile.
    pub fn script<T: NodeScript>(&self, node: NodeHandle) -> Option<Ref<'_, T>> {
        let slot = self.world.get::<ScriptSlot>(node.entity())?;
        Ref::filter_map(slot, |s| {
            s.entries
                .iter()
                .find_map(|e| e.script.as_deref()?.script_any().downcast_ref::<T>())
        })
        .ok()
    }

    pub fn script_mut<T: NodeScript>(&self, node: NodeHandle) -> Option<RefMut<'_, T>> {
        let slot = self.world.get_mut::<ScriptSlot>(node.entity())?;
        RefMut::filter_map(slot, |s| {
            s.entries.iter_mut().find_map(|e| {
                e.script
                    .as_deref_mut()?
                    .script_any_mut()
                    .downcast_mut::<T>()
            })
        })
        .ok()
    }

    /// Calls `f` with the node's `T` script and the whole tree (for methods that change the
    /// scene). The script is detached meanwhile, as during its own callbacks.
    pub fn call_script<T: NodeScript, R>(
        &mut self,
        node: NodeHandle,
        f: impl FnOnce(&mut T, &mut SceneTree) -> R,
    ) -> Option<R> {
        // Found first so a wrong type never detaches a script.
        let id = self
            .world
            .get::<ScriptSlot>(node.entity())?
            .entries
            .iter()
            .find_map(|e| e.script.as_deref()?.script_any().is::<T>().then_some(e.id))?;
        self.with_script_id(node, id, |script, tree| {
            script
                .script_any_mut()
                .downcast_mut::<T>()
                .map(|script| f(script, tree))
        })
        .flatten()
    }

    /// The instance of a script-library script attached to `node` whose script type is
    /// `script_type` (an opaque pointer of the library), or `None` (also while it runs). The
    /// pointer stays valid while the script stays attached and is not running.
    pub fn script_instance(&mut self, node: NodeHandle, script_type: *const ()) -> Option<*mut ()> {
        let mut slot = self.world.get_mut::<ScriptSlot>(node.entity())?;
        slot.entries.iter_mut().find_map(|e| {
            let (ty, instance) = e.script.as_deref_mut()?.foreign_instance()?;
            std::ptr::eq(ty, script_type).then_some(instance)
        })
    }

    /// Whether one of the node's scripts panicked and was disabled (see [`NodeScript`]).
    pub fn has_failed_script(&self, node: NodeHandle) -> bool {
        self.world
            .get::<ScriptSlot>(node.entity())
            .is_some_and(|slot| {
                slot.entries
                    .iter()
                    .any(|e| e.script.as_deref().is_some_and(|s| s.has_failed()))
            })
    }

    /// A component of the node at `path` relative to `from`.
    pub fn get_component<T: Component>(&self, from: NodeHandle, path: &str) -> Option<Ref<'_, T>> {
        self.component::<T>(self.get_node(from, path)?)
    }

    pub fn get_component_mut<T: Component>(
        &self,
        from: NodeHandle,
        path: &str,
    ) -> Option<RefMut<'_, T>> {
        self.component_mut::<T>(self.get_node(from, path)?)
    }

    // =========================================================================================
    // Groups
    // =========================================================================================

    pub fn add_to_group(&mut self, node: NodeHandle, group: &str) -> Result<(), SceneError> {
        if !self.is_valid(node) {
            return Err(SceneError::InvalidNode(node));
        }
        if let Some(mut groups) = self.world.get_mut::<Groups>(node.entity()) {
            if !groups.0.iter().any(|g| g == group) {
                groups.0.push(group.to_owned());
            }
            return Ok(());
        }
        self.insert_component(node, Groups(vec![group.to_owned()]))
    }

    pub fn remove_from_group(&mut self, node: NodeHandle, group: &str) -> bool {
        self.world
            .get_mut::<Groups>(node.entity())
            .is_some_and(|mut groups| {
                let before = groups.0.len();
                groups.0.retain(|g| g != group);
                groups.0.len() != before
            })
    }

    pub fn is_in_group(&self, node: NodeHandle, group: &str) -> bool {
        self.world
            .get::<Groups>(node.entity())
            .is_some_and(|groups| groups.0.iter().any(|g| g == group))
    }

    /// Nodes in `group` that are inside the tree, in tree order.
    pub fn nodes_in_group(&self, group: &str) -> Vec<NodeHandle> {
        let Some(groups) = self.world.storage::<Groups>() else {
            return Vec::new();
        };
        if !groups
            .values()
            .iter()
            .any(|g| g.0.iter().any(|n| n == group))
        {
            return Vec::new();
        }
        self.subtree(self.root())
            .into_iter()
            .filter(|n| {
                groups
                    .get(n.entity())
                    .is_some_and(|g| g.0.iter().any(|name| name == group))
            })
            .collect()
    }

    // =========================================================================================
    // Timers
    // =========================================================================================

    /// Creates a detached `Timer` node.
    pub fn create_timer_node(&mut self, name: &str, timer: Timer) -> NodeHandle {
        self.spawn_node(name, class::TIMER, (timer,))
    }

    /// A one-shot timer under the root that emits [`Timer::TIMEOUT`] after `seconds` and then
    /// frees itself (Godot's `get_tree().create_timer()`). Connect to the returned node.
    pub fn create_timer(&mut self, seconds: f32) -> NodeHandle {
        let mut timer = Timer::new(seconds).one_shot().autostart();
        timer.free_on_timeout = true;
        let node = self.create_timer_node("Timer", timer);
        self.add_child(self.root(), node)
            .expect("the root accepts a fresh node");
        node
    }

    /// Advances every timer in the tree by `delta` and emits their timeouts. Called by
    /// [`SceneTree::process`].
    pub(crate) fn update_timers(&mut self, delta: f32) {
        if !delta.is_finite() {
            return;
        }
        let mut fired = Vec::new();
        let mut to_free = Vec::new();
        {
            let (Some(mut timers), Some(infos)) = (
                self.world.storage_mut::<Timer>(),
                self.world.storage::<NodeInfo>(),
            ) else {
                return;
            };
            for (entity, timer) in timers.iter_mut() {
                if !infos.get(entity).is_some_and(|i| i.in_tree) {
                    continue;
                }
                if timer.autostart && !timer.autostarted {
                    timer.autostarted = true;
                    timer.start();
                }
                if !timer.running || timer.paused {
                    continue;
                }
                timer.time_left -= delta;
                // Tolerate float drift from summing frame deltas.
                if timer.time_left > 1e-5 {
                    continue;
                }
                let node = NodeHandle::from_entity(entity);
                fired.push(node);
                // A NaN wait would otherwise fire every frame.
                if timer.one_shot || timer.wait_time.is_nan() || timer.wait_time <= 0.0 {
                    timer.stop();
                    if timer.free_on_timeout {
                        to_free.push(node);
                    }
                } else {
                    // Keep the overshoot so repeating timers do not drift; at most one
                    // timeout per frame.
                    timer.time_left = (timer.time_left + timer.wait_time).max(0.0);
                }
            }
        }
        for node in fired {
            self.emit(Signal::new(node, Timer::TIMEOUT));
        }
        for node in to_free {
            let _ = self.queue_free(node);
        }
    }

    // =========================================================================================
    // Exported properties
    // =========================================================================================

    /// The exported property values set on the node's script `script` (by registry name),
    /// as scene files store them. Empty when none are set or there is no such script.
    pub fn script_properties(&self, node: NodeHandle, script: &str) -> toml::Table {
        self.world
            .get::<ScriptSlot>(node.entity())
            .and_then(|slot| {
                slot.entries
                    .iter()
                    .find(|e| e.name.as_deref() == Some(script))
                    .map(|e| e.properties.clone())
            })
            .unwrap_or_default()
    }

    /// Sets exported property values on the node's script `script` (by registry name),
    /// replacing those set before, and hands them to the running instance as the kinds its
    /// [`ScriptRegistry`] description declares. The values are kept (to be saved, and set
    /// again after a hot reload) even when the script does not know them. `false` if the
    /// node has no such script.
    pub fn set_script_properties(
        &mut self,
        node: NodeHandle,
        script: &str,
        properties: toml::Table,
    ) -> bool {
        let registry = self.world.resource::<ScriptRegistry>().map(|r| r.clone());
        let Some(mut slot) = self.world.get_mut::<ScriptSlot>(node.entity()) else {
            return false;
        };
        let Some(entry) = slot
            .entries
            .iter_mut()
            .find(|e| e.name.as_deref() == Some(script))
        else {
            return false;
        };
        if let Some(instance) = entry.script.as_mut() {
            let metadata = registry.as_ref().and_then(|r| r.metadata(script));
            apply_properties(instance.as_mut(), metadata, script, &properties);
        }
        entry.properties = properties;
        true
    }

    // =========================================================================================
    // Hot reload
    // =========================================================================================

    /// Replaces every script attached by name with a fresh instance from the
    /// [`ScriptRegistry`] resource, carrying state over with
    /// [`NodeScript::save_state`] / [`NodeScript::load_state`], then calls
    /// [`NodeScript::reloaded`] on it. Returns how many scripts were replaced.
    ///
    /// A script running right now (the one that asked for the reload) cannot be swapped
    /// under its own feet: it is replaced by [`SceneTree::reload_pending_scripts`] once it
    /// returned (the engine calls that every frame).
    pub fn reload_scripts(&mut self) -> usize {
        let named: Vec<(NodeHandle, u64, String)> = self
            .world
            .storage::<ScriptSlot>()
            .map(|slots| {
                slots
                    .iter()
                    .flat_map(|(entity, slot)| {
                        slot.entries.iter().filter_map(move |e| {
                            Some((NodeHandle::from_entity(entity), e.id, e.name.clone()?))
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.replace_scripts(named)
    }

    /// Replaces the scripts [`SceneTree::reload_scripts`] skipped because they were running,
    /// if they are not running anymore. Returns how many were replaced.
    pub fn reload_pending_scripts(&mut self) -> usize {
        let Some(pending) = self
            .world
            .resource_mut::<PendingReloads>()
            .map(|mut p| std::mem::take(&mut p.0))
        else {
            return 0;
        };
        if pending.is_empty() {
            return 0;
        }
        self.replace_scripts(pending)
    }

    fn replace_scripts(&mut self, named: Vec<(NodeHandle, u64, String)>) -> usize {
        let Some(registry) = self.world.resource::<ScriptRegistry>().map(|r| r.clone()) else {
            return 0;
        };
        let mut replaced = Vec::new();
        let mut running = Vec::new();
        for (node, id, name) in named {
            let Some(mut fresh) = registry.create(&name) else {
                continue;
            };
            let Some(mut slot) = self.world.get_mut::<ScriptSlot>(node.entity()) else {
                continue;
            };
            let Some(entry) = slot.entries.iter().find(|e| e.id == id) else {
                continue; // removed meanwhile
            };
            if entry.name.as_deref() != Some(name.as_str()) {
                continue;
            }
            let Some(old) = slot.take(id) else {
                // Running right now: swapped once it returned.
                running.push((node, id, name));
                continue;
            };
            // The scene's property values first, so state carried over wins.
            let properties = slot
                .entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.properties.clone())
                .unwrap_or_default();
            apply_properties(fresh.as_mut(), registry.metadata(&name), &name, &properties);
            match old.save_state() {
                Some(state) => fresh.load_state(&state),
                None => warn!(
                    script = name,
                    "hot reload: no saved state; script starts fresh"
                ),
            }
            drop(old);
            slot.restore(id, fresh);
            drop(slot);
            replaced.push((node, id));
        }
        if !running.is_empty() {
            let pending = self.world.resource_or_insert_with(PendingReloads::default);
            for item in running {
                if !pending.0.contains(&item) {
                    pending.0.push(item);
                }
            }
        }
        for &(node, id) in &replaced {
            self.with_script_id(node, id, |script, tree| script.reloaded(tree, node));
        }
        if !replaced.is_empty() {
            info!(scripts = replaced.len(), "hot reload: scripts replaced");
        }
        replaced.len()
    }
}

/// Scripts a hot reload could not swap because they were running: (node, script id, name).
#[derive(Debug, Default)]
struct PendingReloads(Vec<(NodeHandle, u64, String)>);

/// Hands `properties` to `script`, converted to the kinds `metadata` declares.
fn apply_properties(
    script: &mut dyn NodeScript,
    metadata: Option<&[ScriptPropertyDesc]>,
    script_name: &str,
    properties: &toml::Table,
) {
    for (name, value) in properties {
        let kind = match metadata {
            Some(props) => match props.iter().find(|p| &p.name == name) {
                Some(p) => Some(p.kind),
                None => {
                    warn!(
                        script = script_name,
                        property = name.as_str(),
                        "the script exports no such property; the value is kept but unused"
                    );
                    continue;
                }
            },
            None => None,
        };
        let Some(converted) = crate::property::to_ffi(kind, value) else {
            warn!(
                script = script_name,
                property = name.as_str(),
                "property value `{value}` does not fit the property's type; ignored"
            );
            continue;
        };
        script.set_exported_property(name, converted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static VERSION: AtomicU32 = AtomicU32::new(1);

    /// Counts its frames; asks for a hot reload from its own `process` once.
    struct Reloader {
        version: u32,
        frames: u32,
    }

    impl NodeScript for Reloader {
        fn process(&mut self, tree: &mut SceneTree, _node: NodeHandle, _delta: f32) {
            self.frames += 1;
            if self.frames == 1 && self.version == 1 {
                VERSION.store(2, Ordering::Relaxed);
                assert_eq!(
                    tree.reload_scripts(),
                    0,
                    "the running script is not swapped"
                );
            }
        }

        fn save_state(&self) -> Option<String> {
            Some(self.frames.to_string())
        }

        fn load_state(&mut self, state: &str) {
            self.frames = state.parse().unwrap();
        }
    }

    /// A reload asked for by the running script itself replaces it once it returned.
    #[test]
    fn a_script_running_during_a_reload_is_replaced_afterwards() {
        let mut tree = SceneTree::new();
        let mut registry = ScriptRegistry::new();
        registry.register_factory("Reloader", || {
            Box::new(Reloader {
                version: VERSION.load(Ordering::Relaxed),
                frames: 0,
            })
        });
        tree.world_mut().insert_resource(registry);
        let node = tree.create_node("N");
        tree.set_script_named(node, "Reloader").unwrap();
        let root = tree.root();
        tree.add_child(root, node).unwrap();
        tree.process(0.1);
        assert_eq!(tree.reload_pending_scripts(), 1);
        assert_eq!(tree.reload_pending_scripts(), 0, "replaced once");
        let ty = tree.script::<Reloader>(node).map(|r| (r.version, r.frames));
        assert_eq!(ty, Some((2, 1)), "the new version, with the old state");
    }
}
