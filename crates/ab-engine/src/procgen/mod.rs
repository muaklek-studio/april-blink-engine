//! Procedural generation: one framework for every generator.
//!
//! A `Generator` node names a generator and its parameters; what the generator makes is
//! built under the node when the scene loads (and rebuilt when the parameters change in the
//! editor), never saved: the same seed and parameters make the same result on every
//! machine. Generators share what they need:
//!
//! * [`Rng`]: seeded random numbers, with independent streams per purpose,
//! * [`Noise`]: smooth gradient noise and its fractal forms,
//! * [`Context`]: the parameters read with defaults and ranges, the ground under a point
//!   (other meshes of the scene, such as a generated terrain), other nodes by path,
//! * [`Output`]: scene nodes (pieces, prefabs, primitives, even other generators and
//!   paths) or meshes (terrain), built the same way for all.
//!
//! The generators: `scatter` (objects over an area or along a path), `terrain`
//! (mountains and hills), `dungeon` (rooms and corridors), `building` (floors, walls,
//! windows, a roof) and `town` (a grid of roads lined with buildings). A new one implements
//! [`Generator`] and joins [`GENERATORS`].
//!
//! ```toml
//! [[node]]
//! name = "Forest"
//! type = "Generator"
//! generator = { kind = "scatter", seed = 7, pieces = ["models/tree.glb"], count = 80, width = 60.0, depth = 60.0, min_distance = 3.0, ground = true }
//! ```

mod generators;
mod noise;
mod rng;

pub use generators::GENERATORS;
pub use noise::Noise;
pub use rng::Rng;

use crate::scene_file::{MaterialDesc, SceneFile};
use ab_core::{Affine3A, Transform3D, Vec3};
use ab_rendering::{Mesh, MeshInstance3D, RenderAssets};
use ab_scene::{NodeHandle, SceneTree};
use std::cell::OnceCell;

/// What a generator makes.
#[derive(Debug, Clone)]
pub enum Output {
    /// Nodes (the first is their root), placed at `at` in the generator's space.
    Scene { scene: SceneFile, at: Transform3D },
    /// A mesh in the generator's space (terrain), with collision if asked.
    Mesh {
        name: String,
        mesh: Mesh,
        material: MaterialDesc,
        collide: bool,
    },
}

impl Output {
    /// One node.
    pub fn node(desc: crate::scene_file::NodeDesc, at: Transform3D) -> Self {
        Output::Scene {
            scene: SceneFile {
                environment: None,
                nodes: vec![desc],
            },
            at,
        }
    }
}

/// What a parameter holds, for reading it and for the editor's fields.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    Int { min: i64, max: i64 },
    Float { min: f32, max: f32 },
    Bool,
    Text,
    /// A colour (`"#rrggbb"`).
    Color,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// A model or prefab (asset path); empty for none.
    Piece,
    /// Models and prefabs (a list of asset paths).
    Pieces,
    /// Another node, by path from the generator (`"../Road"`).
    Node,
}

/// One parameter of a generator.
#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub default: ParamDefault,
    pub help: &'static str,
}

/// A parameter's value when the node does not set it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamDefault {
    Int(i64),
    Float(f32),
    Bool(bool),
    Text(&'static str),
    Empty,
}

impl ParamDefault {
    pub fn to_toml(self) -> toml::Value {
        match self {
            Self::Int(v) => toml::Value::Integer(v),
            Self::Float(v) => toml::Value::Float(f64::from(v)),
            Self::Bool(v) => toml::Value::Boolean(v),
            Self::Text(v) => toml::Value::String(v.into()),
            Self::Empty => toml::Value::Array(Vec::new()),
        }
    }
}

/// A generator: its parameters and what it makes of them.
pub trait Generator: Sync {
    /// Its `kind` in scene files.
    fn kind(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn params(&self) -> &'static [Param];
    /// Generators that read the ground under them build after the others: 1, else 0.
    fn stage(&self) -> u8 {
        0
    }
    fn generate(&self, cx: &mut Context<'_>) -> Result<Vec<Output>, String>;
}

/// The generator of `kind`.
pub fn generator(kind: &str) -> Option<&'static dyn Generator> {
    GENERATORS.iter().copied().find(|g| g.kind() == kind)
}

/// A generator's view of the scene and its parameters.
pub struct Context<'a> {
    pub tree: &'a SceneTree,
    /// The generator node.
    pub node: NodeHandle,
    pub rng: Rng,
    params: &'a toml::Table,
    schema: &'static [Param],
    /// The scene's meshes to stand on: world transform, mesh, and world bounds.
    ground: OnceCell<Vec<(Affine3A, ab_rendering::MeshHandle, ab_rendering::Aabb)>>,
}

impl std::fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context").field("node", &self.node).finish()
    }
}

/// `node`'s world transform from its ancestors' local ones (it may not be in the main
/// tree yet, while a scene loads).
pub fn world_of(tree: &SceneTree, node: NodeHandle) -> Affine3A {
    let mut chain = Vec::new();
    let mut current = Some(node);
    while let Some(n) = current {
        if let Some(t) = tree.transform(n) {
            chain.push(t.to_affine());
        }
        current = tree.parent(n);
    }
    chain.iter().rev().fold(Affine3A::IDENTITY, |acc, t| acc * *t)
}

fn top_of(tree: &SceneTree, node: NodeHandle) -> NodeHandle {
    let mut top = node;
    while let Some(parent) = tree.parent(top) {
        top = parent;
    }
    top
}

impl<'a> Context<'a> {
    pub fn new(
        tree: &'a SceneTree,
        node: NodeHandle,
        seed: u64,
        params: &'a toml::Table,
        schema: &'static [Param],
    ) -> Self {
        Self {
            tree,
            node,
            rng: Rng::new(seed),
            params,
            schema,
            ground: OnceCell::new(),
        }
    }

    fn param(&self, key: &str) -> Option<&'static Param> {
        self.schema.iter().find(|p| p.key == key)
    }

    /// A number parameter, clamped to its range.
    pub fn float(&self, key: &str) -> f32 {
        let param = self.param(key);
        let default = match param.map(|p| p.default) {
            Some(ParamDefault::Float(v)) => v,
            Some(ParamDefault::Int(v)) => v as f32,
            _ => 0.0,
        };
        let value = match self.params.get(key) {
            Some(toml::Value::Float(v)) => *v as f32,
            Some(toml::Value::Integer(v)) => *v as f32,
            _ => default,
        };
        match param.map(|p| p.kind) {
            Some(ParamKind::Float { min, max }) => value.clamp(min, max),
            _ => value,
        }
    }

    /// An integer parameter, clamped to its range.
    pub fn int(&self, key: &str) -> i64 {
        let param = self.param(key);
        let default = match param.map(|p| p.default) {
            Some(ParamDefault::Int(v)) => v,
            _ => 0,
        };
        let value = match self.params.get(key) {
            Some(toml::Value::Integer(v)) => *v,
            Some(toml::Value::Float(v)) => *v as i64,
            _ => default,
        };
        match param.map(|p| p.kind) {
            Some(ParamKind::Int { min, max }) => value.clamp(min, max),
            _ => value,
        }
    }

    pub fn bool(&self, key: &str) -> bool {
        match (self.params.get(key), self.param(key).map(|p| p.default)) {
            (Some(toml::Value::Boolean(v)), _) => *v,
            (_, Some(ParamDefault::Bool(v))) => v,
            _ => false,
        }
    }

    pub fn text(&self, key: &str) -> String {
        match (self.params.get(key), self.param(key).map(|p| p.default)) {
            (Some(toml::Value::String(v)), _) => v.clone(),
            (_, Some(ParamDefault::Text(v))) => v.to_owned(),
            _ => String::new(),
        }
    }

    /// A list of asset paths (or one path written alone).
    pub fn pieces(&self, key: &str) -> Vec<String> {
        match self.params.get(key) {
            Some(toml::Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .collect(),
            Some(toml::Value::String(one)) if !one.trim().is_empty() => vec![one.clone()],
            _ => Vec::new(),
        }
    }

    /// The node a `Node` parameter names, from the generator.
    pub fn node_param(&self, key: &str) -> Option<NodeHandle> {
        let path = self.text(key);
        if path.trim().is_empty() {
            return None;
        }
        self.tree.get_node(self.node, path.trim())
    }

    /// The generator's world transform.
    pub fn world(&self) -> Affine3A {
        world_of(self.tree, self.node)
    }

    /// The height (in the generator's space) of what is below `point` (generator space,
    /// its height ignored): the top of the scene's other meshes there.
    pub fn ground(&self, point: Vec3) -> Option<f32> {
        let world = self.world();
        let at = world.transform_point3(point);
        let meshes = self.ground.get_or_init(|| self.ground_meshes());
        let assets = self.tree.world().resource::<RenderAssets>()?;
        let mut best: Option<f32> = None;
        for (global, handle, bounds) in meshes {
            if at.x < bounds.min.x || at.x > bounds.max.x || at.z < bounds.min.z || at.z > bounds.max.z {
                continue;
            }
            let Some(mesh) = assets.mesh(*handle) else {
                continue;
            };
            let inverse = global.inverse();
            let origin = inverse.transform_point3(Vec3::new(at.x, bounds.max.y + 1.0, at.z));
            let dir = inverse.transform_vector3(Vec3::NEG_Y);
            for tri in mesh.indices.chunks_exact(3) {
                let p = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
                if let Some(t) = ray_triangle(origin, dir, p(tri[0]), p(tri[1]), p(tri[2])) {
                    let hit = global.transform_point3(origin + dir * t);
                    if best.is_none_or(|b| hit.y > b) {
                        best = Some(hit.y);
                    }
                }
            }
        }
        best.map(|y| world.inverse().transform_point3(Vec3::new(at.x, y, at.z)).y)
    }

    fn ground_meshes(&self) -> Vec<(Affine3A, ab_rendering::MeshHandle, ab_rendering::Aabb)> {
        let Some(assets) = self.tree.world().resource::<RenderAssets>() else {
            return Vec::new();
        };
        let top = top_of(self.tree, self.node);
        self.tree
            .subtree(top)
            .into_iter()
            .filter(|&n| n != self.node && !self.tree.is_ancestor_of(self.node, n))
            .filter_map(|n| {
                let mesh = self.tree.component::<MeshInstance3D>(n).map(|m| m.mesh)?;
                let global = world_of(self.tree, n);
                let bounds = assets.mesh_aabb(mesh)?.transformed(&global);
                Some((global, mesh, bounds))
            })
            .collect()
    }
}

/// Möller–Trumbore, both faces: distance along `dir`.
fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

/// Up to `count` points in a `width` × `depth` rectangle around the origin, none closer
/// than `min_distance` to another (dart throwing: dense but not on a grid).
pub fn scatter_points(rng: &mut Rng, count: usize, width: f32, depth: f32, min_distance: f32) -> Vec<(f32, f32)> {
    let mut points: Vec<(f32, f32)> = Vec::with_capacity(count);
    let tries = count * 30 + 10;
    let min2 = min_distance * min_distance;
    for _ in 0..tries {
        if points.len() >= count {
            break;
        }
        let p = (rng.range(-width / 2.0, width / 2.0), rng.range(-depth / 2.0, depth / 2.0));
        if points
            .iter()
            .all(|q| (q.0 - p.0).powi(2) + (q.1 - p.1).powi(2) >= min2)
        {
            points.push(p);
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scattered_points_keep_their_distance_and_repeat_by_seed() {
        let a = scatter_points(&mut Rng::new(5), 50, 40.0, 40.0, 3.0);
        let b = scatter_points(&mut Rng::new(5), 50, 40.0, 40.0, 3.0);
        assert_eq!(a, b);
        assert!(a.len() > 30, "{}", a.len());
        for (i, p) in a.iter().enumerate() {
            assert!(p.0.abs() <= 20.0 && p.1.abs() <= 20.0);
            for q in &a[i + 1..] {
                assert!((p.0 - q.0).hypot(p.1 - q.1) >= 3.0 - 1e-4);
            }
        }
    }

    #[test]
    fn rays_hit_triangles_from_either_side() {
        let (a, b, c) = (Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(ray_triangle(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y, a, b, c), Some(5.0));
        assert!(ray_triangle(Vec3::new(5.0, 5.0, 0.0), Vec3::NEG_Y, a, b, c).is_none());
        assert_eq!(ray_triangle(Vec3::new(0.0, -2.0, 0.0), Vec3::Y, a, b, c), Some(2.0));
    }
}
