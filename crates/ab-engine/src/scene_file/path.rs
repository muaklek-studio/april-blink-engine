//! `Path3D` nodes: a spline and what it builds (see [`crate::spline`]).

use super::format::{MaterialDesc, NodeDesc, PathDesc, SceneFile};
use super::{instantiate_scene, material_handle, Generated, SceneOrigin};
use crate::spline::{self, Spline};
use ab_core::{Transform3D, Vec3};
use ab_physics::{BodyKind, PhysicsBody3D, PhysicsNodes as _, Shape};
use ab_rendering::{MeshInstance3D, RenderAssets, RenderNodes as _};
use ab_scene::{NodeHandle, SceneTree};

/// The class of path nodes.
pub const PATH_3D: &str = "Path3D";

/// A path node's description and its curve.
#[derive(Debug, Clone, PartialEq)]
pub struct Path3D {
    pub desc: PathDesc,
    pub spline: Spline,
}

impl Path3D {
    pub fn new(desc: PathDesc) -> Self {
        let points = desc.points.iter().map(|&p| Vec3::from(p)).collect();
        let spline = Spline::new(points, desc.closed.unwrap_or(false));
        Self { desc, spline }
    }
}

/// What a path builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    None,
    Ribbon,
    Tube,
    Instances,
}

impl PathMode {
    pub const ALL: [PathMode; 4] = [Self::None, Self::Ribbon, Self::Tube, Self::Instances];

    pub fn of(desc: &PathDesc) -> Result<Self, String> {
        Ok(match desc.mode.as_deref().unwrap_or("none") {
            "none" => Self::None,
            "ribbon" => Self::Ribbon,
            "tube" => Self::Tube,
            "instances" => Self::Instances,
            other => {
                return Err(format!(
                    "unknown path mode `{other}` (none, ribbon, tube, instances)"
                ))
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ribbon => "ribbon",
            Self::Tube => "tube",
            Self::Instances => "instances",
        }
    }
}

/// Creates a `Path3D` node from `desc` and builds along it.
pub(super) fn create(tree: &mut SceneTree, name: &str, desc: &NodeDesc) -> Result<NodeHandle, String> {
    let path = desc.path.clone().unwrap_or_default();
    PathMode::of(&path)?;
    let node = tree.create_node3d(name);
    tree.set_class_name(node, PATH_3D).map_err(|e| e.to_string())?;
    let material = desc.material.clone();
    tree.insert_component(node, Path3D::new(path))
        .map_err(|e| e.to_string())?;
    tree.insert_component(
        node,
        SceneOrigin {
            material: material.clone(),
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    build(tree, node)?;
    Ok(node)
}

/// Replaces what `node` (a path) built with what its description builds now.
pub fn build(tree: &mut SceneTree, node: NodeHandle) -> Result<(), String> {
    for child in tree.children(node) {
        if tree.component::<Generated>(child).is_some() {
            let _ = tree.queue_free(child);
        }
    }
    tree.flush_deletions();
    let Some(path) = tree.component::<Path3D>(node).map(|p| p.clone()) else {
        return Ok(());
    };
    let material = tree
        .component::<SceneOrigin>(node)
        .and_then(|o| o.material.clone())
        .unwrap_or_default();
    let mode = PathMode::of(&path.desc)?;
    let d = &path.desc;
    let mesh = match mode {
        PathMode::None => return Ok(()),
        PathMode::Ribbon => spline::ribbon(&path.spline, d.width.unwrap_or(3.0).max(0.01)),
        PathMode::Tube => spline::tube(
            &path.spline,
            d.radius.unwrap_or(0.15).max(0.001),
            d.sides.unwrap_or(10) as usize,
            d.sag.unwrap_or(0.0),
        ),
        PathMode::Instances => {
            return build_instances(tree, node, &path);
        }
    };
    let Some(mut mesh) = mesh else {
        return Ok(()); // fewer than two points: nothing to build yet
    };
    let collide = d.collide.unwrap_or(false);
    let shape = collide.then(|| {
        let vertices: Vec<Vec3> = mesh.vertices.iter().map(|v| Vec3::from(v.position)).collect();
        let indices: Vec<[u32; 3]> = mesh.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
        Shape::trimesh(vertices, indices)
    });
    mesh.compute_tangents();
    let mesh = tree
        .world()
        .resource_mut::<RenderAssets>()
        .ok_or("the RenderAssets resource is missing")?
        .add_mesh(mesh)
        .map_err(|e| e.to_string())?;
    let material = material_handle(tree, &material)?;
    let built = match shape {
        Some(shape) => {
            let body = tree.create_physics_body3d("Built", PhysicsBody3D::new(BodyKind::Static));
            let collider = tree.create_collision_shape3d("Shape", shape);
            tree.add_child(body, collider).map_err(|e| e.to_string())?;
            body
        }
        None => tree.create_node3d("Built"),
    };
    let visual = tree.create_mesh_instance3d("Mesh", MeshInstance3D::new(mesh, material));
    tree.add_child(built, visual).map_err(|e| e.to_string())?;
    tree.insert_component(built, Generated).map_err(|e| e.to_string())?;
    tree.add_child(node, built).map_err(|e| e.to_string())?;
    Ok(())
}

fn build_instances(tree: &mut SceneTree, node: NodeHandle, path: &Path3D) -> Result<(), String> {
    let d = &path.desc;
    let Some(piece) = d.piece.clone().filter(|p| !p.trim().is_empty()) else {
        return Ok(()); // no piece chosen yet
    };
    let places = spline::instance_places(&path.spline, d.spacing.unwrap_or(2.0), d.align.unwrap_or(true));
    let built = tree.create_node3d("Built");
    tree.insert_component(built, Generated).map_err(|e| e.to_string())?;
    tree.add_child(node, built).map_err(|e| e.to_string())?;
    // One description of the piece, instanced at each place.
    let scene = SceneFile {
        environment: None,
        nodes: vec![NodeDesc {
            name: "Piece".into(),
            instance: Some(piece),
            ..Default::default()
        }],
    };
    for (i, (position, rotation)) in places.into_iter().enumerate() {
        let copy = instantiate_scene(tree, &scene, "path").map_err(|e| e.to_string())?;
        let _ = tree.set_name(copy, &format!("Piece{i}"));
        let _ = tree.set_transform(copy, Transform3D::from_trs(position, rotation, Vec3::ONE));
        tree.add_child(built, copy).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The path's description, for saving.
pub(super) fn describe(tree: &SceneTree, node: NodeHandle, desc: &mut NodeDesc) {
    desc.path = tree.component::<Path3D>(node).map(|p| p.desc.clone());
    desc.material = tree
        .component::<SceneOrigin>(node)
        .and_then(|o| o.material.clone())
        .filter(|m| *m != MaterialDesc::default());
}
