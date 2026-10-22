use crate::handle::NodeHandle;

/// Parent link. Present only on nodes that have a parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(pub NodeHandle);

/// Ordered child list. Present on every node (empty lists do not allocate).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Children(pub Vec<NodeHandle>);
