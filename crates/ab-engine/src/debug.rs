//! Debug visualisation and profiling helpers.
//!
//! * [`DebugDraw`](ab_rendering::DebugDraw) (resource): lines and wire shapes from game code.
//! * [`DebugSettings`] (resource): what the engine draws for you: physics shapes, joints and
//!   contacts (toggled with F4, see [`EngineConfig::debug_physics_key`](crate::EngineConfig)),
//!   the navigation grid, the bounding box of every mesh.
//! * [`profile`]: time a block of game code into the [`Profiler`].
//!
//! ```no_run
//! use ab_engine::prelude::*;
//!
//! fn think(tree: &mut SceneTree, enemy: NodeHandle) {
//!     ab_engine::profile(tree, "ai.think", |tree| {
//!         let at = tree.global_position(enemy).unwrap_or_default();
//!         let mut debug = tree.world().resource_mut::<DebugDraw>().unwrap();
//!         debug.sphere(at, 5.0, Color::rgb(1.0, 0.3, 0.2)); // sight radius
//!     });
//! }
//! ```

use crate::profiler::Profiler;
use ab_core::{Color, Vec3};
use ab_ecs::World;
use ab_navigation::NavGrid;
use ab_physics::{PhysicsDebugOptions, PhysicsWorld};
use ab_rendering::DebugDraw;
use ab_scene::SceneTree;
use std::time::Instant;

/// ECS resource: debug views the engine draws into [`DebugDraw`] every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DebugSettings {
    /// Collision shapes (orange: simulated, cyan: kinematic, green: static, violet: areas,
    /// dimmed: asleep), joints (yellow) and contacts (red).
    pub physics: bool,
    pub physics_options: PhysicsDebugOptions,
    /// Walkable cells of the [`NavGrid`] resource.
    pub navigation: bool,
    /// The world bounding box of every visible mesh, as the renderer culls it (green: static,
    /// yellow: skinned, posed with its joints).
    pub bounds: bool,
}

/// Colour of a static mesh's box in the bounds view.
pub const BOUNDS_COLOR: Color = Color::rgb(0.35, 0.95, 0.45);
/// Colour of a skinned mesh's box in the bounds view.
pub const SKINNED_BOUNDS_COLOR: Color = Color::rgb(1.0, 0.85, 0.2);

/// Draws the views enabled in [`DebugSettings`]. The engine runs it in `PostUpdate`.
pub fn debug_shapes_system(world: &mut World) {
    let Some(settings) = world.resource::<DebugSettings>().map(|s| *s) else {
        return;
    };
    if !settings.physics && !settings.navigation && !settings.bounds {
        return;
    }
    let Some(mut debug) = world.resource_mut::<DebugDraw>() else {
        return;
    };
    if settings.physics {
        if let Some(physics) = world.resource::<PhysicsWorld>() {
            physics.debug_lines(&settings.physics_options, |a, b, color| {
                debug.line(a, b, color);
            });
        }
    }
    if settings.navigation {
        if let Some(grid) = world.resource::<NavGrid>() {
            draw_nav_grid(&grid, &mut debug);
        }
    }
    if settings.bounds {
        ab_rendering::for_each_mesh_bounds(world, |mesh| {
            let color = if mesh.skinned {
                SKINNED_BOUNDS_COLOR
            } else {
                BOUNDS_COLOR
            };
            debug.aabb(mesh.bounds.min, mesh.bounds.max, color);
        });
    }
}

fn draw_nav_grid(grid: &NavGrid, debug: &mut DebugDraw) {
    let half = grid.cell_size() * 0.4;
    let color = Color::rgb(0.2, 0.75, 1.0);
    for x in 0..grid.width() {
        for z in 0..grid.depth() {
            if !grid.is_walkable((x, z)) {
                continue;
            }
            let c = grid.cell_center((x, z)) + Vec3::Y * 0.04;
            let corners = [
                c + Vec3::new(-half, 0.0, -half),
                c + Vec3::new(half, 0.0, -half),
                c + Vec3::new(half, 0.0, half),
                c + Vec3::new(-half, 0.0, half),
                c + Vec3::new(-half, 0.0, -half),
            ];
            debug.path(&corners, color);
        }
    }
}

/// Runs `f` and records how long it took in the [`Profiler`] as `name` (shown with the
/// engine's sections in the F3 overlay and the editor's Profiler panel).
pub fn profile<R>(tree: &mut SceneTree, name: &str, f: impl FnOnce(&mut SceneTree) -> R) -> R {
    let start = Instant::now();
    let result = f(tree);
    if let Some(mut profiler) = tree.world().resource_mut::<Profiler>() {
        profiler.record(name, start.elapsed());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_core::Vec2;

    #[test]
    fn draws_enabled_views_only() {
        let mut world = World::new();
        world.insert_resource(DebugDraw::default());
        world.insert_resource(DebugSettings::default());
        let mut grid = NavGrid::new(Vec2::ZERO, Vec2::new(2.0, 2.0), 1.0);
        grid.set_walkable((0, 0), Some(0.0));
        grid.set_walkable((1, 1), Some(0.5));
        world.insert_resource(grid);
        debug_shapes_system(&mut world);
        assert_eq!(world.resource::<DebugDraw>().unwrap().line_count(), 0);
        world.resource_mut::<DebugSettings>().unwrap().navigation = true;
        debug_shapes_system(&mut world);
        assert_eq!(
            world.resource::<DebugDraw>().unwrap().line_count(),
            8,
            "a square per walkable cell"
        );
    }

    #[test]
    fn bounds_view_boxes_every_mesh() {
        use ab_rendering::{Material, Mesh, MeshInstance3D, RenderAssets, RenderNodes};
        let mut tree = SceneTree::new();
        let mut assets = RenderAssets::new();
        let cube = assets.add_mesh(Mesh::cube(1.0)).unwrap();
        let material = assets.add_material(Material::default());
        let world = tree.world_mut();
        world.insert_resource(assets);
        world.insert_resource(DebugDraw::default());
        world.insert_resource(DebugSettings {
            bounds: true,
            ..Default::default()
        });
        let root = tree.root();
        for name in ["A", "B"] {
            let node = tree.create_mesh_instance3d(name, MeshInstance3D::new(cube, material));
            tree.add_child(root, node).unwrap();
        }
        tree.update_transforms();
        debug_shapes_system(tree.world_mut());
        assert_eq!(
            tree.world().resource::<DebugDraw>().unwrap().line_count(),
            24,
            "12 edges per box"
        );
    }

    #[test]
    fn profile_records_into_the_profiler() {
        let mut tree = SceneTree::new();
        tree.world_mut().insert_resource(Profiler::new());
        let value = profile(&mut tree, "work", |_| 42);
        assert_eq!(value, 42);
        assert!(tree
            .world()
            .resource::<Profiler>()
            .unwrap()
            .average_ms("work")
            .is_some());
    }
}
