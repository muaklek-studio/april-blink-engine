use crate::class;
use crate::components::{Children, NodeInfo, Parent, Transform};
use crate::error::SceneError;
use crate::handle::NodeHandle;
use crate::path::{self, PathSegment};
use crate::profile::ScriptProfile;
use crate::registry::ScriptRegistry;
use crate::script::{Guarded, NodeScript, ScriptSlot};
use crate::signal::{SignalQueue, Signals};
use crate::systems::propagate_transforms;
use ab_core::{look_rotation, Affine3A, Quat, Transform3D, Vec3};
use ab_ecs::{Bundle, Component, Ref, RefMut, World};
use std::fmt;

/// Name of the root node, used by absolute paths (`/root/...`).
pub const ROOT_NAME: &str = "root";

/// Characters Godot forbids in node names; they are replaced with `_`.
const INVALID_NAME_CHARS: &[char] = &['.', ':', '@', '/', '"', '%'];

type Result<T> = std::result::Result<T, SceneError>;

#[derive(Clone, Copy)]
enum ProcessKind {
    Process,
    Physics,
}

/// The node tree: owns the ECS [`World`] and exposes the Godot-style Node API on top of it.
///
/// Nodes are created detached, then attached with [`SceneTree::add_child`]; attaching under a
/// node that is inside the tree runs `enter_tree` and `ready` immediately (as in Godot).
///
/// Per frame the engine calls [`SceneTree::physics_process`] for each fixed step, then
/// [`SceneTree::process`], which also propagates transforms and frees queued nodes.
pub struct SceneTree {
    pub(crate) world: World,
    root: NodeHandle,
    free_queue: Vec<NodeHandle>,
    /// Reused per-frame buffer of nodes to dispatch `process`/`physics_process` to.
    dispatch_buffer: Vec<NodeHandle>,
    pub(crate) signals: Signals,
}

impl SceneTree {
    pub fn new() -> Self {
        Self::with_world(World::new())
    }

    /// Builds a tree around an existing world (e.g. one already holding engine resources).
    pub fn with_world(mut world: World) -> Self {
        let mut info = NodeInfo::new(ROOT_NAME, class::NODE);
        info.in_tree = true;
        info.ready_called = true;
        let root = NodeHandle::from_entity(world.spawn((info, Children::default())));
        world.resource_or_insert_with(SignalQueue::default);
        world.resource_or_insert_with(ScriptRegistry::new);
        world.resource_or_insert_with(ScriptProfile::default);
        Self {
            world,
            root,
            free_queue: Vec::new(),
            dispatch_buffer: Vec::new(),
            signals: Signals::default(),
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn root(&self) -> NodeHandle {
        self.root
    }

    /// Number of live nodes, including the root and detached nodes.
    pub fn node_count(&self) -> usize {
        self.world.storage::<NodeInfo>().map_or(0, |s| s.len())
    }

    // =========================================================================================
    // Creation
    // =========================================================================================

    /// Creates a detached plain `Node`.
    pub fn create_node(&mut self, name: &str) -> NodeHandle {
        self.spawn_node(name, class::NODE, ())
    }

    /// Creates a detached `Node3D` with an identity transform.
    pub fn create_node3d(&mut self, name: &str) -> NodeHandle {
        self.spawn_node(name, class::NODE_3D, (Transform::default(),))
    }

    /// Creates a detached node of class `class_name` carrying the components in `bundle`.
    /// Include a [`Transform`] in the bundle to make it a `Node3D`.
    pub fn spawn_node<B: Bundle>(
        &mut self,
        name: &str,
        class_name: &'static str,
        bundle: B,
    ) -> NodeHandle {
        let name = sanitize_name(name, class_name);
        let entity = self
            .world
            .spawn((NodeInfo::new(name, class_name), Children::default()));
        self.world
            .insert_bundle(entity, bundle)
            .expect("freshly spawned entity is alive");
        NodeHandle::from_entity(entity)
    }

    /// Adds or replaces a component on a node (mesh, camera, rigid body, ...).
    pub fn insert_component<T: Component>(&mut self, node: NodeHandle, component: T) -> Result<()> {
        self.ensure_valid(node)?;
        self.world
            .insert(node.entity(), component)
            .map_err(|_| SceneError::InvalidNode(node))?;
        Ok(())
    }

    pub fn component<T: Component>(&self, node: NodeHandle) -> Option<Ref<'_, T>> {
        self.world.get::<T>(node.entity())
    }

    pub fn component_mut<T: Component>(&self, node: NodeHandle) -> Option<RefMut<'_, T>> {
        self.world.get_mut::<T>(node.entity())
    }

    /// Whether `node` refers to a live node. Stale handles to freed nodes return `false`.
    pub fn is_valid(&self, node: NodeHandle) -> bool {
        self.world.has::<NodeInfo>(node.entity())
    }

    fn ensure_valid(&self, node: NodeHandle) -> Result<()> {
        if self.is_valid(node) {
            Ok(())
        } else {
            Err(SceneError::InvalidNode(node))
        }
    }

    // =========================================================================================
    // Metadata
    // =========================================================================================

    fn info(&self, node: NodeHandle) -> Option<Ref<'_, NodeInfo>> {
        self.world.get::<NodeInfo>(node.entity())
    }

    fn info_mut(&self, node: NodeHandle) -> Result<RefMut<'_, NodeInfo>> {
        self.world
            .get_mut::<NodeInfo>(node.entity())
            .ok_or(SceneError::InvalidNode(node))
    }

    pub fn name(&self, node: NodeHandle) -> Option<Ref<'_, str>> {
        self.info(node)
            .map(|info| Ref::map(info, |i| i.name.as_str()))
    }

    /// Renames a node. Invalid characters become `_`; a name already used by a sibling gets a
    /// numeric suffix (`Enemy` → `Enemy2`).
    pub fn set_name(&mut self, node: NodeHandle, name: &str) -> Result<()> {
        let class_name = self.class_name(node).ok_or(SceneError::InvalidNode(node))?;
        if node == self.root {
            return Err(SceneError::RootNode);
        }
        let mut name = sanitize_name(name, class_name);
        if let Some(parent) = self.parent(node) {
            name = self.unique_child_name(parent, &name, Some(node));
        }
        self.info_mut(node)?.name = name;
        Ok(())
    }

    pub fn class_name(&self, node: NodeHandle) -> Option<&'static str> {
        self.info(node).map(|i| i.class_name)
    }

    pub fn set_class_name(&mut self, node: NodeHandle, class_name: &'static str) -> Result<()> {
        self.info_mut(node)?.class_name = class_name;
        Ok(())
    }

    pub fn is_inside_tree(&self, node: NodeHandle) -> bool {
        self.info(node).is_some_and(|i| i.in_tree)
    }

    /// Whether `ready` has run for this node.
    pub fn is_node_ready(&self, node: NodeHandle) -> bool {
        self.info(node).is_some_and(|i| i.ready_called)
    }

    pub fn is_queued_for_deletion(&self, node: NodeHandle) -> bool {
        self.info(node).is_some_and(|i| i.queued_for_deletion)
    }

    pub fn is_visible(&self, node: NodeHandle) -> bool {
        self.info(node).is_some_and(|i| i.visible)
    }

    pub fn set_visible(&mut self, node: NodeHandle, visible: bool) -> Result<()> {
        self.info_mut(node)?.visible = visible;
        Ok(())
    }

    /// Visible itself and every ancestor visible.
    pub fn is_visible_in_tree(&self, node: NodeHandle) -> bool {
        let mut current = Some(node);
        while let Some(n) = current {
            if !self.is_visible(n) {
                return false;
            }
            current = self.parent(n);
        }
        true
    }

    pub fn set_process(&mut self, node: NodeHandle, enabled: bool) -> Result<()> {
        self.info_mut(node)?.process_enabled = enabled;
        Ok(())
    }

    pub fn is_processing(&self, node: NodeHandle) -> bool {
        self.info(node).is_some_and(|i| i.process_enabled)
    }

    pub fn set_physics_process(&mut self, node: NodeHandle, enabled: bool) -> Result<()> {
        self.info_mut(node)?.physics_process_enabled = enabled;
        Ok(())
    }

    pub fn is_physics_processing(&self, node: NodeHandle) -> bool {
        self.info(node).is_some_and(|i| i.physics_process_enabled)
    }

    // =========================================================================================
    // Hierarchy
    // =========================================================================================

    pub fn parent(&self, node: NodeHandle) -> Option<NodeHandle> {
        self.world.get::<Parent>(node.entity()).map(|p| p.0)
    }

    /// Copy of the child list, in order.
    pub fn children(&self, node: NodeHandle) -> Vec<NodeHandle> {
        self.world
            .get::<Children>(node.entity())
            .map(|c| c.0.clone())
            .unwrap_or_default()
    }

    pub fn child_count(&self, node: NodeHandle) -> usize {
        self.world
            .get::<Children>(node.entity())
            .map_or(0, |c| c.0.len())
    }

    pub fn get_child(&self, node: NodeHandle, index: usize) -> Option<NodeHandle> {
        self.world
            .get::<Children>(node.entity())?
            .0
            .get(index)
            .copied()
    }

    /// Whether `ancestor` is a (strict) ancestor of `node`.
    pub fn is_ancestor_of(&self, ancestor: NodeHandle, node: NodeHandle) -> bool {
        let mut current = self.parent(node);
        while let Some(n) = current {
            if n == ancestor {
                return true;
            }
            current = self.parent(n);
        }
        false
    }

    /// Attaches a detached `child` as the last child of `parent`.
    ///
    /// If `parent` is inside the tree, `enter_tree` runs for the whole subtree (parents first),
    /// then `ready` for nodes that never had it (children first).
    pub fn add_child(&mut self, parent: NodeHandle, child: NodeHandle) -> Result<()> {
        self.ensure_valid(parent)?;
        self.ensure_valid(child)?;
        if child == self.root {
            return Err(SceneError::RootNode);
        }
        if let Some(existing) = self.parent(child) {
            return Err(SceneError::AlreadyHasParent {
                child,
                parent: existing,
            });
        }
        if child == parent || self.is_ancestor_of(child, parent) {
            return Err(SceneError::WouldCreateCycle { parent, child });
        }

        let name = self.info(child).map(|i| i.name.clone()).unwrap_or_default();
        let unique = self.unique_child_name(parent, &name, None);
        if unique != name {
            self.info_mut(child)?.name = unique;
        }

        self.world
            .insert(child.entity(), Parent(parent))
            .expect("child is alive");
        self.world
            .get_mut::<Children>(parent.entity())
            .expect("node has Children")
            .0
            .push(child);
        self.mark_transform_dirty(child);

        if self.is_inside_tree(parent) {
            self.propagate_enter_tree(child);
            self.propagate_ready(child);
        }
        Ok(())
    }

    /// Detaches `child` from `parent`. The node stays alive (re-add it or free it).
    /// If it was inside the tree, `exit_tree` runs for the subtree (children first).
    pub fn remove_child(&mut self, parent: NodeHandle, child: NodeHandle) -> Result<()> {
        self.ensure_valid(parent)?;
        self.ensure_valid(child)?;
        if self.parent(child) != Some(parent) {
            return Err(SceneError::NotAChild { parent, child });
        }
        if self.is_inside_tree(child) {
            self.propagate_exit_tree(child);
        }
        // A callback may already have moved the node elsewhere.
        if self.parent(child) == Some(parent) {
            self.unlink(parent, child);
        }
        Ok(())
    }

    fn unlink(&mut self, parent: NodeHandle, child: NodeHandle) {
        self.world.remove::<Parent>(child.entity());
        if let Some(mut children) = self.world.get_mut::<Children>(parent.entity()) {
            children.0.retain(|&c| c != child);
        }
        self.mark_transform_dirty(child);
    }

    /// Moves `node` under `new_parent`, optionally keeping its global transform.
    /// Like Godot, the node exits and re-enters the tree; `ready` does not run again.
    pub fn reparent(
        &mut self,
        node: NodeHandle,
        new_parent: NodeHandle,
        keep_global_transform: bool,
    ) -> Result<()> {
        self.ensure_valid(node)?;
        self.ensure_valid(new_parent)?;
        if node == self.root {
            return Err(SceneError::RootNode);
        }
        if node == new_parent || self.is_ancestor_of(node, new_parent) {
            return Err(SceneError::WouldCreateCycle {
                parent: new_parent,
                child: node,
            });
        }
        let global = keep_global_transform
            .then(|| self.global_transform(node))
            .flatten();
        if let Some(old_parent) = self.parent(node) {
            self.remove_child(old_parent, node)?;
        }
        self.add_child(new_parent, node)?;
        if let Some(global) = global {
            self.set_global_transform(node, global)?;
        }
        Ok(())
    }

    /// Moves `child` to `index` among its siblings (clamped to the valid range).
    pub fn move_child(
        &mut self,
        parent: NodeHandle,
        child: NodeHandle,
        index: usize,
    ) -> Result<()> {
        let mut children = self
            .world
            .get_mut::<Children>(parent.entity())
            .ok_or(SceneError::InvalidNode(parent))?;
        let from = children
            .0
            .iter()
            .position(|&c| c == child)
            .ok_or(SceneError::NotAChild { parent, child })?;
        let node = children.0.remove(from);
        let to = index.min(children.0.len());
        children.0.insert(to, node);
        Ok(())
    }

    /// Resolves a [`NodePath`](crate::NodePath)-style string relative to `from`
    /// (or from the root when it starts with `/`).
    pub fn get_node(&self, from: NodeHandle, path: impl AsRef<str>) -> Option<NodeHandle> {
        let path = path.as_ref();
        let mut segments = path::segments(path);
        let mut current = if path::is_absolute(path) {
            // The first segment of an absolute path names the root itself.
            match segments.next() {
                Some(PathSegment::Name(ROOT_NAME)) => self.root,
                Some(_) => return None,
                None => return Some(self.root),
            }
        } else {
            self.ensure_valid(from).ok()?;
            from
        };
        for segment in segments {
            current = match segment {
                PathSegment::Current => current,
                PathSegment::Parent => self.parent(current)?,
                PathSegment::Name(name) => self.child_by_name(current, name)?,
            };
        }
        Some(current)
    }

    fn child_by_name(&self, node: NodeHandle, name: &str) -> Option<NodeHandle> {
        let children = self.world.get::<Children>(node.entity())?;
        let infos = self.world.storage::<NodeInfo>()?;
        children
            .0
            .iter()
            .copied()
            .find(|c| infos.get(c.entity()).is_some_and(|i| i.name == name))
    }

    /// Finds a descendant by exact name: direct children first, then each child's subtree.
    pub fn find_child(&self, from: NodeHandle, name: &str, recursive: bool) -> Option<NodeHandle> {
        if let Some(found) = self.child_by_name(from, name) {
            return Some(found);
        }
        if recursive {
            for child in self.children(from) {
                if let Some(found) = self.find_child(child, name, true) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Absolute path (`/root/Level/Player`) of a node inside the tree.
    pub fn path_of(&self, node: NodeHandle) -> Option<String> {
        if !self.is_inside_tree(node) {
            return None;
        }
        let mut names = Vec::new();
        let mut current = Some(node);
        while let Some(n) = current {
            names.push(self.name(n)?.to_owned());
            current = self.parent(n);
        }
        names.reverse();
        Some(format!("/{}", names.join("/")))
    }

    /// `node` and all of its descendants in tree order (pre-order).
    pub fn subtree(&self, node: NodeHandle) -> Vec<NodeHandle> {
        let mut out = Vec::new();
        self.collect_preorder(node, &mut out, |_| true);
        out
    }

    fn collect_preorder(
        &self,
        node: NodeHandle,
        out: &mut Vec<NodeHandle>,
        mut include: impl FnMut(NodeHandle) -> bool,
    ) {
        let Some(children) = self.world.storage::<Children>() else {
            return;
        };
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            let Some(Children(kids)) = children.get(n.entity()) else {
                continue;
            };
            if include(n) {
                out.push(n);
            }
            stack.extend(kids.iter().rev());
        }
    }

    fn collect_postorder(&self, node: NodeHandle) -> Vec<NodeHandle> {
        let mut out = Vec::new();
        let Some(children) = self.world.storage::<Children>() else {
            return out;
        };
        // (node, children already pushed)
        let mut stack = vec![(node, false)];
        while let Some((n, expanded)) = stack.pop() {
            if expanded {
                out.push(n);
                continue;
            }
            let Some(Children(kids)) = children.get(n.entity()) else {
                continue;
            };
            stack.push((n, true));
            stack.extend(kids.iter().rev().map(|&k| (k, false)));
        }
        out
    }

    fn unique_child_name(
        &self,
        parent: NodeHandle,
        desired: &str,
        ignore: Option<NodeHandle>,
    ) -> String {
        let taken = |name: &str| {
            self.child_by_name(parent, name)
                .is_some_and(|existing| Some(existing) != ignore)
        };
        if !taken(desired) {
            return desired.to_owned();
        }
        let base = desired.trim_end_matches(|c: char| c.is_ascii_digit());
        // The siblings' names once: a thousand "Bullet"s would otherwise scan every sibling
        // for every number tried.
        let names: std::collections::HashSet<String> = self
            .children(parent)
            .into_iter()
            .filter(|&c| Some(c) != ignore)
            .filter_map(|c| self.name(c).map(|n| n.to_string()))
            .filter(|n| n.starts_with(base))
            .collect();
        // `u128` so a suffix of `u64::MAX` still has a successor.
        let mut n: u128 = desired[base.len()..]
            .parse::<u64>()
            .map_or(2, |n| u128::from(n) + 1)
            .max(2);
        loop {
            let candidate = format!("{base}{n}");
            if !names.contains(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    // =========================================================================================
    // Lifecycle
    // =========================================================================================

    fn propagate_enter_tree(&mut self, node: NodeHandle) {
        // Children are read after their parent's callback, so nodes a callback adds below a
        // node that has not entered yet enter in this pass too.
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            // Skip nodes detached by an earlier callback in this pass.
            let attached = n == node || self.parent(n).is_some_and(|p| self.is_inside_tree(p));
            if !attached {
                continue;
            }
            match self.info_mut(n) {
                // Already entered by a nested `add_child` from a callback (with its subtree).
                Ok(info) if info.in_tree => continue,
                Ok(mut info) => info.in_tree = true,
                Err(_) => continue,
            }
            self.with_scripts(n, |script, tree| script.enter_tree(tree, n));
            if let Some(children) = self.world.get::<Children>(n.entity()) {
                stack.extend(children.0.iter().rev());
            }
        }
    }

    fn propagate_ready(&mut self, node: NodeHandle) {
        for n in self.collect_postorder(node) {
            let due = self.info(n).is_some_and(|i| i.in_tree && !i.ready_called);
            if !due {
                continue;
            }
            self.info_mut(n).expect("checked above").ready_called = true;
            self.with_scripts(n, |script, tree| script.ready(tree, n));
        }
    }

    fn propagate_exit_tree(&mut self, node: NodeHandle) {
        let mut order = self.subtree(node);
        order.reverse();
        for n in order {
            if !self.is_inside_tree(n) {
                continue;
            }
            self.with_scripts(n, |script, tree| script.exit_tree(tree, n));
            if let Ok(mut info) = self.info_mut(n) {
                info.in_tree = false;
            }
        }
        // A callback may have attached nodes below a node that had not exited yet; they
        // entered the tree and must leave with it rather than stay "in tree" while detached.
        if self.is_inside_tree(node) {
            return;
        }
        let stragglers: Vec<NodeHandle> = self
            .subtree(node)
            .into_iter()
            .filter(|&n| {
                n != node
                    && self.is_inside_tree(n)
                    && self.parent(n).is_some_and(|p| !self.is_inside_tree(p))
            })
            .collect();
        for n in stragglers {
            self.propagate_exit_tree(n);
        }
    }

    /// Runs `f` on each of the node's scripts in order, each temporarily detached so it can
    /// borrow the tree mutably. Scripts added during the pass run from the next one; removed
    /// ones are skipped. Returns how many ran.
    pub(crate) fn with_scripts(
        &mut self,
        node: NodeHandle,
        mut f: impl FnMut(&mut dyn NodeScript, &mut Self),
    ) -> usize {
        let ids = match self.world.get::<ScriptSlot>(node.entity()) {
            Some(slot) => slot.ids(),
            None => return 0,
        };
        let mut ran = 0;
        for id in ids {
            if self
                .with_script_id(node, id, |script, tree| f(script, tree))
                .is_some()
            {
                ran += 1;
            }
        }
        ran
    }

    /// Runs `f` with one script (by its id in the node's [`ScriptSlot`]) detached. `None` if
    /// it is gone or already running.
    pub(crate) fn with_script_id<R>(
        &mut self,
        node: NodeHandle,
        id: u64,
        f: impl FnOnce(&mut dyn NodeScript, &mut Self) -> R,
    ) -> Option<R> {
        let mut script = self
            .world
            .get_mut::<ScriptSlot>(node.entity())
            .and_then(|mut slot| slot.take(id))?;
        let result = f(script.as_mut(), self);
        // Put it back unless the node was freed or its scripts replaced meanwhile.
        if let Some(mut slot) = self.world.get_mut::<ScriptSlot>(node.entity()) {
            slot.restore(id, script);
        }
        Some(result)
    }

    fn named_script(&self, name: &str) -> Result<Box<dyn NodeScript>> {
        self.world
            .resource::<ScriptRegistry>()
            .and_then(|registry| registry.create(name))
            .ok_or_else(|| SceneError::UnknownScript(name.to_owned()))
    }

    /// Appends to the node's scripts, creating the slot on first use.
    fn push_script(
        &mut self,
        node: NodeHandle,
        script: Box<dyn NodeScript>,
        name: Option<String>,
    ) -> Result<()> {
        if !self.is_valid(node) {
            return Err(SceneError::InvalidNode(node));
        }
        if let Some(mut slot) = self.world.get_mut::<ScriptSlot>(node.entity()) {
            slot.push(script, name);
            return Ok(());
        }
        let mut slot = ScriptSlot::default();
        slot.push(script, name);
        self.insert_component(node, slot)
    }

    /// Attaches `script` as the node's only script (replacing any). Like Godot, attaching to
    /// a node that is already in the tree does not re-run `enter_tree`/`ready`. Hot reload
    /// does not touch scripts attached this way; use [`SceneTree::set_script_named`] for
    /// those. [`SceneTree::add_script`] adds one next to the others.
    pub fn set_script(&mut self, node: NodeHandle, script: impl NodeScript) -> Result<()> {
        self.remove_script(node);
        self.push_script(node, Box::new(Guarded::new(script)), None)
    }

    /// Attaches a new instance of the script registered as `name` in the
    /// [`ScriptRegistry`] resource as the node's only script. Hot reload swaps it for newer
    /// versions.
    pub fn set_script_named(&mut self, node: NodeHandle, name: &str) -> Result<()> {
        let script = self.named_script(name)?;
        self.remove_script(node);
        self.push_script(node, script, Some(name.to_owned()))
    }

    /// Adds `script` after the node's other scripts (a node can have several, like the
    /// components of a Unity object; every callback runs them in order).
    pub fn add_script(&mut self, node: NodeHandle, script: impl NodeScript) -> Result<()> {
        self.push_script(node, Box::new(Guarded::new(script)), None)
    }

    /// Adds a new instance of the script registered as `name` after the node's others.
    pub fn add_script_named(&mut self, node: NodeHandle, name: &str) -> Result<()> {
        let script = self.named_script(name)?;
        self.push_script(node, script, Some(name.to_owned()))
    }

    /// Registry name of the node's first script, if it was attached by name.
    pub fn script_name(&self, node: NodeHandle) -> Option<String> {
        self.world
            .get::<ScriptSlot>(node.entity())?
            .entries
            .first()?
            .name
            .clone()
    }

    /// Registry names of the node's scripts attached by name, in order.
    pub fn script_names(&self, node: NodeHandle) -> Vec<String> {
        self.world
            .get::<ScriptSlot>(node.entity())
            .map(|slot| slot.entries.iter().filter_map(|e| e.name.clone()).collect())
            .unwrap_or_default()
    }

    /// How many scripts the node has.
    pub fn script_count(&self, node: NodeHandle) -> usize {
        self.world
            .get::<ScriptSlot>(node.entity())
            .map_or(0, |slot| slot.entries.len())
    }

    /// Removes every script of the node.
    pub fn remove_script(&mut self, node: NodeHandle) -> bool {
        self.world.remove::<ScriptSlot>(node.entity()).is_some()
    }

    /// Removes the node's first script registered as `name`.
    pub fn remove_script_named(&mut self, node: NodeHandle, name: &str) -> bool {
        let Some(mut slot) = self.world.get_mut::<ScriptSlot>(node.entity()) else {
            return false;
        };
        let Some(index) = slot
            .entries
            .iter()
            .position(|e| e.name.as_deref() == Some(name))
        else {
            return false;
        };
        slot.entries.remove(index);
        let empty = slot.entries.is_empty();
        drop(slot);
        if empty {
            self.remove_script(node);
        }
        true
    }

    pub fn has_script(&self, node: NodeHandle) -> bool {
        self.world.has::<ScriptSlot>(node.entity())
    }

    /// Runs `physics_process(delta)` on every scripted node in tree order. Call once per
    /// fixed step.
    pub fn physics_process(&mut self, delta: f32) {
        self.flush_signals();
        self.dispatch(ProcessKind::Physics, delta);
    }

    /// End-of-frame update: runs `process(delta)` on scripted nodes in tree order, propagates
    /// transforms, then frees nodes queued with [`SceneTree::queue_free`].
    pub fn process(&mut self, delta: f32) {
        self.flush_signals();
        self.update_timers(delta);
        self.dispatch(ProcessKind::Process, delta);
        self.update_transforms();
        self.flush_deletions();
    }

    fn dispatch(&mut self, kind: ProcessKind, delta: f32) {
        if self
            .world
            .storage::<ScriptSlot>()
            .is_none_or(|s| s.is_empty())
        {
            return;
        }
        // Snapshot first: callbacks may add or remove nodes. New nodes start next frame.
        let mut nodes = std::mem::take(&mut self.dispatch_buffer);
        nodes.clear();
        {
            let scripts = self.world.storage::<ScriptSlot>().expect("checked above");
            self.collect_preorder(self.root, &mut nodes, |n| scripts.contains(n.entity()));
        }
        let profile = self
            .world
            .resource::<ScriptProfile>()
            .is_some_and(|p| p.enabled);
        let mut timings = Vec::new();
        for &node in &nodes {
            let enabled = self.info(node).is_some_and(|i| {
                i.in_tree
                    && match kind {
                        ProcessKind::Process => i.process_enabled,
                        ProcessKind::Physics => i.physics_process_enabled,
                    }
            });
            if !enabled {
                continue;
            }
            self.with_scripts(node, |script, tree| {
                let start = profile.then(std::time::Instant::now);
                match kind {
                    ProcessKind::Process => script.process(tree, node, delta),
                    ProcessKind::Physics => script.physics_process(tree, node, delta),
                }
                if let Some(start) = start {
                    timings.push((script.script_type_name(), start.elapsed()));
                }
            });
        }
        if let Some(mut profile) = self.world.resource_mut::<ScriptProfile>() {
            for (name, duration) in timings {
                profile.add(name, duration);
            }
        }
        self.dispatch_buffer = nodes;
    }

    // =========================================================================================
    // Deletion
    // =========================================================================================

    /// Schedules the node and its subtree to be freed at the end of the current frame.
    /// Queuing twice is harmless.
    pub fn queue_free(&mut self, node: NodeHandle) -> Result<()> {
        if node == self.root {
            return Err(SceneError::RootNode);
        }
        let mut info = self.info_mut(node)?;
        if !info.queued_for_deletion {
            info.queued_for_deletion = true;
            drop(info);
            self.free_queue.push(node);
        }
        Ok(())
    }

    /// Frees every queued node now (normally called by [`SceneTree::process`]).
    pub fn flush_deletions(&mut self) {
        let mut i = 0;
        // `exit_tree` callbacks may queue more nodes, so the queue can grow while draining.
        while i < self.free_queue.len() {
            let node = self.free_queue[i];
            i += 1;
            self.free_now(node);
        }
        if !self.free_queue.is_empty() {
            let world = &self.world;
            self.signals.retain_alive(|e| world.is_alive(e));
        }
        self.free_queue.clear();
    }

    /// Frees `node` (and its children) right away, unlike [`SceneTree::queue_free`]; other
    /// nodes waiting in the free queue stay there until the end of the frame. For nodes a
    /// tool or loader made and must take back now (a scene that failed half-way, a
    /// generator's previous output).
    pub fn free_immediately(&mut self, node: NodeHandle) {
        if node == self.root() || !self.is_valid(node) {
            return;
        }
        self.free_now(node);
        let world = &self.world;
        self.signals.retain_alive(|e| world.is_alive(e));
    }

    fn free_now(&mut self, node: NodeHandle) {
        if !self.is_valid(node) {
            return;
        }
        if let Some(parent) = self.parent(node) {
            let _ = self.remove_child(parent, node);
            // An `exit_tree` callback may have attached the node elsewhere. As in Godot, a
            // queued node is freed regardless, so detach it from wherever it ended up instead
            // of leaving a dead handle in that parent's children.
            if let Some(parent) = self.parent(node) {
                let _ = self.remove_child(parent, node);
                if let Some(parent) = self.parent(node) {
                    self.unlink(parent, node);
                }
            }
        }
        for n in self.subtree(node) {
            self.world.despawn(n.entity());
        }
    }

    // =========================================================================================
    // Node3D
    // =========================================================================================

    /// Recomputes cached global transforms for the tree. Called by [`SceneTree::process`];
    /// reading [`SceneTree::global_transform`] is always up to date regardless.
    pub fn update_transforms(&mut self) {
        propagate_transforms(&self.world, self.root);
    }

    pub fn is_node3d(&self, node: NodeHandle) -> bool {
        self.world.has::<Transform>(node.entity())
    }

    fn mark_transform_dirty(&self, node: NodeHandle) {
        if let Some(mut t) = self.world.get_mut::<Transform>(node.entity()) {
            t.dirty = true;
        }
    }

    fn modify_local(&mut self, node: NodeHandle, f: impl FnOnce(&mut Transform3D)) -> Result<()> {
        let mut t = self
            .world
            .get_mut::<Transform>(node.entity())
            .ok_or_else(|| {
                if self.is_valid(node) {
                    SceneError::NotNode3D(node)
                } else {
                    SceneError::InvalidNode(node)
                }
            })?;
        let mut local = t.local;
        f(&mut local);
        // One NaN (a zero vector normalised, a parent scaled to zero) would stay in the node
        // and its physics body for good: refused, the transform is left as it was.
        let finite =
            local.translation.is_finite() && local.rotation.is_finite() && local.scale.is_finite();
        if !finite {
            return Err(SceneError::NonFiniteTransform(node));
        }
        t.local = local;
        t.dirty = true;
        Ok(())
    }

    /// Local transform (relative to the parent).
    pub fn transform(&self, node: NodeHandle) -> Option<Transform3D> {
        self.world.get::<Transform>(node.entity()).map(|t| t.local)
    }

    pub fn set_transform(&mut self, node: NodeHandle, transform: Transform3D) -> Result<()> {
        self.modify_local(node, |local| *local = transform)
    }

    pub fn position(&self, node: NodeHandle) -> Option<Vec3> {
        self.transform(node).map(|t| t.translation)
    }

    pub fn set_position(&mut self, node: NodeHandle, position: Vec3) -> Result<()> {
        self.modify_local(node, |local| local.translation = position)
    }

    pub fn rotation(&self, node: NodeHandle) -> Option<Quat> {
        self.transform(node).map(|t| t.rotation)
    }

    pub fn set_rotation(&mut self, node: NodeHandle, rotation: Quat) -> Result<()> {
        let length = rotation.length();
        // A zero or NaN quaternion (e.g. from a bad calculation) would make the node vanish.
        let rotation = if rotation.is_finite() && length > 1e-6 {
            rotation / length
        } else {
            Quat::IDENTITY
        };
        self.modify_local(node, |local| local.rotation = rotation)
    }

    pub fn scale(&self, node: NodeHandle) -> Option<Vec3> {
        self.transform(node).map(|t| t.scale)
    }

    pub fn set_scale(&mut self, node: NodeHandle, scale: Vec3) -> Result<()> {
        self.modify_local(node, |local| local.scale = scale)
    }

    /// Moves the node by `offset` expressed in its own (rotated and scaled) axes, like Godot's
    /// `Node3D.translate`.
    pub fn translate(&mut self, node: NodeHandle, offset: Vec3) -> Result<()> {
        self.modify_local(node, |local| {
            local.translation += local.transform_vector(offset)
        })
    }

    /// Moves the node by `offset` in global space.
    pub fn global_translate(&mut self, node: NodeHandle, offset: Vec3) -> Result<()> {
        let position = self
            .global_position(node)
            .ok_or(SceneError::NotNode3D(node))?;
        self.set_global_position(node, position + offset)
    }

    /// Rotates around `axis` (in parent space) by `angle` radians.
    pub fn rotate(&mut self, node: NodeHandle, axis: Vec3, angle: f32) -> Result<()> {
        let rotation = Quat::from_axis_angle(axis.normalize_or(Vec3::Y), angle);
        self.modify_local(node, |local| local.rotate(rotation))
    }

    /// Rotates around `axis` (in the node's own space) by `angle` radians.
    pub fn rotate_object_local(&mut self, node: NodeHandle, axis: Vec3, angle: f32) -> Result<()> {
        let rotation = Quat::from_axis_angle(axis.normalize_or(Vec3::Y), angle);
        self.modify_local(node, |local| local.rotate_local(rotation))
    }

    /// Global transform. Always current: if anything along the parent chain changed since the
    /// last propagation it is recomputed on the fly.
    pub fn global_transform(&self, node: NodeHandle) -> Option<Affine3A> {
        let transforms = self.world.storage::<Transform>()?;
        let own = transforms.get(node.entity())?;

        // Walk the Node3D ancestor chain (it ends at a plain Node or the top). If nothing on it
        // is dirty the cached value is valid; that is the common case and does not allocate.
        let chain = || {
            std::iter::successors(Some(node), |&n| self.parent(n))
                .map_while(|n| transforms.get(n.entity()))
        };
        if !chain().any(|t| t.dirty) {
            return Some(own.global);
        }
        let locals: Vec<Affine3A> = chain().map(|t| t.local.to_affine()).collect();
        Some(
            locals
                .iter()
                .rev()
                .fold(Affine3A::IDENTITY, |parent, local| parent * *local),
        )
    }

    pub fn global_position(&self, node: NodeHandle) -> Option<Vec3> {
        self.global_transform(node).map(|g| g.translation.into())
    }

    /// Global transform of the node's parent space (identity under a plain `Node`).
    fn parent_global(&self, node: NodeHandle) -> Affine3A {
        self.parent(node)
            .and_then(|p| self.global_transform(p))
            .unwrap_or(Affine3A::IDENTITY)
    }

    /// Sets the global transform by solving for the local one. Shear is discarded.
    pub fn set_global_transform(&mut self, node: NodeHandle, global: Affine3A) -> Result<()> {
        let local = self.parent_global(node).inverse() * global;
        self.modify_local(node, |t| *t = Transform3D::from_affine(&local))
    }

    pub fn set_global_position(&mut self, node: NodeHandle, position: Vec3) -> Result<()> {
        let local = self
            .parent_global(node)
            .inverse()
            .transform_point3(position);
        self.modify_local(node, |t| t.translation = local)
    }

    /// Rotates the node so its -Z axis points at the global `target`. Does nothing if the
    /// target is at the node's position or straight along `up`.
    pub fn look_at(&mut self, node: NodeHandle, target: Vec3, up: Vec3) -> Result<()> {
        let global = self.global_transform(node).ok_or_else(|| {
            if self.is_valid(node) {
                SceneError::NotNode3D(node)
            } else {
                SceneError::InvalidNode(node)
            }
        })?;
        let (scale, _, position) = global.to_scale_rotation_translation();
        let Some(rotation) = look_rotation(target - position, up) else {
            return Ok(());
        };
        let global = Affine3A::from_scale_rotation_translation(scale, rotation, position);
        self.set_global_transform(node, global)
    }
}

impl Default for SceneTree {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SceneTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SceneTree")
            .field("root", &self.root)
            .field("nodes", &self.node_count())
            .field("queued_frees", &self.free_queue.len())
            .finish()
    }
}

fn sanitize_name(name: &str, fallback: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return fallback.to_owned();
    }
    name.replace(INVALID_NAME_CHARS, "_")
}
