use crate::handle::NodeHandle;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SceneError {
    #[error("{0} is not a valid node (freed or never created)")]
    InvalidNode(NodeHandle),
    #[error("{child} already has a parent ({parent}); remove it first or use reparent")]
    AlreadyHasParent {
        child: NodeHandle,
        parent: NodeHandle,
    },
    #[error("adding {child} under {parent} would create a cycle")]
    WouldCreateCycle {
        parent: NodeHandle,
        child: NodeHandle,
    },
    #[error("{child} is not a child of {parent}")]
    NotAChild {
        parent: NodeHandle,
        child: NodeHandle,
    },
    #[error("the root node cannot be moved or freed")]
    RootNode,
    #[error("{0} is not a Node3D (it has no transform)")]
    NotNode3D(NodeHandle),
    #[error("{0}: a NaN or infinite position, rotation or scale was refused")]
    NonFiniteTransform(NodeHandle),
    #[error("no script is registered as `{0}`")]
    UnknownScript(String),
}
