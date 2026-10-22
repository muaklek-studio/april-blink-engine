//! Blood: droplets and a fine mist thrown from a wound, and splats left where the droplets
//! land.
//!
//! ```no_run
//! use ab_engine::prelude::*;
//! use ab_engine::blood::{BloodNodes, BloodOptions};
//!
//! fn on_hit(tree: &mut SceneTree, at: Vec3, bullet_direction: Vec3) {
//!     let root = tree.root();
//!     let _ = tree.spawn_blood(root, at, BloodOptions::along(bullet_direction));
//! }
//! ```
//!
//! Splats are flat decals on the surfaces the droplets would reach (found with physics
//! raycasts, so only colliders get them). Each one spreads out when its droplet lands, stays,
//! then dries and shrinks away. They come from a fixed pool ([`BloodSplats::max`]): a new
//! splat replaces the oldest one, so a long fight costs no more than a short one.

use ab_core::utils::rand::SimpleRng;
use ab_core::{Color, Quat, Time, Transform3D, Vec3};
use ab_ecs::World;
use ab_effects::{EffectNodes, ParticleEmitter, ParticleSettings};
use ab_physics::{PhysicsWorld, QueryOptions};
use ab_rendering::{
    ColorSpace, Material, MaterialHandle, Mesh, MeshHandle, MeshInstance3D, RenderAssets,
    RenderNodes, Texture, TextureHandle,
};
use ab_scene::{NodeHandle, SceneError, SceneTree};

/// How a [`BloodNodes::spawn_blood`] looks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BloodOptions {
    /// Which way the blood leaves the wound: the hit's direction, or the surface normal.
    pub direction: Vec3,
    /// How much: 1 a normal hit, 2–3 a heavy one, 0.5 a graze.
    pub amount: f32,
    /// Splats left on nearby surfaces (0 = none).
    pub splats: u32,
    /// Seconds a splat stays before it dries away (it then takes a quarter of that to go).
    pub splat_lifetime: f32,
    /// Colour of fresh blood (linear).
    pub color: Color,
    /// Physics layers splats stick to.
    pub collision_mask: u32,
}

impl Default for BloodOptions {
    fn default() -> Self {
        Self {
            direction: Vec3::Y,
            amount: 1.0,
            splats: 4,
            splat_lifetime: 20.0,
            color: Color::rgb(0.15, 0.004, 0.006),
            collision_mask: u32::MAX,
        }
    }
}

impl BloodOptions {
    /// Blood thrown along `direction`.
    pub fn along(direction: Vec3) -> Self {
        Self {
            direction,
            ..Self::default()
        }
    }
}

/// `spawn_blood` on [`SceneTree`] (`use ab_engine::blood::BloodNodes;`).
pub trait BloodNodes {
    /// Droplets and mist at `position` under `parent` (freed when done), and splats where
    /// the droplets land. Returns the particle node.
    fn spawn_blood(
        &mut self,
        parent: NodeHandle,
        position: Vec3,
        options: BloodOptions,
    ) -> Result<NodeHandle, SceneError>;
}

impl BloodNodes for SceneTree {
    fn spawn_blood(
        &mut self,
        parent: NodeHandle,
        position: Vec3,
        options: BloodOptions,
    ) -> Result<NodeHandle, SceneError> {
        let amount = options.amount.max(0.05);
        let direction = options.direction.normalize_or(Vec3::Y);
        let tint = |mut settings: ParticleSettings| {
            for (_, color) in &mut settings.color.0 {
                let alpha = color.a;
                // Thin droplets let more light through than a pool.
                *color = Color::rgba(
                    options.color.r * 1.8,
                    options.color.g * 1.8,
                    options.color.b * 1.8,
                    alpha,
                );
            }
            settings
        };
        let droplets = tint(ParticleSettings::blood())
            .with_amount((45.0 * amount).round().max(4.0) as u32)
            .with_direction(direction, 28.0);
        let node = self.spawn_effect(parent, position, droplets)?;
        let mist = tint(ParticleSettings::blood_mist())
            .with_amount((14.0 * amount.sqrt()).round().max(2.0) as u32)
            .with_direction(direction, 40.0);
        let mist = self.create_particles3d("Mist", ParticleEmitter::new(mist).one_shot());
        self.add_child(node, mist)?;
        if options.splats > 0 {
            place_splats(self, parent, position, direction, amount, &options);
        }
        Ok(node)
    }
}

/// Throws `options.splats` droplets along arcs from `origin` and leaves a splat where each
/// one meets a collider.
fn place_splats(
    tree: &mut SceneTree,
    parent: NodeHandle,
    origin: Vec3,
    direction: Vec3,
    amount: f32,
    options: &BloodOptions,
) {
    let seed = {
        let splats = tree
            .world_mut()
            .resource_or_insert_with(BloodSplats::default);
        splats.spawned = splats.spawned.wrapping_add(1);
        splats.spawned.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ u64::from(origin.x.to_bits())
    };
    let mut rng = SimpleRng::new(seed);
    let mut hits = Vec::new();
    {
        let Some(physics) = tree.world().resource::<PhysicsWorld>() else {
            return;
        };
        let query = QueryOptions::new().with_mask(options.collision_mask);
        for _ in 0..options.splats {
            // Thrown along the hit, falling: most land ahead and below, some right under.
            let jitter = Vec3::new(
                rng.range_f32(-1.0, 1.0),
                rng.range_f32(-1.0, 1.0),
                rng.range_f32(-1.0, 1.0),
            ) * 0.45;
            let throw = direction * rng.range_f32(0.2, 1.3) + Vec3::NEG_Y * 1.2 + jitter;
            let reach = 3.0 * amount.sqrt();
            if let Some(hit) = physics.cast_ray(origin, throw, reach, &query) {
                hits.push((hit.position, hit.normal, hit.distance));
            }
        }
    }
    for (position, normal, distance) in hits {
        let size = rng.range_f32(0.25, 0.6) * amount.sqrt();
        let spin = rng.range_f32(0.0, std::f32::consts::TAU);
        let variant = rng.next_u32() as usize;
        let splat = SplatSpawn {
            position,
            normal,
            size,
            spin,
            variant,
            // Droplets fly at about 6 m/s.
            delay: distance / 6.0,
            lifetime: options.splat_lifetime.max(0.5),
            color: options.color,
        };
        if let Err(err) = spawn_splat(tree, parent, &splat) {
            tracing::warn!("blood splat: {err}");
        }
    }
}

/// ECS resource: the pool of blood splats ([`BloodNodes::spawn_blood`]).
#[derive(Debug)]
pub struct BloodSplats {
    /// Splats on screen at most; a new one replaces the oldest.
    pub max: usize,
    splats: Vec<Splat>,
    /// Slot the next splat takes once the pool is full.
    next: usize,
    assets: Option<SplatAssets>,
    spawned: u64,
}

impl Default for BloodSplats {
    fn default() -> Self {
        Self {
            max: 48,
            splats: Vec::new(),
            next: 0,
            assets: None,
            spawned: 0,
        }
    }
}

impl BloodSplats {
    /// Splat slots made so far (at most [`BloodSplats::max`]).
    pub fn slots(&self) -> usize {
        self.splats.len()
    }
}

#[derive(Debug, Clone, Copy)]
struct SplatAssets {
    mesh: MeshHandle,
    textures: [TextureHandle; SPLAT_VARIANTS],
}

#[derive(Debug)]
struct Splat {
    node: Option<NodeHandle>,
    material: MaterialHandle,
    age: f32,
    delay: f32,
    lifetime: f32,
    color: Color,
}

struct SplatSpawn {
    position: Vec3,
    normal: Vec3,
    size: f32,
    spin: f32,
    variant: usize,
    delay: f32,
    lifetime: f32,
    color: Color,
}

const SPLAT_VARIANTS: usize = 3;
const SPLAT_TEXTURE_SIZE: u32 = 128;
/// Alpha cutoff of a hidden splat: above any texel's alpha.
const HIDDEN: f32 = 1.01;
/// Alpha cutoff of a fully spread splat (the texture's edge sits at alpha 0.5).
const SPREAD: f32 = 0.5;
/// Seconds a splat takes to spread once its droplet lands.
const SPREAD_TIME: f32 = 0.18;

fn spawn_splat(
    tree: &mut SceneTree,
    parent: NodeHandle,
    splat: &SplatSpawn,
) -> Result<(), SceneError> {
    let world = tree.world_mut();
    world.resource_or_insert_with(BloodSplats::default);
    let assets = splat_assets(world);
    let mut pool = world.resource_mut::<BloodSplats>().expect("inserted above");
    let max = pool.max.max(1);
    // A free slot (empty, or its splat dried away), a new one, or the oldest.
    let dried = |s: &Splat| s.node.is_none_or(|n| !tree_visible(world, n));
    let slot = match pool.splats.iter().position(dried) {
        Some(free) => free,
        None if pool.splats.len() < max => {
            drop(pool);
            let material = world
                .resource_mut::<RenderAssets>()
                .expect("splat_assets made it")
                .add_material(splat_material(splat.color, None));
            pool = world.resource_mut::<BloodSplats>().expect("inserted above");
            pool.splats.push(Splat {
                node: None,
                material,
                age: 0.0,
                delay: 0.0,
                lifetime: 0.0,
                color: splat.color,
            });
            pool.splats.len() - 1
        }
        None => {
            let oldest = pool.next % pool.splats.len();
            pool.next = oldest + 1;
            oldest
        }
    };
    let old = pool.splats[slot].node.take();
    let material = pool.splats[slot].material;
    drop(pool);
    if let Some(old) = old.filter(|&n| tree.is_valid(n)) {
        let _ = tree.queue_free(old);
    }
    if let Some(mut render) = tree.world().resource_mut::<RenderAssets>() {
        if let Some(m) = render.material_mut(material) {
            *m = splat_material(
                splat.color,
                Some(assets.textures[splat.variant % SPLAT_VARIANTS]),
            );
        }
    }
    let node = tree.create_mesh_instance3d(
        "BloodSplat",
        MeshInstance3D {
            cast_shadows: false,
            ..MeshInstance3D::new(assets.mesh, material)
        },
    );
    tree.add_child(parent, node)?;
    // Lie on the surface, a hair above it, turned at random about the normal.
    let up = splat.normal.normalize_or(Vec3::Y);
    let rotation = Quat::from_rotation_arc(Vec3::Y, up) * Quat::from_rotation_y(splat.spin);
    let global = Transform3D::from_trs(
        splat.position + up * 0.004,
        rotation,
        Vec3::new(splat.size, 1.0, splat.size),
    );
    tree.set_global_transform(node, global.to_affine())?;
    tree.update_transforms();
    let mut pool = tree
        .world()
        .resource_mut::<BloodSplats>()
        .expect("inserted above");
    let entry = &mut pool.splats[slot];
    entry.node = Some(node);
    entry.age = 0.0;
    entry.delay = splat.delay;
    entry.lifetime = splat.lifetime;
    entry.color = splat.color;
    Ok(())
}

fn splat_material(color: Color, texture: Option<TextureHandle>) -> Material {
    Material {
        albedo_texture: texture,
        alpha_cutoff: Some(HIDDEN),
        // Wet and glossy while fresh.
        roughness: 0.22,
        ..Material::from_color(color)
    }
}

fn tree_visible(world: &World, node: NodeHandle) -> bool {
    world
        .storage::<ab_scene::NodeInfo>()
        .is_some_and(|i| i.get(node.entity()).is_some_and(|i| i.in_tree && i.visible))
}

/// The splat mesh and textures, made on first use.
fn splat_assets(world: &mut World) -> SplatAssets {
    if let Some(assets) = world.resource::<BloodSplats>().and_then(|p| p.assets) {
        return assets;
    }
    let render = world.resource_or_insert_with(RenderAssets::new);
    let mesh = render
        .add_mesh(Mesh::plane(1.0, 1))
        .expect("the plane primitive is valid");
    let textures = std::array::from_fn(|i| {
        render
            .add_texture(splat_texture(i as u64 + 1))
            .expect("splat textures are valid")
    });
    let assets = SplatAssets { mesh, textures };
    if let Some(mut pool) = world.resource_mut::<BloodSplats>() {
        pool.assets = Some(assets);
    }
    assets
}

/// A splat: an irregular pool with droplets thrown around it and a few streaks, white (the
/// material tints it). Alpha falls off smoothly across the edge, which sits at 0.5, so
/// raising the material's alpha cutoff shrinks the splat from its edges inwards.
fn splat_texture(seed: u64) -> Texture {
    let n = SPLAT_TEXTURE_SIZE;
    let mut rng = SimpleRng::new(seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
    // The main pool's radius around the centre, wobbling with the angle.
    let wobble: [(f32, f32); 5] = std::array::from_fn(|i| {
        (
            rng.range_f32(0.03, 0.09) / (1.0 + i as f32 * 0.6),
            rng.range_f32(0.0, std::f32::consts::TAU),
        )
    });
    // Droplets around it: (centre, radius).
    let drops: Vec<(f32, f32, f32)> = (0..9)
        .map(|_| {
            let angle = rng.range_f32(0.0, std::f32::consts::TAU);
            let distance = rng.range_f32(0.27, 0.45);
            (
                angle.cos() * distance,
                angle.sin() * distance,
                rng.range_f32(0.012, 0.035),
            )
        })
        .collect();
    let mut data = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            // -0.5..0.5 across the texture.
            let u = (x as f32 + 0.5) / n as f32 - 0.5;
            let v = (y as f32 + 0.5) / n as f32 - 0.5;
            let r = (u * u + v * v).sqrt();
            let angle = v.atan2(u);
            // Never close to the middle: the pool is thickest there and dries last.
            let edge = (0.2
                + wobble
                    .iter()
                    .enumerate()
                    .map(|(i, (amp, phase))| amp * ((i as f32 + 2.0) * angle + phase).sin())
                    .sum::<f32>())
            .max(0.12);
            // Signed distance in texture units: positive inside.
            let mut inside = edge - r;
            for &(cx, cy, radius) in &drops {
                let d = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
                inside = inside.max(radius - d);
            }
            // Streaks: thin spikes along a few of the droplets' directions.
            for &(cx, cy, radius) in drops.iter().take(4) {
                let len = (cx * cx + cy * cy).sqrt();
                let (dx, dy) = (cx / len, cy / len);
                let along = u * dx + v * dy;
                let across = (u * dy - v * dx).abs();
                if (0.0..len).contains(&along) {
                    inside = inside.max(radius * 0.35 * (1.0 - along / len * 0.5) - across);
                }
            }
            let alpha = (0.5 + inside * 6.0).clamp(0.0, 1.0);
            // Thicker blood is darker in the middle.
            let shade = (215.0 + 40.0 * (1.0 - alpha)) as u8;
            data.extend_from_slice(&[shade, shade, shade, (alpha * 255.0).round() as u8]);
        }
    }
    Texture::new(n, n, data, ColorSpace::Srgb).expect("sized to match")
}

/// Spreads, keeps and dries the splats. The engine runs it in `PostUpdate`.
pub fn blood_system(world: &mut World) {
    let Some(delta) = world.resource::<Time>().map(|t| t.delta) else {
        return;
    };
    let Some(mut pool) = world.resource_mut::<BloodSplats>() else {
        return;
    };
    if pool.splats.iter().all(|s| s.node.is_none()) {
        return;
    }
    let Some(mut render) = world.resource_mut::<RenderAssets>() else {
        return;
    };
    let Some(mut infos) = world.storage_mut::<ab_scene::NodeInfo>() else {
        return;
    };
    for splat in &mut pool.splats {
        let Some(node) = splat.node else {
            continue;
        };
        // Freed with its scene.
        let Some(info) = infos.get_mut(node.entity()).filter(|i| i.in_tree) else {
            splat.node = None;
            continue;
        };
        if !info.visible {
            continue; // dried away; its slot takes the next splat
        }
        splat.age += delta;
        let (cutoff, dryness) = splat_look(splat.age, splat.delay, splat.lifetime);
        if cutoff >= HIDDEN && splat.age > splat.delay {
            // Hidden until the slot is reused (systems cannot free nodes; spawning can).
            info.visible = false;
            continue;
        }
        if let Some(material) = render.material_mut(splat.material) {
            material.alpha_cutoff = Some(cutoff);
            // Drying blood darkens towards brown and loses its shine.
            let c = splat.color;
            let dark = 1.0 - 0.55 * dryness;
            material.albedo = Color::rgb(
                c.r * dark + 0.02 * dryness,
                c.g * dark + 0.008 * dryness,
                c.b * dark,
            );
            material.roughness = 0.22 + 0.6 * dryness;
        }
    }
}

/// Alpha cutoff and dryness (0..1) of a splat `age` seconds after it was spawned.
fn splat_look(age: f32, delay: f32, lifetime: f32) -> (f32, f32) {
    let landed = age - delay;
    if landed <= 0.0 {
        return (HIDDEN, 0.0);
    }
    if landed < SPREAD_TIME {
        // Spreads from the middle out: cutoff from 1 down to the edge's 0.5.
        let t = landed / SPREAD_TIME;
        let eased = 1.0 - (1.0 - t) * (1.0 - t);
        return (1.0 - (1.0 - SPREAD) * eased, 0.0);
    }
    let dryness = ((landed - SPREAD_TIME) / lifetime).clamp(0.0, 1.0);
    let fade = lifetime * 0.25;
    let gone = landed - SPREAD_TIME - lifetime;
    if gone <= 0.0 {
        return (SPREAD, dryness);
    }
    // Shrinks from the edges until nothing is left.
    let t = (gone / fade).clamp(0.0, 1.0);
    (SPREAD + (HIDDEN - SPREAD) * t, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splats_wait_spread_dry_and_vanish() {
        let (delay, life) = (0.3, 4.0);
        assert_eq!(
            splat_look(0.1, delay, life).0,
            HIDDEN,
            "droplet still flying"
        );
        let spreading = splat_look(delay + SPREAD_TIME * 0.5, delay, life).0;
        assert!(spreading < 1.0 && spreading > SPREAD);
        let (cutoff, dryness) = splat_look(delay + 1.0, delay, life);
        assert_eq!(cutoff, SPREAD);
        assert!((dryness - (1.0 - SPREAD_TIME) / life).abs() < 1e-5);
        let drying = splat_look(delay + SPREAD_TIME + life + 0.5, delay, life);
        assert!(drying.0 > SPREAD && drying.0 < HIDDEN && drying.1 == 1.0);
        assert_eq!(
            splat_look(delay + SPREAD_TIME + life * 1.25 + 0.01, delay, life).0,
            HIDDEN
        );
    }

    #[test]
    fn splat_texture_has_a_soft_edged_pool() {
        let texture = splat_texture(1);
        let n = SPLAT_TEXTURE_SIZE as usize;
        let alpha = |x: usize, y: usize| texture.data[(y * n + x) * 4 + 3];
        assert_eq!(alpha(n / 2, n / 2), 255, "solid in the middle");
        assert_eq!(alpha(0, 0), 0, "clear in the corners");
        let partial = texture
            .data
            .chunks(4)
            .filter(|p| p[3] > 0 && p[3] < 255)
            .count();
        assert!(
            partial > n,
            "a soft edge for the cutoff to eat into: {partial}"
        );
        assert_ne!(splat_texture(2).data, texture.data, "variants differ");
    }
}
