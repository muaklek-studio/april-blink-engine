//! Scripting conveniences on [`SceneTree`]: typed access to other scripts and components,
//! groups, timers, and swapping scripts on hot reload.

use crate::class;
use crate::components::NodeInfo;
use crate::handle::NodeHandle;
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
    }

    pub fn stop(&mut self) {
        self.running = false;
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
                if timer.one_shot || timer.wait_time <= 0.0 {
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
    // Hot reload
    // =========================================================================================

    /// Replaces every script attached by name with a fresh instance from the
    /// [`ScriptRegistry`] resource, carrying state over with
    /// [`NodeScript::save_state`] / [`NodeScript::load_state`], then calls
    /// [`NodeScript::reloaded`] on it. Returns how many scripts were replaced.
    pub fn reload_scripts(&mut self) -> usize {
        let Some(registry) = self.world.resource::<ScriptRegistry>().map(|r| r.clone()) else {
            return 0;
        };
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

        let mut replaced = Vec::new();
        for (node, id, name) in named {
            let Some(mut fresh) = registry.create(&name) else {
                continue;
            };
            let Some(mut slot) = self.world.get_mut::<ScriptSlot>(node.entity()) else {
                continue;
            };
            let Some(old) = slot.take(id) else {
                continue; // running right now; leave it
            };
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
        for &(node, id) in &replaced {
            self.with_script_id(node, id, |script, tree| script.reloaded(tree, node));
        }
        if !replaced.is_empty() {
            info!(scripts = replaced.len(), "hot reload: scripts replaced");
        }
        replaced.len()
    }
}
