//! [`Tree`]: the scene, as a script sees it during a callback.

use crate::host::{host, method, string_sink, value_sink, MethodCell};
use crate::math::{Quat, Transform, Vec2, Vec3};
use crate::value::{Arg, Error, ErrorKind, FromValue, IntoValue, Node};
use crate::Script;
use ab_script_api as ffi;
use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;
use std::ptr::NonNull;

/// The scene tree and the engine, during one script callback.
///
/// Every node operation goes through it. It only exists while the engine runs your
/// callback, so it cannot be stored; store [`Node`]s instead.
///
/// Queries return `Option` (`None` when the node is gone); changes return `Result`, which
/// you may ignore with `let _ =` when failure is fine (e.g. the node was freed).
pub struct Tree<'a> {
    ctx: NonNull<ffi::Context>,
    _event: PhantomData<&'a mut ffi::Context>,
}

impl std::fmt::Debug for Tree<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tree").finish_non_exhaustive()
    }
}

/// Identifies a connection made with [`Tree::connect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Connection(u64);

impl<'a> Tree<'a> {
    /// # Safety
    /// `ctx` must be the context of the event being run, valid for `'a`.
    pub(crate) unsafe fn from_context(ctx: NonNull<ffi::Context>) -> Self {
        Self {
            ctx,
            _event: PhantomData,
        }
    }

    // =========================================================================================
    // Calling the engine
    // =========================================================================================

    /// Calls a host method; `each` sees every result.
    pub(crate) fn call(
        &self,
        method: &MethodCell,
        args: &[ffi::Value],
        mut each: impl FnMut(&ffi::Value),
    ) -> Result<(), Error> {
        let Some(host) = host() else {
            return Err(Error::new(ErrorKind::Failed, "not connected to the engine"));
        };
        let id = method.id();
        if id == ffi::MethodId::UNKNOWN {
            return Err(Error::new(
                ErrorKind::UnknownMethod,
                format!(
                    "this engine has no `{}`: it is older than this ab-script ({}); update \
                     the engine or the editor",
                    method.name(),
                    env!("CARGO_PKG_VERSION")
                ),
            ));
        }
        let mut sink = value_sink(&mut each);
        // SAFETY: the context is alive (we are inside its event); the arguments and the sink
        // outlive the call.
        let status =
            unsafe { (host.call)(self.ctx.as_ptr(), id, args.as_ptr(), args.len(), &mut sink) };
        if status.is_ok() {
            return Ok(());
        }
        let mut message = String::new();
        let mut sink = string_sink(&mut message);
        // SAFETY: as above.
        unsafe { (host.error)(self.ctx.as_ptr(), &mut sink) };
        let kind = match status {
            ffi::Status::NO_NODE => ErrorKind::NoNode,
            ffi::Status::UNKNOWN_METHOD => ErrorKind::UnknownMethod,
            ffi::Status::BAD_ARGUMENTS => ErrorKind::BadArguments,
            _ => ErrorKind::Failed,
        };
        if message.is_empty() {
            message = format!("`{}` failed", method.name());
        }
        Err(Error::new(kind, message))
    }

    /// Calls a method for its effect.
    pub(crate) fn run(&self, method: &MethodCell, args: &[ffi::Value]) -> Result<(), Error> {
        self.call(method, args, |_| {})
    }

    /// The first result as `T` (`Ok(None)` if there is none or it has another type).
    pub(crate) fn fetch<T: FromValue>(
        &self,
        method: &MethodCell,
        args: &[ffi::Value],
    ) -> Result<Option<T>, Error> {
        let mut result = None;
        self.call(method, args, |value| {
            if result.is_none() {
                result = Some(T::from_value(value));
            }
        })?;
        Ok(result.flatten())
    }

    /// A query: `None` on failure. Failures other than a missing node are bugs in the
    /// script (wrong names, wrong node types) and are logged once per call site.
    pub(crate) fn query<T: FromValue>(
        &self,
        method: &MethodCell,
        args: &[ffi::Value],
    ) -> Option<T> {
        match self.fetch(method, args) {
            Ok(value) => value,
            Err(err) => {
                report(method, &err);
                None
            }
        }
    }

    /// Every result converted to `T` (those of other types are skipped).
    pub(crate) fn list<T: FromValue>(&self, method: &MethodCell, args: &[ffi::Value]) -> Vec<T> {
        let mut out = Vec::new();
        if let Err(err) = self.call(method, args, |value| out.extend(T::from_value(value))) {
            report(method, &err);
        }
        out
    }

    /// Every result, kept as [`Arg`]s.
    pub(crate) fn values(
        &self,
        method: &MethodCell,
        args: &[ffi::Value],
    ) -> Result<Vec<Arg>, Error> {
        let mut out = Vec::new();
        self.call(method, args, |value| out.push(Arg::from_ffi(value)))?;
        Ok(out)
    }

    fn transform_of(&self, method: &MethodCell, node: Node) -> Option<Transform> {
        let values = self.values(method, &[node.to_value()]).ok()?;
        match values.as_slice() {
            [Arg::Vec3(translation), Arg::Quat(rotation), Arg::Vec3(scale)] => Some(Transform {
                translation: *translation,
                rotation: *rotation,
                scale: *scale,
            }),
            _ => None,
        }
    }

    // =========================================================================================
    // Properties
    // =========================================================================================

    /// A property of a node by path, e.g. `"light.energy"` or `"body.linear_velocity"`
    /// (the typed accessors such as [`Tree::light`] use these).
    pub fn get<T: FromValue>(&self, node: Node, property: &str) -> Option<T> {
        self.query(
            method!("node.get_property"),
            &[node.to_value(), property.to_value()],
        )
    }

    /// Sets a property of a node by path (see [`Tree::get`]).
    pub fn set(&mut self, node: Node, property: &str, value: impl IntoValue) -> Result<(), Error> {
        self.run(
            method!("node.set_property"),
            &[node.to_value(), property.to_value(), value.to_value()],
        )
    }

    // =========================================================================================
    // Nodes
    // =========================================================================================

    /// The tree's root node.
    pub fn root(&self) -> Node {
        self.query(method!("tree.root"), &[])
            .unwrap_or(Node::from_bits(0))
    }

    /// Whether `node` exists (it may be outside the tree).
    pub fn is_valid(&self, node: Node) -> bool {
        self.query(method!("node.is_valid"), &[node.to_value()])
            .unwrap_or(false)
    }

    pub fn is_inside_tree(&self, node: Node) -> bool {
        self.query(method!("node.is_inside_tree"), &[node.to_value()])
            .unwrap_or(false)
    }

    /// Number of nodes in the tree.
    pub fn node_count(&self) -> usize {
        self.query(method!("tree.node_count"), &[]).unwrap_or(0)
    }

    /// A new, detached `Node`; add it with [`Tree::add_child`].
    pub fn create_node(&mut self, name: &str) -> Result<Node, Error> {
        self.created(method!("node.create"), &[name.to_value()])
    }

    /// A new, detached `Node3D`.
    pub fn create_node3d(&mut self, name: &str) -> Result<Node, Error> {
        self.created(method!("node.create_3d"), &[name.to_value()])
    }

    pub(crate) fn created(
        &mut self,
        method: &MethodCell,
        args: &[ffi::Value],
    ) -> Result<Node, Error> {
        self.fetch(method, args)?.ok_or_else(|| {
            Error::new(
                ErrorKind::Failed,
                format!("`{}` made no node", method.name()),
            )
        })
    }

    pub fn name(&self, node: Node) -> Option<String> {
        self.query(method!("node.name"), &[node.to_value()])
    }

    pub fn set_name(&mut self, node: Node, name: &str) -> Result<(), Error> {
        self.run(
            method!("node.set_name"),
            &[node.to_value(), name.to_value()],
        )
    }

    /// The node's type, e.g. `"MeshInstance3D"`.
    pub fn class_name(&self, node: Node) -> Option<String> {
        self.query(method!("node.class_name"), &[node.to_value()])
    }

    /// The node at `path` from `from`: `"Child/Grandchild"`, `"../Sibling"`, `"/Root/X"`.
    pub fn get_node(&self, from: Node, path: &str) -> Option<Node> {
        self.query(
            method!("node.get_node"),
            &[from.to_value(), path.to_value()],
        )
    }

    /// The first descendant named `name`.
    pub fn find_child(&self, node: Node, name: &str) -> Option<Node> {
        self.query(
            method!("node.find_child"),
            &[node.to_value(), name.to_value()],
        )
    }

    pub fn parent(&self, node: Node) -> Option<Node> {
        self.query(method!("node.parent"), &[node.to_value()])
    }

    pub fn children(&self, node: Node) -> Vec<Node> {
        self.list(method!("node.children"), &[node.to_value()])
    }

    pub fn child_count(&self, node: Node) -> usize {
        self.query(method!("node.child_count"), &[node.to_value()])
            .unwrap_or(0)
    }

    /// The node's path from the root, e.g. `"/root/Level/Player"`.
    pub fn path_of(&self, node: Node) -> Option<String> {
        self.query(method!("node.path"), &[node.to_value()])
    }

    pub fn add_child(&mut self, parent: Node, child: Node) -> Result<(), Error> {
        self.run(
            method!("node.add_child"),
            &[parent.to_value(), child.to_value()],
        )
    }

    /// Detaches `child` (it stays alive; free it or add it elsewhere).
    pub fn remove_child(&mut self, parent: Node, child: Node) -> Result<(), Error> {
        self.run(
            method!("node.remove_child"),
            &[parent.to_value(), child.to_value()],
        )
    }

    /// Moves `node` under `new_parent`, keeping its global transform if `keep_global`.
    pub fn reparent(
        &mut self,
        node: Node,
        new_parent: Node,
        keep_global: bool,
    ) -> Result<(), Error> {
        self.run(
            method!("node.reparent"),
            &[
                node.to_value(),
                new_parent.to_value(),
                keep_global.to_value(),
            ],
        )
    }

    /// Frees the node and its children at the end of the frame.
    pub fn queue_free(&mut self, node: Node) -> Result<(), Error> {
        self.run(method!("node.queue_free"), &[node.to_value()])
    }

    pub fn is_visible(&self, node: Node) -> bool {
        self.query(method!("node.is_visible"), &[node.to_value()])
            .unwrap_or(false)
    }

    /// Visible and every ancestor visible.
    pub fn is_visible_in_tree(&self, node: Node) -> bool {
        self.query(method!("node.is_visible_in_tree"), &[node.to_value()])
            .unwrap_or(false)
    }

    pub fn set_visible(&mut self, node: Node, visible: bool) -> Result<(), Error> {
        self.run(
            method!("node.set_visible"),
            &[node.to_value(), visible.to_value()],
        )
    }

    /// Turns the node's `process` callbacks on or off.
    pub fn set_process(&mut self, node: Node, enabled: bool) -> Result<(), Error> {
        self.run(
            method!("node.set_process"),
            &[node.to_value(), enabled.to_value()],
        )
    }

    /// Turns the node's `physics_process` callbacks on or off.
    pub fn set_physics_process(&mut self, node: Node, enabled: bool) -> Result<(), Error> {
        self.run(
            method!("node.set_physics_process"),
            &[node.to_value(), enabled.to_value()],
        )
    }

    pub fn is_processing(&self, node: Node) -> bool {
        self.query(method!("node.is_processing"), &[node.to_value()])
            .unwrap_or(false)
    }

    pub fn is_physics_processing(&self, node: Node) -> bool {
        self.query(method!("node.is_physics_processing"), &[node.to_value()])
            .unwrap_or(false)
    }

    // =========================================================================================
    // Groups
    // =========================================================================================

    pub fn add_to_group(&mut self, node: Node, group: &str) -> Result<(), Error> {
        self.run(method!("group.add"), &[node.to_value(), group.to_value()])
    }

    pub fn remove_from_group(&mut self, node: Node, group: &str) -> Result<(), Error> {
        self.run(
            method!("group.remove"),
            &[node.to_value(), group.to_value()],
        )
    }

    pub fn is_in_group(&self, node: Node, group: &str) -> bool {
        self.query(method!("group.has"), &[node.to_value(), group.to_value()])
            .unwrap_or(false)
    }

    /// The group's nodes in the tree.
    pub fn nodes_in_group(&self, group: &str) -> Vec<Node> {
        self.list(method!("group.nodes"), &[group.to_value()])
    }

    // =========================================================================================
    // Transforms (3D nodes)
    // =========================================================================================

    pub fn position(&self, node: Node) -> Option<Vec3> {
        self.query(method!("node.position"), &[node.to_value()])
    }

    pub fn set_position(&mut self, node: Node, position: Vec3) -> Result<(), Error> {
        self.run(
            method!("node.set_position"),
            &[node.to_value(), position.to_value()],
        )
    }

    pub fn rotation(&self, node: Node) -> Option<Quat> {
        self.query(method!("node.rotation"), &[node.to_value()])
    }

    pub fn set_rotation(&mut self, node: Node, rotation: Quat) -> Result<(), Error> {
        self.run(
            method!("node.set_rotation"),
            &[node.to_value(), rotation.to_value()],
        )
    }

    pub fn scale(&self, node: Node) -> Option<Vec3> {
        self.query(method!("node.scale"), &[node.to_value()])
    }

    pub fn set_scale(&mut self, node: Node, scale: Vec3) -> Result<(), Error> {
        self.run(
            method!("node.set_scale"),
            &[node.to_value(), scale.to_value()],
        )
    }

    /// The local transform (relative to the parent).
    pub fn transform(&self, node: Node) -> Option<Transform> {
        self.transform_of(method!("node.transform"), node)
    }

    pub fn set_transform(&mut self, node: Node, transform: Transform) -> Result<(), Error> {
        self.run(
            method!("node.set_transform"),
            &[
                node.to_value(),
                transform.translation.to_value(),
                transform.rotation.to_value(),
                transform.scale.to_value(),
            ],
        )
    }

    /// The transform in the world, as of the last transform update (start of the frame or
    /// [`Tree::update_transforms`]).
    pub fn global_transform(&self, node: Node) -> Option<Transform> {
        self.transform_of(method!("node.global_transform"), node)
    }

    pub fn set_global_transform(&mut self, node: Node, transform: Transform) -> Result<(), Error> {
        self.run(
            method!("node.set_global_transform"),
            &[
                node.to_value(),
                transform.translation.to_value(),
                transform.rotation.to_value(),
                transform.scale.to_value(),
            ],
        )
    }

    pub fn global_position(&self, node: Node) -> Option<Vec3> {
        self.query(method!("node.global_position"), &[node.to_value()])
    }

    pub fn set_global_position(&mut self, node: Node, position: Vec3) -> Result<(), Error> {
        self.run(
            method!("node.set_global_position"),
            &[node.to_value(), position.to_value()],
        )
    }

    /// Moves by `offset` in the parent's space.
    pub fn translate(&mut self, node: Node, offset: Vec3) -> Result<(), Error> {
        self.run(
            method!("node.translate"),
            &[node.to_value(), offset.to_value()],
        )
    }

    /// Moves by `offset` in world space.
    pub fn global_translate(&mut self, node: Node, offset: Vec3) -> Result<(), Error> {
        self.run(
            method!("node.global_translate"),
            &[node.to_value(), offset.to_value()],
        )
    }

    /// Rotates around `axis` (parent space) by `angle` radians.
    pub fn rotate(&mut self, node: Node, axis: Vec3, angle: f32) -> Result<(), Error> {
        self.run(
            method!("node.rotate"),
            &[node.to_value(), axis.to_value(), angle.to_value()],
        )
    }

    /// Rotates around the node's own `axis` by `angle` radians.
    pub fn rotate_object_local(&mut self, node: Node, axis: Vec3, angle: f32) -> Result<(), Error> {
        self.run(
            method!("node.rotate_object_local"),
            &[node.to_value(), axis.to_value(), angle.to_value()],
        )
    }

    /// Turns the node so its -Z axis points at `target` (world space).
    pub fn look_at(&mut self, node: Node, target: Vec3, up: Vec3) -> Result<(), Error> {
        self.run(
            method!("node.look_at"),
            &[node.to_value(), target.to_value(), up.to_value()],
        )
    }

    /// Recomputes global transforms now (they are otherwise updated once per frame).
    pub fn update_transforms(&mut self) {
        let _ = self.run(method!("tree.update_transforms"), &[]);
    }

    // =========================================================================================
    // Scripts
    // =========================================================================================

    /// Replaces the node's scripts with the one registered as `name`.
    pub fn set_script(&mut self, node: Node, name: &str) -> Result<(), Error> {
        self.run(method!("script.set"), &[node.to_value(), name.to_value()])
    }

    /// Adds the script registered as `name` after the node's other scripts.
    pub fn add_script(&mut self, node: Node, name: &str) -> Result<(), Error> {
        self.run(method!("script.add"), &[node.to_value(), name.to_value()])
    }

    pub fn remove_script(&mut self, node: Node, name: &str) -> Result<(), Error> {
        self.run(
            method!("script.remove"),
            &[node.to_value(), name.to_value()],
        )
    }

    /// Names of the node's scripts, in order.
    pub fn script_names(&self, node: Node) -> Vec<String> {
        self.list(method!("script.names"), &[node.to_value()])
    }

    /// The node's script of type `T` (from this library), to read it.
    pub fn script<T: Script>(&mut self, node: Node) -> Option<&T> {
        self.script_ptr::<T>(node)
            // SAFETY: the engine returned the instance of `T`'s script type, created by this
            // library; it stays valid while `self` is borrowed (no other engine call can run).
            .map(|p| unsafe { &*p })
    }

    /// The node's script of type `T` (from this library), to change it.
    pub fn script_mut<T: Script>(&mut self, node: Node) -> Option<&mut T> {
        self.script_ptr::<T>(node)
            // SAFETY: as in `script`; the borrow of `self` is exclusive.
            .map(|p| unsafe { &mut *p })
    }

    fn script_ptr<T: Script>(&mut self, node: Node) -> Option<*mut T> {
        let host = host()?;
        let script_type = crate::script_type::<T>()?;
        // SAFETY: a live context; `script_type` belongs to this library.
        let instance = unsafe { (host.script)(self.ctx.as_ptr(), node.id(), script_type) };
        (!instance.is_null()).then(|| instance.cast::<T>())
    }

    // =========================================================================================
    // Signals
    // =========================================================================================

    /// Delivers `source`'s `signal` to `target`'s scripts (their `on_signal`).
    pub fn connect(
        &mut self,
        source: Node,
        signal: &str,
        target: Node,
    ) -> Result<Connection, Error> {
        self.fetch::<u64>(
            method!("signal.connect"),
            &[source.to_value(), signal.to_value(), target.to_value()],
        )?
        .map(Connection)
        .ok_or_else(|| Error::new(ErrorKind::Failed, "no connection was made"))
    }

    /// Returns whether the connection existed.
    pub fn disconnect(&mut self, connection: Connection) -> bool {
        self.fetch(method!("signal.disconnect"), &[connection.0.to_value()])
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    /// Emits `signal` from `source` to everything connected to it.
    pub fn emit(&mut self, source: Node, signal: &str) -> Result<(), Error> {
        self.run(
            method!("signal.emit"),
            &[source.to_value(), signal.to_value()],
        )
    }

    /// Emits `signal` with an argument.
    pub fn emit_with(
        &mut self,
        source: Node,
        signal: &str,
        arg: impl Into<Arg>,
    ) -> Result<(), Error> {
        let arg = arg.into();
        self.run(
            method!("signal.emit"),
            &[source.to_value(), signal.to_value(), arg.to_value()],
        )
    }

    // =========================================================================================
    // Time
    // =========================================================================================

    /// Seconds of game time since the start.
    pub fn elapsed(&self) -> f64 {
        self.query(method!("time.elapsed"), &[]).unwrap_or(0.0)
    }

    /// Frames since the start.
    pub fn frame(&self) -> u64 {
        self.query(method!("time.frame"), &[]).unwrap_or(0)
    }

    /// Seconds per fixed (physics) step.
    pub fn fixed_delta(&self) -> f32 {
        self.query(method!("time.fixed_delta"), &[])
            .unwrap_or(1.0 / 60.0)
    }

    /// Game speed: 1 is normal, 0 pauses time-based things.
    pub fn time_scale(&self) -> f32 {
        self.query(method!("time.scale"), &[]).unwrap_or(1.0)
    }

    pub fn set_time_scale(&mut self, scale: f32) {
        let _ = self.run(method!("time.set_scale"), &[scale.to_value()]);
    }

    /// Frames per second, averaged by the engine.
    pub fn fps(&self) -> f32 {
        self.query(method!("time.fps"), &[]).unwrap_or(0.0)
    }

    // =========================================================================================
    // Input and the engine
    // =========================================================================================

    /// The action map (`input.toml`): actions by name.
    pub fn input(&self) -> Input<'_, 'a> {
        Input { tree: self }
    }

    /// Quitting, the mouse, the window.
    pub fn engine(&mut self) -> Engine<'_, 'a> {
        Engine { tree: self }
    }

    // =========================================================================================
    // Scenes, files and saves
    // =========================================================================================

    /// Loads a scene file (`"levels/one.abscene"`, or a glTF model) as a detached node.
    pub fn load_scene(&mut self, path: &str) -> Result<Node, Error> {
        self.created(method!("scene.load"), &[path.to_value()])
    }

    /// Loads a scene file and adds it under `parent`.
    pub fn instantiate(&mut self, path: &str, parent: Node) -> Result<Node, Error> {
        self.created(
            method!("scene.instantiate"),
            &[path.to_value(), parent.to_value()],
        )
    }

    /// Builds nodes from scene-file text (the TOML inside `.abscene` scenes), detached.
    pub fn instantiate_toml(&mut self, toml: &str) -> Result<Node, Error> {
        self.created(method!("scene.from_toml"), &[toml.to_value()])
    }

    /// Replaces the current scene (freed at the end of the frame) with the scene at `path`.
    pub fn change_scene(&mut self, path: &str) -> Result<Node, Error> {
        self.created(method!("scene.change"), &[path.to_value()])
    }

    /// Loads the current scene again from its file.
    pub fn reload_scene(&mut self) -> Result<Node, Error> {
        self.created(method!("scene.reload"), &[])
    }

    /// Starts loading everything the scene file at `path` uses (models, textures, sounds,
    /// instanced scenes) in the background, so [`Tree::load_scene`] or
    /// [`Tree::change_scene`] later does not stall the game. Watch
    /// [`Tree::preload_progress`]; calling it again restarts the preload.
    pub fn preload_scene(&mut self, path: &str) -> Result<(), Error> {
        self.run(method!("scene.preload"), &[path.to_value()])
    }

    /// How far [`Tree::preload_scene`] has got for `path`; `None` if it was not called.
    pub fn preload_progress(&self, path: &str) -> Option<PreloadProgress> {
        let values = self
            .values(method!("scene.preload_progress"), &[path.to_value()])
            .ok()?;
        let at = |i: usize| values.get(i).and_then(Arg::get::<u32>).unwrap_or(0);
        Some(PreloadProgress {
            loaded: at(0),
            total: at(1),
            ready: values.get(2)?.get::<bool>()?,
            error: values.get(3).and_then(Arg::get::<String>),
        })
    }

    /// Starts loading one model (`.glb`, `.gltf`), image (`.png`, `.jpg`) or sound (`.wav`,
    /// `.ogg`, `.mp3`, `.flac`) in the background; using it later does not wait.
    pub fn preload_asset(&mut self, path: &str) -> Result<(), Error> {
        self.run(method!("assets.preload"), &[path.to_value()])
    }

    /// Background loads not finished yet (0 once everything requested is ready).
    pub fn pending_loads(&self) -> u32 {
        self.fetch::<u32>(method!("assets.pending"), &[])
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    /// Plays a `Cutscene` node from its start: its camera takes over until it ends. The
    /// node emits `cutscene_event` (with the event's name) for its signal events, and
    /// `finished` at the end.
    pub fn play_cutscene(&mut self, cutscene: Node) -> Result<(), Error> {
        self.run(method!("cutscene.play"), &[cutscene.to_value()])
    }

    /// Stops a cutscene where it is (a skip): the camera is handed back, `finished` emitted.
    pub fn stop_cutscene(&mut self, cutscene: Node) -> Result<(), Error> {
        self.run(method!("cutscene.stop"), &[cutscene.to_value()])
    }

    pub fn is_cutscene_playing(&self, cutscene: Node) -> bool {
        self.query::<bool>(method!("cutscene.playing"), &[cutscene.to_value()])
            .unwrap_or(false)
    }

    /// Seconds since the cutscene started.
    pub fn cutscene_time(&self, cutscene: Node) -> f32 {
        self.query::<f32>(method!("cutscene.time"), &[cutscene.to_value()])
            .unwrap_or(0.0)
    }

    /// Sets a parameter of the animation state machine on `node` (or in it): a number, a
    /// bool as 0/1. The state machine picks its state from them each frame.
    pub fn set_anim_param(&mut self, node: Node, name: &str, value: f32) -> Result<(), Error> {
        self.run(
            method!("state_machine.set"),
            &[node.to_value(), name.to_value(), value.to_value()],
        )
    }

    /// Fires a trigger of the animation state machine (used up by the transition it lets
    /// through: a jump, an attack).
    pub fn fire_anim_trigger(&mut self, node: Node, name: &str) -> Result<(), Error> {
        self.set_anim_param(node, name, 1.0)
    }

    /// A parameter's value (0 when there is none).
    pub fn anim_param(&self, node: Node, name: &str) -> f32 {
        self.query::<f32>(
            method!("state_machine.get"),
            &[node.to_value(), name.to_value()],
        )
        .unwrap_or(0.0)
    }

    /// The state the animation state machine is in (empty when none).
    pub fn anim_state(&self, node: Node) -> String {
        self.query::<String>(method!("state_machine.state"), &[node.to_value()])
            .unwrap_or_default()
    }

    /// Turns the node's IK chains on or off (off: the animation only).
    pub fn set_ik_enabled(&mut self, node: Node, enabled: bool) -> Result<(), Error> {
        self.run(
            method!("ik.enabled"),
            &[node.to_value(), enabled.to_value()],
        )
    }

    /// Draws every IK chain, its target and pole (debugging).
    pub fn set_ik_debug(&mut self, on: bool) -> Result<(), Error> {
        self.run(method!("ik.debug"), &[on.to_value()])
    }

    /// The length of a `Path3D` (metres, in the world).
    pub fn path_length(&self, path: Node) -> Result<f32, Error> {
        self.fetch::<f64>(method!("path.length"), &[path.to_value()])?
            .map(|v| v as f32)
            .ok_or_else(|| Error::new(ErrorKind::Failed, "not a path"))
    }

    /// Where a `Path3D` is `distance` metres from its start, and which way it goes there
    /// (world space): move a train, a camera or a patrol along it. A closed path wraps.
    pub fn path_point(&self, path: Node, distance: f32) -> Result<(Vec3, Vec3), Error> {
        let values = self.values(
            method!("path.point"),
            &[path.to_value(), distance.to_value()],
        )?;
        match values.as_slice() {
            [p, d] => match (p.get::<Vec3>(), d.get::<Vec3>()) {
                (Some(p), Some(d)) => Ok((p, d)),
                _ => Err(Error::new(ErrorKind::Failed, "unexpected path values")),
            },
            _ => Err(Error::new(ErrorKind::Failed, "not a path")),
        }
    }

    /// A number from the game's config (`config.toml`): `tree.config_f32("Player.MoveSpeed")`.
    /// `Err` names the problem: an unknown key (with the closest one), a value of another
    /// type, or a game without a config.
    pub fn config_f32(&self, key: &str) -> Result<f32, Error> {
        self.config_number(key).map(|v| v as f32)
    }

    /// A number from the game's config, at full precision.
    pub fn config_number(&self, key: &str) -> Result<f64, Error> {
        self.fetch(method!("config.number"), &[key.to_value()])?
            .ok_or_else(|| Error::new(ErrorKind::Failed, format!("no number `{key}`")))
    }

    /// An integer from the game's config (`Player.MaxHealth`).
    pub fn config_int(&self, key: &str) -> Result<i64, Error> {
        self.fetch(method!("config.int"), &[key.to_value()])?
            .ok_or_else(|| Error::new(ErrorKind::Failed, format!("no integer `{key}`")))
    }

    pub fn config_bool(&self, key: &str) -> Result<bool, Error> {
        self.fetch(method!("config.bool"), &[key.to_value()])?
            .ok_or_else(|| Error::new(ErrorKind::Failed, format!("no bool `{key}`")))
    }

    pub fn config_text(&self, key: &str) -> Result<String, Error> {
        self.fetch(method!("config.text"), &[key.to_value()])?
            .ok_or_else(|| Error::new(ErrorKind::Failed, format!("no text `{key}`")))
    }

    /// Changes a setting of the game's config. Refused (with the reason) for constants, a
    /// value of another type, one outside the setting's range or options, or an unknown key.
    pub fn set_config(&mut self, key: &str, value: impl IntoValue) -> Result<(), Error> {
        self.run(method!("config.set"), &[key.to_value(), value.to_value()])
    }

    /// The text of translation `key` in the game's language (`[localization]` in
    /// `game.toml`; missing texts come from the first language). The key itself when no
    /// language has it, or the game has no languages.
    pub fn text(&self, key: &str) -> String {
        self.query::<String>(method!("locale.text"), &[key.to_value()])
            .unwrap_or_else(|| key.to_owned())
    }

    /// [`Tree::text`] with `{name}` placeholders filled:
    /// `tree.text_with("hud.coins", &[("count", &coins)])`.
    pub fn text_with(&self, key: &str, args: &[(&str, &dyn core::fmt::Display)]) -> String {
        let mut text = self.text(key);
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), &value.to_string());
        }
        text
    }

    /// The game's current language code (`"en"`), empty when it has none.
    pub fn language(&self) -> String {
        self.query::<String>(method!("locale.language"), &[])
            .unwrap_or_default()
    }

    /// The codes of the game's languages, in `game.toml`'s order.
    pub fn languages(&self) -> Vec<String> {
        self.query::<String>(method!("locale.languages"), &[])
            .map(|list| {
                list.split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Switches the game's language; labels and buttons with a key change at once.
    pub fn set_language(&mut self, code: &str) -> Result<(), Error> {
        self.run(method!("locale.set_language"), &[code.to_value()])
    }

    /// A text asset (`"dialogue/intro.txt"`), from the asset folder or the game's pack.
    pub fn read_text(&self, path: &str) -> Result<String, Error> {
        self.fetch(method!("assets.read_text"), &[path.to_value()])?
            .ok_or_else(|| Error::new(ErrorKind::Failed, format!("cannot read {path}")))
    }

    /// The player's saves (in the game's data folder).
    pub fn saves(&mut self) -> Saves<'_, 'a> {
        Saves { tree: self }
    }

    /// Rendering statistics of the last frame.
    pub fn render_stats(&self) -> RenderStats {
        let values = self
            .values(method!("render.stats"), &[])
            .unwrap_or_default();
        let at = |i: usize| values.get(i).and_then(Arg::get::<u32>).unwrap_or(0);
        RenderStats {
            draw_calls: at(0),
            objects: at(1),
            culled: at(2),
            shadow_draw_calls: at(3),
            lights: at(4),
        }
    }
}

/// Logs a failed query once per call site.
pub(crate) fn report(method: &MethodCell, err: &Error) {
    if err.kind() != ErrorKind::NoNode && method.first_warning() {
        crate::host::log(
            ffi::LogLevel::WARN,
            "ab_script",
            format_args!("`{}`: {err}", method.name()),
        );
    }
}

/// How far a scene preload has got ([`Tree::preload_progress`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreloadProgress {
    /// Assets loaded so far.
    pub loaded: u32,
    /// Assets the scene uses (0 while its file is being read).
    pub total: u32,
    /// Everything is loaded (or the scene could not be read: see `error`).
    pub ready: bool,
    pub error: Option<String>,
}

impl PreloadProgress {
    /// 0..=1, for a progress bar.
    pub fn fraction(&self) -> f32 {
        match (self.ready, self.total) {
            (true, _) => 1.0,
            (false, 0) => 0.0,
            (false, total) => self.loaded as f32 / total as f32,
        }
    }
}

/// Rendering statistics ([`Tree::render_stats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub draw_calls: u32,
    pub objects: u32,
    /// Objects skipped as outside the view.
    pub culled: u32,
    pub shadow_draw_calls: u32,
    pub lights: u32,
}

/// Actions of the input map ([`Tree::input`]).
#[derive(Debug)]
pub struct Input<'t, 'a> {
    tree: &'t Tree<'a>,
}

impl Input<'_, '_> {
    pub fn is_pressed(&self, action: &str) -> bool {
        self.tree
            .query(method!("input.is_pressed"), &[action.to_value()])
            .unwrap_or(false)
    }

    /// Pressed this frame.
    pub fn is_just_pressed(&self, action: &str) -> bool {
        self.tree
            .query(method!("input.is_just_pressed"), &[action.to_value()])
            .unwrap_or(false)
    }

    /// Released this frame.
    pub fn is_just_released(&self, action: &str) -> bool {
        self.tree
            .query(method!("input.is_just_released"), &[action.to_value()])
            .unwrap_or(false)
    }

    /// 0..=1 (analog triggers and sticks; 1 for pressed keys).
    pub fn strength(&self, action: &str) -> f32 {
        self.tree
            .query(method!("input.strength"), &[action.to_value()])
            .unwrap_or(0.0)
    }

    /// `positive` minus `negative`: -1..=1.
    pub fn axis(&self, negative: &str, positive: &str) -> f32 {
        self.tree
            .query(
                method!("input.axis"),
                &[negative.to_value(), positive.to_value()],
            )
            .unwrap_or(0.0)
    }

    /// A 2D direction from four actions, at most length 1 (x right, y up).
    pub fn vector(&self, left: &str, right: &str, down: &str, up: &str) -> Vec2 {
        self.tree
            .query(
                method!("input.vector"),
                &[
                    left.to_value(),
                    right.to_value(),
                    down.to_value(),
                    up.to_value(),
                ],
            )
            .unwrap_or(Vec2::ZERO)
    }

    /// Mouse movement this frame (+Y down), for looking around.
    pub fn look(&self) -> Vec2 {
        self.tree
            .query(method!("input.look"), &[])
            .unwrap_or(Vec2::ZERO)
    }

    /// Whether the input map has `action`.
    pub fn has_action(&self, action: &str) -> bool {
        self.tree
            .query(method!("input.has_action"), &[action.to_value()])
            .unwrap_or(false)
    }
}

/// The running game ([`Tree::engine`]).
#[derive(Debug)]
pub struct Engine<'t, 'a> {
    tree: &'t mut Tree<'a>,
}

impl Engine<'_, '_> {
    /// Ends the game at the end of the frame.
    pub fn quit(&mut self) {
        let _ = self.tree.run(method!("engine.quit"), &[]);
    }

    pub fn is_mouse_captured(&self) -> bool {
        self.tree
            .query(method!("engine.is_mouse_captured"), &[])
            .unwrap_or(false)
    }

    /// Hides the cursor and keeps it in the window, for mouse look.
    pub fn set_mouse_captured(&mut self, captured: bool) {
        let _ = self
            .tree
            .run(method!("engine.set_mouse_captured"), &[captured.to_value()]);
    }

    pub fn is_fullscreen(&self) -> bool {
        self.tree
            .query(method!("engine.is_fullscreen"), &[])
            .unwrap_or(false)
    }

    pub fn set_fullscreen(&mut self, fullscreen: bool) {
        let _ = self
            .tree
            .run(method!("engine.set_fullscreen"), &[fullscreen.to_value()]);
    }
}

/// Save slots ([`Tree::saves`]): text per slot name, in the game's data folder.
#[derive(Debug)]
pub struct Saves<'t, 'a> {
    tree: &'t mut Tree<'a>,
}

impl Saves<'_, '_> {
    pub fn write(&mut self, slot: &str, text: &str) -> Result<(), Error> {
        self.tree
            .run(method!("save.write"), &[slot.to_value(), text.to_value()])
    }

    /// The slot's text, `Ok(None)` if it was never saved.
    pub fn read(&mut self, slot: &str) -> Result<Option<String>, Error> {
        self.tree.fetch(method!("save.read"), &[slot.to_value()])
    }

    pub fn exists(&mut self, slot: &str) -> bool {
        self.tree
            .query(method!("save.exists"), &[slot.to_value()])
            .unwrap_or(false)
    }

    pub fn delete(&mut self, slot: &str) -> Result<(), Error> {
        self.tree.run(method!("save.delete"), &[slot.to_value()])
    }

    /// Saves `value` as JSON.
    pub fn save<T: Serialize>(&mut self, slot: &str, value: &T) -> Result<(), Error> {
        let text = serde_json::to_string(value)
            .map_err(|err| Error::new(ErrorKind::Failed, err.to_string()))?;
        self.tree.run(
            method!("save.write_json"),
            &[slot.to_value(), text.to_value()],
        )
    }

    /// Loads JSON saved with [`Saves::save`]; `Ok(None)` if the slot is empty.
    pub fn load<T: DeserializeOwned>(&mut self, slot: &str) -> Result<Option<T>, Error> {
        let json: Option<String> = self
            .tree
            .fetch(method!("save.read_json"), &[slot.to_value()])?;
        match json {
            None => Ok(None),
            Some(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|err| Error::new(ErrorKind::Failed, format!("save `{slot}`: {err}"))),
        }
    }
}
