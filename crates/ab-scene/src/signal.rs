//! Godot-style signals: a node emits a named event, connected handlers react.

use crate::handle::NodeHandle;
use crate::scene_tree::SceneTree;
use ab_ecs::Entity;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tracing::{error, warn};

/// An emitted signal: who sent it, its name and an optional argument.
pub struct Signal {
    pub source: NodeHandle,
    pub name: &'static str,
    arg: Option<Box<dyn Any>>,
}

impl Signal {
    pub fn new(source: NodeHandle, name: &'static str) -> Self {
        Self {
            source,
            name,
            arg: None,
        }
    }

    /// Attaches an argument, read back with [`Signal::arg`].
    pub fn with_arg<T: Any>(mut self, arg: T) -> Self {
        self.arg = Some(Box::new(arg));
        self
    }

    /// Attaches an already boxed argument (a script library's argument).
    pub fn with_arg_boxed(mut self, arg: Box<dyn Any>) -> Self {
        self.arg = Some(arg);
        self
    }

    /// The argument, if there is one of type `T`.
    pub fn arg<T: Any>(&self) -> Option<&T> {
        self.arg.as_deref()?.downcast_ref()
    }

    pub fn is(&self, name: &str) -> bool {
        self.name == name
    }
}

impl fmt::Debug for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signal")
            .field("source", &self.source)
            .field("name", &self.name)
            .field("has_arg", &self.arg.is_some())
            .finish()
    }
}

/// ECS resource where engine systems (UI, physics, audio, timers) queue signals. The tree
/// delivers them at the start of `physics_process` and `process`.
#[derive(Debug, Default)]
pub struct SignalQueue {
    signals: Vec<Signal>,
}

impl SignalQueue {
    pub fn push(&mut self, signal: Signal) {
        self.signals.push(signal);
    }

    pub fn len(&self) -> usize {
        self.signals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }

    pub(crate) fn take(&mut self) -> Vec<Signal> {
        std::mem::take(&mut self.signals)
    }
}

/// Identifies a connection, for [`SceneTree::disconnect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// The id as a number (for script libraries, which cannot hold engine types).
    pub fn to_bits(self) -> u64 {
        self.0
    }

    pub fn from_bits(bits: u64) -> Self {
        Self(bits)
    }
}

/// Returns whether to stay connected.
type Callback = Box<dyn FnMut(&mut SceneTree, &Signal) -> bool>;

/// Wraps a handler so a panic is caught (in the caller's `std`, see `Guarded`) and the
/// connection dropped.
fn guard(mut handler: impl FnMut(&mut SceneTree, &Signal) + 'static) -> Callback {
    Box::new(
        move |tree, signal| match catch_unwind(AssertUnwindSafe(|| handler(tree, signal))) {
            Ok(()) => true,
            Err(payload) => {
                error!(
                    signal = signal.name,
                    source = %signal.source,
                    "signal handler panicked: {}; disconnected",
                    crate::script::panic_message(payload.as_ref())
                );
                false
            }
        },
    )
}

enum Handler {
    /// Delivered to the target node's script ([`NodeScript::on_signal`](crate::NodeScript)).
    Script(NodeHandle),
    Callback(Callback),
}

struct Connection {
    id: ConnectionId,
    /// `None` while the handler runs.
    handler: Option<Handler>,
    one_shot: bool,
}

/// Connections of a tree, keyed by (source, signal name).
#[derive(Default)]
pub(crate) struct Signals {
    next_id: u64,
    connections: HashMap<(Entity, &'static str), Vec<Connection>>,
    depth: u32,
}

impl Signals {
    fn add(
        &mut self,
        source: NodeHandle,
        name: &'static str,
        handler: Handler,
        one_shot: bool,
    ) -> ConnectionId {
        self.next_id += 1;
        let id = ConnectionId(self.next_id);
        self.connections
            .entry((source.entity(), name))
            .or_default()
            .push(Connection {
                id,
                handler: Some(handler),
                one_shot,
            });
        id
    }

    /// Drops connections from or to dead entities.
    pub(crate) fn retain_alive(&mut self, alive: impl Fn(Entity) -> bool) {
        self.connections.retain(|(source, _), list| {
            list.retain(|c| match &c.handler {
                Some(Handler::Script(target)) => alive(target.entity()),
                _ => true,
            });
            alive(*source) && !list.is_empty()
        });
    }

    pub(crate) fn len(&self) -> usize {
        self.connections.values().map(Vec::len).sum()
    }
}

/// Nested emissions deeper than this are dropped (a handler re-emitting its own signal).
const MAX_DEPTH: u32 = 32;

impl SceneTree {
    /// Connects `source`'s signal `name` to `target`'s script: it receives
    /// [`NodeScript::on_signal`](crate::NodeScript::on_signal). The connection goes away when
    /// either node is freed, and keeps working across hot reloads of the target's script.
    pub fn connect(
        &mut self,
        source: NodeHandle,
        name: &'static str,
        target: NodeHandle,
    ) -> ConnectionId {
        self.signals
            .add(source, name, Handler::Script(target), false)
    }

    /// Connects a closure. Prefer [`SceneTree::connect`] in hot-reloadable code: a closure
    /// keeps running the code it was created with. A panicking closure is logged and
    /// disconnected.
    pub fn connect_fn(
        &mut self,
        source: NodeHandle,
        name: &'static str,
        handler: impl FnMut(&mut SceneTree, &Signal) + 'static,
    ) -> ConnectionId {
        self.signals
            .add(source, name, Handler::Callback(guard(handler)), false)
    }

    /// Like [`SceneTree::connect_fn`], disconnected after the first call.
    pub fn connect_fn_once(
        &mut self,
        source: NodeHandle,
        name: &'static str,
        handler: impl FnMut(&mut SceneTree, &Signal) + 'static,
    ) -> ConnectionId {
        self.signals
            .add(source, name, Handler::Callback(guard(handler)), true)
    }

    /// Removes a connection. Returns whether it existed.
    pub fn disconnect(&mut self, id: ConnectionId) -> bool {
        let mut found = false;
        self.signals.connections.retain(|_, list| {
            let before = list.len();
            list.retain(|c| c.id != id);
            found |= list.len() != before;
            !list.is_empty()
        });
        found
    }

    /// Whether `source`'s signal `name` is connected to `target`'s script.
    pub fn is_connected(&self, source: NodeHandle, name: &'static str, target: NodeHandle) -> bool {
        self.signals
            .connections
            .get(&(source.entity(), name))
            .is_some_and(|list| {
                list.iter()
                    .any(|c| matches!(c.handler, Some(Handler::Script(t)) if t == target))
            })
    }

    /// Number of live connections in the tree.
    pub fn connection_count(&self) -> usize {
        self.signals.len()
    }

    /// Emits a signal now: every connected handler runs before this returns, in connection
    /// order. Handlers may connect, disconnect, emit and free nodes. A handler is never
    /// re-entered by an emission from inside itself, and chains of emissions deeper than 32
    /// are cut off.
    pub fn emit(&mut self, signal: Signal) {
        let key = (signal.source.entity(), signal.name);
        let Some(ids) = self
            .signals
            .connections
            .get(&key)
            .map(|list| list.iter().map(|c| c.id).collect::<Vec<_>>())
        else {
            return;
        };
        if self.signals.depth >= MAX_DEPTH {
            warn!(
                signal = signal.name,
                "signal recursion too deep; emission dropped"
            );
            return;
        }
        self.signals.depth += 1;
        for id in ids {
            let taken = self.signals.connections.get_mut(&key).and_then(|list| {
                let connection = list.iter_mut().find(|c| c.id == id)?;
                Some((connection.handler.take()?, connection.one_shot))
            });
            let Some((mut handler, one_shot)) = taken else {
                continue; // disconnected meanwhile, or running higher up the stack
            };
            let keep = match &mut handler {
                Handler::Script(target) => {
                    let target = *target;
                    if self.is_valid(target) {
                        // Every script of the target node hears it.
                        self.with_scripts(target, |script, tree| {
                            script.on_signal(tree, target, &signal)
                        });
                        true
                    } else {
                        false
                    }
                }
                Handler::Callback(callback) => callback(self, &signal),
            };
            // Put the handler back unless it was disconnected while running.
            let list = self.signals.connections.get_mut(&key);
            if let Some(list) = list {
                if keep && !one_shot {
                    if let Some(connection) = list.iter_mut().find(|c| c.id == id) {
                        connection.handler = Some(handler);
                    }
                } else {
                    list.retain(|c| c.id != id);
                }
            }
        }
        self.signals.depth -= 1;
        if self
            .signals
            .connections
            .get(&key)
            .is_some_and(|list| list.is_empty())
        {
            self.signals.connections.remove(&key);
        }
    }

    /// Delivers the signals queued in the [`SignalQueue`] resource (by engine systems).
    pub fn flush_signals(&mut self) {
        // Handlers that queue more signals are served too, within reason.
        for _ in 0..16 {
            let queued = self
                .world
                .resource_mut::<SignalQueue>()
                .map(|mut q| q.take())
                .unwrap_or_default();
            if queued.is_empty() {
                break;
            }
            for signal in queued {
                self.emit(signal);
            }
        }
    }
}
