use ab_ecs::Entity;
use std::fmt;

/// User-facing reference to a node.
///
/// Handles are generational: once a node is freed, every handle to it becomes invalid (see
/// [`SceneTree::is_valid`](crate::SceneTree::is_valid)) even if its slot is reused, so a stale
/// handle can never reach another node.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeHandle(Entity);

impl NodeHandle {
    pub const fn from_entity(entity: Entity) -> Self {
        Self(entity)
    }

    pub const fn entity(self) -> Entity {
        self.0
    }

    /// Packs the handle into a `u64` (save files, FFI, network-free IDs).
    pub const fn to_bits(self) -> u64 {
        self.0.to_bits()
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self(Entity::from_bits(bits))
    }
}

impl From<NodeHandle> for Entity {
    fn from(handle: NodeHandle) -> Self {
        handle.0
    }
}

impl fmt::Debug for NodeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Node({:?})", self.0)
    }
}

impl fmt::Display for NodeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl serde::Serialize for NodeHandle {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.to_bits())
    }
}

impl<'de> serde::Deserialize<'de> for NodeHandle {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        u64::deserialize(deserializer).map(Self::from_bits)
    }
}
