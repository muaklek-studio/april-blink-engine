//! The built-in generators (see the module docs of [`super`]).

use super::{scatter_points, Context, Generator, Noise, Output, Param, ParamDefault as D, ParamKind as K};
use crate::scene_file::{ColorDesc, MaterialDesc, MeshDesc, NodeDesc, PathDesc};
use ab_core::{Quat, Transform3D, Vec3};
use ab_rendering::{Mesh, Vertex};

pub static GENERATORS: &[&dyn Generator] = &[&Scatter, &Terrain, &Dungeon, &Building, &Town];

/// A piece (model or prefab) as a node.
fn piece(name: &str, path: &str) -> NodeDesc {
    NodeDesc {
        name: name.into(),
        instance: Some(path.into()),
        ..Default::default()
    }
}

/// A box of `size` with its bottom at the origin, coloured: what a generator builds with
/// when it was given no piece.
fn block(name: &str, size: Vec3, color: &str) -> crate::scene_file::SceneFile {
    crate::scene_file::SceneFile {
        environment: None,
        nodes: vec![
            NodeDesc {
                name: name.into(),
                kind: Some("StaticBody3D".into()),
                ..Default::default()
            },
            NodeDesc {
                name: "Mesh".into(),
                parent: Some(".".into()),
                kind: Some("MeshInstance3D".into()),
                position: Some([0.0, size.y / 2.0, 0.0]),
                scale: Some(size.to_array()),
                mesh: Some(MeshDesc::Cube { size: 1.0 }),
                material: Some(MaterialDesc {
                    color: Some(ColorDesc::Hex(color.into())),
                    roughness: Some(0.85),
                    ..Default::default()
                }),
                ..Default::default()
            },
            NodeDesc {
                name: "Shape".into(),
                parent: Some(".".into()),
                kind: Some("CollisionShape3D".into()),
                position: Some([0.0, size.y / 2.0, 0.0]),
                shape: Some(crate::scene_file::ShapeDesc::Box(size.to_array())),
                ..Default::default()
            },
        ],
    }
}

fn at(position: Vec3, yaw: f32, scale: f32) -> Transform3D {
    Transform3D::from_trs(position, Quat::from_rotation_y(yaw), Vec3::splat(scale))
}

// =============================================================================================
// Scatter
// =============================================================================================

/// Objects over an area or along a path: trees, rocks, streetlights, houses by a road.
pub struct Scatter;

static SCATTER: &[Param] = &[
    Param { key: "pieces", label: "Pieces", kind: K::Pieces, default: D::Empty, help: "Models or prefabs to place, picked at random" },
    Param { key: "count", label: "Count", kind: K::Int { min: 0, max: 5000 }, default: D::Int(40), help: "How many (over an area, or at random along a path)" },
    Param { key: "width", label: "Width (m)", kind: K::Float { min: 0.1, max: 5000.0 }, default: D::Float(40.0), help: "The area along X, centred on the generator" },
    Param { key: "depth", label: "Depth (m)", kind: K::Float { min: 0.1, max: 5000.0 }, default: D::Float(40.0), help: "The area along Z" },
    Param { key: "min_distance", label: "Min distance (m)", kind: K::Float { min: 0.0, max: 500.0 }, default: D::Float(2.0), help: "No two closer than this" },
    Param { key: "along", label: "Along path", kind: K::Node, default: D::Text(""), help: "A Path3D to follow instead of an area (\"../Road\")" },
    Param { key: "spacing", label: "Spacing (m)", kind: K::Float { min: 0.0, max: 1000.0 }, default: D::Float(0.0), help: "Along a path: one every this many metres (0: Count at random)" },
    Param { key: "offset", label: "Side offset (m)", kind: K::Float { min: 0.0, max: 500.0 }, default: D::Float(0.0), help: "Along a path: this far to its side" },
    Param { key: "both_sides", label: "Both sides", kind: K::Bool, default: D::Bool(false), help: "Along a path: on both sides" },
    Param { key: "rotation", label: "Rotation", kind: K::Choice(&["random", "along", "none"]), default: D::Text("random"), help: "Random turns, facing along the path, or none" },
    Param { key: "scale_min", label: "Scale from", kind: K::Float { min: 0.01, max: 100.0 }, default: D::Float(1.0), help: "Random scale between this…" },
    Param { key: "scale_max", label: "Scale to", kind: K::Float { min: 0.01, max: 100.0 }, default: D::Float(1.0), help: "…and this" },
    Param { key: "ground", label: "Stand on the ground", kind: K::Bool, default: D::Bool(true), help: "Put each on what is below it (a terrain, a floor)" },
    Param { key: "sink", label: "Sink (m)", kind: K::Float { min: -10.0, max: 10.0 }, default: D::Float(0.0), help: "How far into the ground" },
];

impl Generator for Scatter {
    fn kind(&self) -> &'static str {
        "scatter"
    }
    fn title(&self) -> &'static str {
        "Scatter (objects over an area or along a path)"
    }
    fn params(&self) -> &'static [Param] {
        SCATTER
    }
    fn stage(&self) -> u8 {
        1
    }

    fn generate(&self, cx: &mut Context<'_>) -> Result<Vec<Output>, String> {
        let pieces = cx.pieces("pieces");
        let mut rng = cx.rng.fork("scatter");
        let count = cx.int("count") as usize;
        let rotation = cx.text("rotation");
        // Places in the generator's space, and the direction there (along a path).
        let mut places: Vec<(Vec3, Vec3)> = Vec::new();
        match cx.node_param("along") {
            Some(path_node) => {
                let path = cx
                    .tree
                    .component::<crate::scene_file::path::Path3D>(path_node)
                    .map(|p| p.spline.clone())
                    .ok_or_else(|| format!("`{}` is not a Path3D", cx.text("along")))?;
                let to_local = cx.world().inverse() * super::world_of(cx.tree, path_node);
                let length = path.length();
                let spacing = cx.float("spacing");
                let distances: Vec<f32> = if spacing > 0.0 {
                    (0..=(length / spacing) as usize).map(|i| i as f32 * spacing).collect()
                } else {
                    (0..count).map(|_| rng.range(0.0, length)).collect()
                };
                let offset = cx.float("offset");
                let sides: &[f32] = if cx.bool("both_sides") { &[1.0, -1.0] } else { &[1.0] };
                for d in distances {
                    let Some((p, dir)) = path.at_distance(d) else { continue };
                    let side = dir.cross(Vec3::Y).normalize_or(Vec3::X);
                    for &s in sides {
                        let local = to_local.transform_point3(p + side * offset * s);
                        let facing = to_local.transform_vector3(-side * s);
                        places.push((local, facing));
                    }
                }
            }
            None => {
                let points = scatter_points(&mut rng, count, cx.float("width"), cx.float("depth"), cx.float("min_distance"));
                places.extend(points.into_iter().map(|(x, z)| (Vec3::new(x, 0.0, z), Vec3::Z)));
            }
        }
        let (scale_min, scale_max) = (cx.float("scale_min"), cx.float("scale_max").max(cx.float("scale_min")));
        let ground = cx.bool("ground");
        let sink = cx.float("sink");
        let mut out = Vec::with_capacity(places.len());
        for (i, (mut p, facing)) in places.into_iter().enumerate() {
            if ground {
                if let Some(y) = cx.ground(p) {
                    p.y = y;
                }
            }
            p.y -= sink;
            let yaw = match rotation.as_str() {
                "random" => rng.range(0.0, std::f32::consts::TAU),
                "along" => facing.x.atan2(facing.z),
                _ => 0.0,
            };
            let scale = rng.range(scale_min, scale_max);
            let name = format!("Item{i}");
            let scene = match rng.pick(&pieces) {
                Some(path) => crate::scene_file::SceneFile {
                    environment: None,
                    nodes: vec![piece(&name, path)],
                },
                None => block(&name, Vec3::new(0.6, 1.2, 0.6), "#6f8f55"),
            };
            out.push(Output::Scene { scene, at: at(p, yaw, scale) });
        }
        Ok(out)
    }
}

// =============================================================================================
// Terrain
// =============================================================================================

/// Hills and mountains from fractal noise, as one mesh (with collision).
pub struct Terrain;

static TERRAIN: &[Param] = &[
    Param { key: "size", label: "Size (m)", kind: K::Float { min: 1.0, max: 10000.0 }, default: D::Float(120.0), help: "Width and depth" },
    Param { key: "resolution", label: "Resolution", kind: K::Int { min: 2, max: 512 }, default: D::Int(96), help: "Squares along each side" },
    Param { key: "height", label: "Height (m)", kind: K::Float { min: 0.0, max: 2000.0 }, default: D::Float(18.0), help: "Highest peaks above the base" },
    Param { key: "feature", label: "Feature size (m)", kind: K::Float { min: 1.0, max: 5000.0 }, default: D::Float(45.0), help: "Larger: wider hills" },
    Param { key: "octaves", label: "Detail", kind: K::Int { min: 1, max: 8 }, default: D::Int(5), help: "Layers of noise" },
    Param { key: "ridges", label: "Mountain ridges", kind: K::Bool, default: D::Bool(false), help: "Sharp crests instead of rounded hills" },
    Param { key: "flat_radius", label: "Flat centre (m)", kind: K::Float { min: 0.0, max: 5000.0 }, default: D::Float(0.0), help: "Keep the middle level (for a town)" },
    Param { key: "color", label: "Color", kind: K::Color, default: D::Text("#5f7a43"), help: "Ground colour" },
    Param { key: "collide", label: "Collision", kind: K::Bool, default: D::Bool(true), help: "Characters and bodies stand on it" },
];

impl Generator for Terrain {
    fn kind(&self) -> &'static str {
        "terrain"
    }
    fn title(&self) -> &'static str {
        "Terrain (hills and mountains)"
    }
    fn params(&self) -> &'static [Param] {
        TERRAIN
    }

    fn generate(&self, cx: &mut Context<'_>) -> Result<Vec<Output>, String> {
        let size = cx.float("size");
        let n = cx.int("resolution") as usize;
        let height = cx.float("height");
        let feature = cx.float("feature");
        let octaves = cx.int("octaves") as u32;
        let ridges = cx.bool("ridges");
        let flat = cx.float("flat_radius");
        let noise = Noise::new(cx.rng.fork("terrain").next_u64());
        let h = |x: f32, z: f32| {
            let (u, v) = (x / feature, z / feature);
            let mut y = if ridges {
                noise.ridged(u, v, octaves)
            } else {
                noise.fbm(u, v, octaves, 2.0, 0.5) * 0.5 + 0.5
            } * height;
            if flat > 0.0 {
                let r = (x * x + z * z).sqrt();
                let k = ((r - flat) / flat.max(1.0)).clamp(0.0, 1.0);
                y *= k * k * (3.0 - 2.0 * k);
            }
            y
        };
        let step = size / n as f32;
        let half = size / 2.0;
        let mut vertices = Vec::with_capacity((n + 1) * (n + 1));
        for j in 0..=n {
            for i in 0..=n {
                let (x, z) = (-half + i as f32 * step, -half + j as f32 * step);
                let y = h(x, z);
                let dx = h(x + step, z) - h(x - step, z);
                let dz = h(x, z + step) - h(x, z - step);
                let normal = Vec3::new(-dx, 2.0 * step, -dz).normalize();
                vertices.push(Vertex::new(
                    Vec3::new(x, y, z),
                    normal,
                    ab_core::Vec2::new(x / 4.0, z / 4.0),
                ));
            }
        }
        let row = n as u32 + 1;
        let mut indices = Vec::with_capacity(n * n * 6);
        for j in 0..n as u32 {
            for i in 0..n as u32 {
                let (a, b) = (j * row + i, j * row + i + 1);
                let (c, d) = (a + row, b + row);
                indices.extend([a, c, b, b, c, d]);
            }
        }
        let material = MaterialDesc {
            color: Some(ColorDesc::Hex(cx.text("color"))),
            roughness: Some(0.95),
            ..Default::default()
        };
        Ok(vec![Output::Mesh {
            name: "Terrain".into(),
            mesh: Mesh::new(vertices, indices),
            material,
            collide: cx.bool("collide"),
        }])
    }
}

// =============================================================================================
// Grid rooms (dungeons, buildings)
// =============================================================================================

/// Floor cells on a grid, and the walls where floor meets nothing.
struct Layout {
    width: usize,
    depth: usize,
    floor: Vec<bool>,
}

impl Layout {
    fn new(width: usize, depth: usize) -> Self {
        Self {
            width,
            depth,
            floor: vec![false; width * depth],
        }
    }

    fn get(&self, x: i64, z: i64) -> bool {
        x >= 0
            && z >= 0
            && (x as usize) < self.width
            && (z as usize) < self.depth
            && self.floor[z as usize * self.width + x as usize]
    }

    fn set(&mut self, x: usize, z: usize) {
        if x < self.width && z < self.depth {
            self.floor[z * self.width + x] = true;
        }
    }

    /// Floor cells, and wall edges: (cell, direction 0..4 = +X, -X, +Z, -Z).
    fn walls(&self) -> Vec<((usize, usize), usize)> {
        let mut walls = Vec::new();
        for z in 0..self.depth {
            for x in 0..self.width {
                if !self.get(x as i64, z as i64) {
                    continue;
                }
                for (d, (dx, dz)) in [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().enumerate() {
                    if !self.get(x as i64 + dx, z as i64 + dz) {
                        walls.push(((x, z), d));
                    }
                }
            }
        }
        walls
    }
}

/// Floor tiles and walls of a layout as outputs: the given pieces, or blocks.
fn build_layout(
    layout: &Layout,
    cell: f32,
    wall_height: f32,
    floor_piece: Option<&str>,
    wall_piece: Option<&str>,
    y: f32,
    out: &mut Vec<Output>,
    name: &str,
) {
    let origin = Vec3::new(-(layout.width as f32) * cell / 2.0, y, -(layout.depth as f32) * cell / 2.0);
    let centre = |x: usize, z: usize| origin + Vec3::new((x as f32 + 0.5) * cell, 0.0, (z as f32 + 0.5) * cell);
    for z in 0..layout.depth {
        for x in 0..layout.width {
            if !layout.get(x as i64, z as i64) {
                continue;
            }
            let label = format!("{name}Floor{x}_{z}");
            let scene = match floor_piece {
                Some(p) => crate::scene_file::SceneFile { environment: None, nodes: vec![piece(&label, p)] },
                None => block(&label, Vec3::new(cell, 0.2, cell), "#6b6660"),
            };
            out.push(Output::Scene { scene, at: at(centre(x, z) - Vec3::Y * 0.2, 0.0, 1.0) });
        }
    }
    for ((x, z), d) in layout.walls() {
        let (offset, yaw) = match d {
            0 => (Vec3::X * cell / 2.0, std::f32::consts::FRAC_PI_2),
            1 => (Vec3::NEG_X * cell / 2.0, std::f32::consts::FRAC_PI_2),
            2 => (Vec3::Z * cell / 2.0, 0.0),
            _ => (Vec3::NEG_Z * cell / 2.0, 0.0),
        };
        let label = format!("{name}Wall{x}_{z}_{d}");
        let scene = match wall_piece {
            Some(p) => crate::scene_file::SceneFile { environment: None, nodes: vec![piece(&label, p)] },
            None => block(&label, Vec3::new(cell, wall_height, 0.2), "#a39b8f"),
        };
        out.push(Output::Scene { scene, at: at(centre(x, z) + offset, yaw, 1.0) });
    }
}

fn optional(text: String) -> Option<String> {
    Some(text).filter(|t| !t.trim().is_empty())
}

// =============================================================================================
// Dungeon
// =============================================================================================

/// Rooms joined by corridors on a grid, with floors, walls and lights.
pub struct Dungeon;

static DUNGEON: &[Param] = &[
    Param { key: "width", label: "Width (cells)", kind: K::Int { min: 6, max: 200 }, default: D::Int(28), help: "Grid cells along X" },
    Param { key: "depth", label: "Depth (cells)", kind: K::Int { min: 6, max: 200 }, default: D::Int(28), help: "Grid cells along Z" },
    Param { key: "cell", label: "Cell size (m)", kind: K::Float { min: 0.5, max: 20.0 }, default: D::Float(2.0), help: "One grid cell" },
    Param { key: "rooms", label: "Rooms", kind: K::Int { min: 1, max: 100 }, default: D::Int(8), help: "Rooms to try to fit" },
    Param { key: "room_min", label: "Room from (cells)", kind: K::Int { min: 2, max: 50 }, default: D::Int(3), help: "Smallest room side" },
    Param { key: "room_max", label: "Room to (cells)", kind: K::Int { min: 2, max: 50 }, default: D::Int(7), help: "Largest room side" },
    Param { key: "wall_height", label: "Wall height (m)", kind: K::Float { min: 0.5, max: 20.0 }, default: D::Float(3.0), help: "Blocks' wall height (pieces keep theirs)" },
    Param { key: "floor_piece", label: "Floor piece", kind: K::Piece, default: D::Text(""), help: "One cell of floor (else a block)" },
    Param { key: "wall_piece", label: "Wall piece", kind: K::Piece, default: D::Text(""), help: "One cell of wall, facing Z (else a block)" },
    Param { key: "lights", label: "Lights", kind: K::Bool, default: D::Bool(true), help: "A light in every room" },
];

impl Generator for Dungeon {
    fn kind(&self) -> &'static str {
        "dungeon"
    }
    fn title(&self) -> &'static str {
        "Dungeon (rooms and corridors)"
    }
    fn params(&self) -> &'static [Param] {
        DUNGEON
    }

    fn generate(&self, cx: &mut Context<'_>) -> Result<Vec<Output>, String> {
        let (w, d) = (cx.int("width") as usize, cx.int("depth") as usize);
        let cell = cx.float("cell");
        let (min, max) = (cx.int("room_min"), cx.int("room_max").max(cx.int("room_min")));
        let mut rng = cx.rng.fork("rooms");
        let mut layout = Layout::new(w, d);
        let mut rooms: Vec<(usize, usize, usize, usize)> = Vec::new();
        for _ in 0..cx.int("rooms") * 20 {
            if rooms.len() as i64 >= cx.int("rooms") {
                break;
            }
            let rw = rng.int(min, max) as usize;
            let rd = rng.int(min, max) as usize;
            if rw + 2 >= w || rd + 2 >= d {
                continue;
            }
            let rx = rng.int(1, (w - rw - 1) as i64) as usize;
            let rz = rng.int(1, (d - rd - 1) as i64) as usize;
            // One cell apart from the others at least.
            let clear = rooms.iter().all(|&(x, z, ow, od)| {
                rx + rw + 1 <= x || x + ow + 1 <= rx || rz + rd + 1 <= z || z + od + 1 <= rz
            });
            if clear {
                rooms.push((rx, rz, rw, rd));
            }
        }
        if rooms.is_empty() {
            return Err("no room fits: make the grid larger or the rooms smaller".into());
        }
        for &(x, z, rw, rd) in &rooms {
            for cz in z..z + rd {
                for cx_ in x..x + rw {
                    layout.set(cx_, cz);
                }
            }
        }
        // Each room to the next (by position), with an L-shaped corridor.
        let mut order: Vec<_> = rooms.iter().map(|&(x, z, rw, rd)| (x + rw / 2, z + rd / 2)).collect();
        order.sort();
        for pair in order.windows(2) {
            let ((ax, az), (bx, bz)) = (pair[0], pair[1]);
            let horizontal_first = rng.chance(0.5);
            let (corner_x, corner_z) = if horizontal_first { (bx, az) } else { (ax, bz) };
            for x in ax.min(corner_x)..=ax.max(corner_x) {
                layout.set(x, if horizontal_first { az } else { bz });
            }
            for z in az.min(bz)..=az.max(bz) {
                layout.set(corner_x, z);
            }
            let _ = corner_z;
        }
        let mut out = Vec::new();
        let floor = optional(cx.text("floor_piece"));
        let wall = optional(cx.text("wall_piece"));
        build_layout(&layout, cell, cx.float("wall_height"), floor.as_deref(), wall.as_deref(), 0.0, &mut out, "");
        if cx.bool("lights") {
            let origin = Vec3::new(-(w as f32) * cell / 2.0, 0.0, -(d as f32) * cell / 2.0);
            for (i, &(x, z, rw, rd)) in rooms.iter().enumerate() {
                let centre = origin + Vec3::new((x as f32 + rw as f32 / 2.0) * cell, cx.float("wall_height") * 0.8, (z as f32 + rd as f32 / 2.0) * cell);
                let light = NodeDesc {
                    name: format!("RoomLight{i}"),
                    kind: Some("OmniLight3D".into()),
                    light: Some(crate::scene_file::LightDesc {
                        color: Some(ColorDesc::Hex("#ffcf8a".into())),
                        energy: Some(3.0),
                        range: Some(rw.max(rd) as f32 * cell),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                out.push(Output::node(light, at(centre, 0.0, 1.0)));
            }
        }
        Ok(out)
    }
}

// =============================================================================================
// Building
// =============================================================================================

/// A building: floors of walls with windows, a door, a roof.
pub struct Building;

static BUILDING: &[Param] = &[
    Param { key: "floors", label: "Floors", kind: K::Int { min: 1, max: 100 }, default: D::Int(3), help: "Storeys" },
    Param { key: "width", label: "Width (cells)", kind: K::Int { min: 1, max: 50 }, default: D::Int(4), help: "Along X" },
    Param { key: "depth", label: "Depth (cells)", kind: K::Int { min: 1, max: 50 }, default: D::Int(3), help: "Along Z" },
    Param { key: "cell", label: "Cell size (m)", kind: K::Float { min: 0.5, max: 20.0 }, default: D::Float(3.0), help: "One wall section" },
    Param { key: "floor_height", label: "Floor height (m)", kind: K::Float { min: 1.0, max: 20.0 }, default: D::Float(3.0), help: "One storey" },
    Param { key: "windows", label: "Windows", kind: K::Float { min: 0.0, max: 1.0 }, default: D::Float(0.6), help: "Share of wall sections with a window" },
    Param { key: "wall_color", label: "Wall color", kind: K::Color, default: D::Text("#c9b9a0"), help: "Blocks' colour" },
    Param { key: "roof_color", label: "Roof color", kind: K::Color, default: D::Text("#7a3b2e"), help: "Blocks' colour" },
    Param { key: "wall_piece", label: "Wall piece", kind: K::Piece, default: D::Text(""), help: "One wall section, facing Z (else a block)" },
    Param { key: "window_piece", label: "Window piece", kind: K::Piece, default: D::Text(""), help: "A wall section with a window (else a block with glass)" },
];

impl Generator for Building {
    fn kind(&self) -> &'static str {
        "building"
    }
    fn title(&self) -> &'static str {
        "Building (floors, windows, roof)"
    }
    fn params(&self) -> &'static [Param] {
        BUILDING
    }

    fn generate(&self, cx: &mut Context<'_>) -> Result<Vec<Output>, String> {
        let (w, d, floors) = (cx.int("width") as usize, cx.int("depth") as usize, cx.int("floors") as usize);
        let (cell, story) = (cx.float("cell"), cx.float("floor_height"));
        let mut rng = cx.rng.fork("windows");
        let wall_color = cx.text("wall_color");
        let wall_piece = optional(cx.text("wall_piece"));
        let window_piece = optional(cx.text("window_piece"));
        let mut layout = Layout::new(w, d);
        for z in 0..d {
            for x in 0..w {
                layout.set(x, z);
            }
        }
        let origin = Vec3::new(-(w as f32) * cell / 2.0, 0.0, -(d as f32) * cell / 2.0);
        let mut out = Vec::new();
        // The door: the middle of the front (-Z) on the ground floor.
        let door = (w / 2, 0usize, 3usize);
        for f in 0..floors {
            let y = f as f32 * story;
            for ((x, z), side) in layout.walls() {
                let (offset, yaw) = match side {
                    0 => (Vec3::X * cell / 2.0, std::f32::consts::FRAC_PI_2),
                    1 => (Vec3::NEG_X * cell / 2.0, std::f32::consts::FRAC_PI_2),
                    2 => (Vec3::Z * cell / 2.0, 0.0),
                    _ => (Vec3::NEG_Z * cell / 2.0, 0.0),
                };
                let centre = origin + Vec3::new((x as f32 + 0.5) * cell, y, (z as f32 + 0.5) * cell) + offset;
                let name = format!("F{f}Wall{x}_{z}_{side}");
                let is_door = f == 0 && (x, z, side) == door;
                let window = !is_door && rng.chance(cx.float("windows"));
                let scene = match (is_door, window, &wall_piece, &window_piece) {
                    (false, true, _, Some(p)) => crate::scene_file::SceneFile { environment: None, nodes: vec![piece(&name, p)] },
                    (false, false, Some(p), _) => crate::scene_file::SceneFile { environment: None, nodes: vec![piece(&name, p)] },
                    _ => {
                        let mut scene = block(&name, Vec3::new(cell, story, 0.25), &wall_color);
                        if is_door || window {
                            // A dark panel set into the wall: the door or the glass.
                            let (size, lift, color) = if is_door {
                                ([cell * 0.4, story * 0.7, 0.3], story * 0.35, "#4a3424")
                            } else {
                                ([cell * 0.5, story * 0.4, 0.3], story * 0.55, "#2c3e50")
                            };
                            scene.nodes.push(NodeDesc {
                                name: if is_door { "Door".into() } else { "Window".into() },
                                parent: Some(".".into()),
                                kind: Some("MeshInstance3D".into()),
                                position: Some([0.0, lift, 0.0]),
                                scale: Some(size),
                                mesh: Some(MeshDesc::Cube { size: 1.0 }),
                                material: Some(MaterialDesc {
                                    color: Some(ColorDesc::Hex(color.into())),
                                    roughness: Some(if is_door { 0.8 } else { 0.1 }),
                                    metallic: Some(if is_door { 0.0 } else { 0.5 }),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            });
                        }
                        scene
                    }
                };
                out.push(Output::Scene { scene, at: at(centre, yaw, 1.0) });
            }
            // The floor slab of this storey.
            let slab = block(&format!("Floor{f}"), Vec3::new(w as f32 * cell, 0.2, d as f32 * cell), "#7d7468");
            out.push(Output::Scene { scene: slab, at: at(Vec3::new(0.0, y, 0.0), 0.0, 1.0) });
        }
        let roof = block("Roof", Vec3::new(w as f32 * cell + 0.6, 0.4, d as f32 * cell + 0.6), &cx.text("roof_color"));
        out.push(Output::Scene { scene: roof, at: at(Vec3::new(0.0, floors as f32 * story, 0.0), 0.0, 1.0) });
        Ok(out)
    }
}

// =============================================================================================
// Town
// =============================================================================================

/// Blocks of buildings between roads: a grid of road paths, and building generators (or
/// pieces) along them. Made of the other generators' parts: paths and buildings.
pub struct Town;

static TOWN: &[Param] = &[
    Param { key: "blocks_x", label: "Blocks along X", kind: K::Int { min: 1, max: 30 }, default: D::Int(3), help: "Town blocks" },
    Param { key: "blocks_z", label: "Blocks along Z", kind: K::Int { min: 1, max: 30 }, default: D::Int(2), help: "Town blocks" },
    Param { key: "block", label: "Block size (m)", kind: K::Float { min: 8.0, max: 500.0 }, default: D::Float(30.0), help: "Between road centres" },
    Param { key: "road_width", label: "Road width (m)", kind: K::Float { min: 1.0, max: 50.0 }, default: D::Float(6.0), help: "Roads are ribbons with collision" },
    Param { key: "houses", label: "Buildings per side", kind: K::Int { min: 0, max: 20 }, default: D::Int(2), help: "Along each side of a block" },
    Param { key: "pieces", label: "House pieces", kind: K::Pieces, default: D::Empty, help: "Prefabs to use (else generated buildings)" },
    Param { key: "max_floors", label: "Max floors", kind: K::Int { min: 1, max: 50 }, default: D::Int(4), help: "Generated buildings get 1 to this many" },
];

impl Generator for Town {
    fn kind(&self) -> &'static str {
        "town"
    }
    fn title(&self) -> &'static str {
        "Town (roads and buildings)"
    }
    fn params(&self) -> &'static [Param] {
        TOWN
    }

    fn generate(&self, cx: &mut Context<'_>) -> Result<Vec<Output>, String> {
        let (bx, bz) = (cx.int("blocks_x") as usize, cx.int("blocks_z") as usize);
        let block = cx.float("block");
        let road = cx.float("road_width");
        let pieces = cx.pieces("pieces");
        let mut rng = cx.rng.fork("town");
        let (wx, wz) = (bx as f32 * block, bz as f32 * block);
        let mut out = Vec::new();
        let road_desc = |name: String, from: Vec3, to: Vec3| NodeDesc {
            name,
            kind: Some("Path3D".into()),
            path: Some(PathDesc {
                points: vec![from.to_array(), to.to_array()],
                mode: Some("ribbon".into()),
                width: Some(road),
                collide: Some(true),
                ..Default::default()
            }),
            material: Some(MaterialDesc {
                color: Some(ColorDesc::Hex("#3a3b3e".into())),
                roughness: Some(0.9),
                ..Default::default()
            }),
            ..Default::default()
        };
        for i in 0..=bx {
            let x = -wx / 2.0 + i as f32 * block;
            out.push(Output::node(road_desc(format!("RoadX{i}"), Vec3::new(x, 0.02, -wz / 2.0), Vec3::new(x, 0.02, wz / 2.0)), Transform3D::IDENTITY));
        }
        for j in 0..=bz {
            let z = -wz / 2.0 + j as f32 * block;
            out.push(Output::node(road_desc(format!("RoadZ{j}"), Vec3::new(-wx / 2.0, 0.03, z), Vec3::new(wx / 2.0, 0.03, z)), Transform3D::IDENTITY));
        }
        // Buildings around the inside of each block, facing the road.
        let per_side = cx.int("houses") as usize;
        let inner = block - road;
        let lot = inner / per_side.max(1) as f32;
        let mut n = 0;
        for i in 0..bx {
            for j in 0..bz {
                let centre = Vec3::new(-wx / 2.0 + (i as f32 + 0.5) * block, 0.0, -wz / 2.0 + (j as f32 + 0.5) * block);
                for side in 0..4 {
                    for k in 0..per_side {
                        let along = -inner / 2.0 + (k as f32 + 0.5) * lot;
                        let back = inner / 2.0 - lot * 0.4;
                        let (offset, yaw) = match side {
                            0 => (Vec3::new(along, 0.0, -back), 0.0),
                            1 => (Vec3::new(back, 0.0, along), -std::f32::consts::FRAC_PI_2),
                            2 => (Vec3::new(-along, 0.0, back), std::f32::consts::PI),
                            _ => (Vec3::new(-back, 0.0, -along), std::f32::consts::FRAC_PI_2),
                        };
                        let name = format!("House{n}");
                        n += 1;
                        let desc = match rng.pick(&pieces) {
                            Some(p) => piece(&name, p),
                            None => {
                                let cells = ((lot * 0.7) / 3.0).floor().max(1.0) as i64;
                                let mut params = toml::Table::new();
                                params.insert("floors".into(), rng.int(1, cx.int("max_floors")).into());
                                params.insert("width".into(), cells.into());
                                params.insert("depth".into(), cells.into());
                                NodeDesc {
                                    name,
                                    kind: Some("Generator".into()),
                                    generator: Some(crate::scene_file::GeneratorDesc {
                                        kind: "building".into(),
                                        seed: rng.next_u64() % 1_000_000,
                                        params,
                                    }),
                                    ..Default::default()
                                }
                            }
                        };
                        out.push(Output::node(desc, at(centre + offset, yaw, 1.0)));
                    }
                }
            }
        }
        Ok(out)
    }
}
