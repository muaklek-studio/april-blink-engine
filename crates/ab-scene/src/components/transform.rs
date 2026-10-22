use ab_core::{Affine3A, Transform3D};

/// Spatial state of a `Node3D`.
///
/// `local` is relative to the parent (when the parent is also a `Node3D`; a plain `Node`
/// parent breaks inheritance, as in Godot). `global` is a cache refreshed by
/// [`propagate_transforms`](crate::propagate_transforms); `dirty` marks that `local` (or the
/// node's place in the hierarchy) changed since the last propagation.
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    pub local: Transform3D,
    pub global: Affine3A,
    pub dirty: bool,
}

impl Transform {
    pub fn new(local: Transform3D) -> Self {
        Self {
            local,
            global: local.to_affine(),
            dirty: true,
        }
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::new(Transform3D::IDENTITY)
    }
}
