use crate::emitter::ParticleEmitter;
use crate::screen::ScreenEffects;
use crate::settings::ParticleSettings;
use crate::system::{shake_cameras, CameraShake, LightFlash};
use ab_core::{Color, Vec3};
use ab_rendering::{OmniLight3D, RenderNodes};
use ab_scene::{NodeHandle, NodeScript, SceneError, SceneTree, Transform};

/// Class names.
pub mod class {
    pub const PARTICLES_3D: &str = "Particles3D";
    pub const CAMERA_SHAKE: &str = "CameraShake";
}

/// Frees its node once every one-shot emitter and light flash in its subtree is done.
#[derive(Debug, Default)]
struct FreeWhenFinished;

impl NodeScript for FreeWhenFinished {
    fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, _delta: f32) {
        let done = tree.subtree(node).into_iter().all(|n| {
            tree.component::<ParticleEmitter>(n)
                .is_none_or(|e| e.is_finished() || !e.one_shot)
                && tree
                    .component::<LightFlash>(n)
                    .is_none_or(|f| f.is_finished())
        });
        if done {
            let _ = tree.queue_free(node);
        }
    }
}

/// Frees `node` once every one-shot emitter and light flash in its subtree is done: an effect
/// scene played once.
pub fn free_when_finished(tree: &mut SceneTree, node: NodeHandle) -> Result<(), SceneError> {
    tree.set_script(node, FreeWhenFinished)
}

/// Options for [`EffectNodes::spawn_explosion`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExplosionOptions {
    /// Overall size multiplier.
    pub scale: f32,
    /// Peak energy of the flash light (0 = no light).
    pub flash: f32,
    /// Trauma added to every [`CameraShake`].
    pub shake: f32,
    /// Chromatic-aberration kick through [`ScreenEffects`] (0 = none).
    pub distortion: f32,
}

impl Default for ExplosionOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            flash: 40.0,
            shake: 0.6,
            distortion: 1.0,
        }
    }
}

/// Effect constructors on [`SceneTree`] (`use ab_effects::EffectNodes;`).
pub trait EffectNodes {
    /// A detached `Particles3D` node emitting continuously.
    fn create_particles3d(&mut self, name: &str, emitter: ParticleEmitter) -> NodeHandle;

    /// A detached `CameraShake` pivot; put the camera under it.
    fn create_camera_shake(&mut self, name: &str) -> NodeHandle;

    /// Plays a one-shot effect at a position under `parent` and frees it when done
    /// (impacts, pickups). The settings' `amount` particles are emitted at once.
    fn spawn_effect(
        &mut self,
        parent: NodeHandle,
        position: Vec3,
        settings: ParticleSettings,
    ) -> Result<NodeHandle, SceneError>;

    /// Fireball, smoke, sparks, a fading flash of light and camera shake, freed when done.
    fn spawn_explosion(
        &mut self,
        parent: NodeHandle,
        position: Vec3,
        options: ExplosionOptions,
    ) -> Result<NodeHandle, SceneError>;
}

impl EffectNodes for SceneTree {
    fn create_particles3d(&mut self, name: &str, emitter: ParticleEmitter) -> NodeHandle {
        let seed = 0x9e37_79b9 ^ (self.node_count() as u64).wrapping_mul(0x2545_f491);
        self.spawn_node(
            name,
            class::PARTICLES_3D,
            (Transform::default(), emitter.with_seed(seed)),
        )
    }

    fn create_camera_shake(&mut self, name: &str) -> NodeHandle {
        self.spawn_node(
            name,
            class::CAMERA_SHAKE,
            (Transform::default(), CameraShake::default()),
        )
    }

    fn spawn_effect(
        &mut self,
        parent: NodeHandle,
        position: Vec3,
        settings: ParticleSettings,
    ) -> Result<NodeHandle, SceneError> {
        let node = self.create_particles3d("Effect", ParticleEmitter::new(settings).one_shot());
        self.set_script(node, FreeWhenFinished)?;
        self.add_child(parent, node)?;
        self.set_global_position(node, position)?;
        Ok(node)
    }

    fn spawn_explosion(
        &mut self,
        parent: NodeHandle,
        position: Vec3,
        options: ExplosionOptions,
    ) -> Result<NodeHandle, SceneError> {
        let s = options.scale.max(0.01);
        let root = self.create_node3d("Explosion");
        self.set_script(root, FreeWhenFinished)?;
        self.add_child(parent, root)?;
        self.set_global_position(root, position)?;
        let scaled = |mut settings: ParticleSettings| {
            settings.size = settings.size.map(|v| v * s);
            settings.speed = settings.speed.map(|v| v * s);
            settings
        };
        let smoke = ParticleSettings {
            lifetime: 2.5,
            spread: 180.0,
            speed: [0.5, 1.5],
            damping: 1.0,
            ..ParticleSettings::smoke().with_amount(25)
        };
        for (name, settings) in [
            ("Fireball", ParticleSettings::explosion()),
            ("Smoke", smoke),
            (
                "Sparks",
                ParticleSettings::sparks().with_direction(Vec3::Y, 90.0),
            ),
        ] {
            let node =
                self.create_particles3d(name, ParticleEmitter::new(scaled(settings)).one_shot());
            self.add_child(root, node)?;
        }
        if options.flash > 0.0 {
            let light = self.create_omni_light3d(
                "Flash",
                OmniLight3D {
                    color: Color::rgb(1.0, 0.6, 0.25),
                    energy: options.flash,
                    range: 8.0 * s,
                },
            );
            self.insert_component(light, LightFlash::new(options.flash, 0.5))?;
            self.add_child(root, light)?;
        }
        if options.shake > 0.0 {
            shake_cameras(self.world(), options.shake);
        }
        if options.distortion > 0.0 {
            if let Some(mut screen) = self.world().resource_mut::<ScreenEffects>() {
                screen.distort(options.distortion, 0.5);
            }
        }
        Ok(root)
    }
}
