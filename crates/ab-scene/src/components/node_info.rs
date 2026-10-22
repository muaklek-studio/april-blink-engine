/// Per-node metadata. Present on every node.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// Unique among siblings.
    pub name: String,
    /// Built-in or script class name (`"Node"`, `"Node3D"`, `"Camera3D"`, ...).
    pub class_name: &'static str,
    /// Own visibility flag; see `SceneTree::is_visible_in_tree` for the inherited value.
    pub visible: bool,
    pub process_enabled: bool,
    pub physics_process_enabled: bool,
    /// Whether the node is currently attached under the tree root.
    pub in_tree: bool,
    /// Whether `ready` has been dispatched (it runs once per node).
    pub ready_called: bool,
    pub queued_for_deletion: bool,
}

impl NodeInfo {
    pub fn new(name: impl Into<String>, class_name: &'static str) -> Self {
        Self {
            name: name.into(),
            class_name,
            visible: true,
            process_enabled: true,
            physics_process_enabled: true,
            in_tree: false,
            ready_called: false,
            queued_for_deletion: false,
        }
    }
}
