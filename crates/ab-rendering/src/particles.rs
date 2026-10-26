//! Particle draw data. A simulation (`ab-effects`) fills the [`ParticleDrawList`] resource each
//! frame; the renderer sorts the particles back to front and draws them as camera-facing
//! quads in the HDR pass, after opaque geometry (depth-tested, fogged, tonemapped).

use ab_core::{Color, Vec3};

/// One particle, in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleInstance {
    pub position: Vec3,
    /// Quad edge length in metres.
    pub size: f32,
    /// Linear color with straight alpha. Values above 1 glow (HDR).
    pub color: Color,
    /// World velocity; with `stretch > 0` the quad is elongated along it (sparks, rain).
    pub velocity: Vec3,
    /// Extra length per m/s of speed (0 = round particle).
    pub stretch: f32,
    /// Rotation in the view plane (radians), for non-stretched particles.
    pub rotation: f32,
    /// Edge profile: 0 = very soft (smoke), 1 = hard disc.
    pub hardness: f32,
    /// Adds light instead of covering what is behind (fire, sparks, magic).
    pub additive: bool,
}

/// ECS resource: particles to draw this frame.
#[derive(Debug, Clone, Default)]
pub struct ParticleDrawList {
    pub instances: Vec<ParticleInstance>,
    /// Which emitter drew which instances (to draw some emitters alone).
    pub sources: Vec<(ab_ecs::Entity, std::ops::Range<usize>)>,
}

impl ParticleDrawList {
    pub fn clear(&mut self) {
        self.instances.clear();
        self.sources.clear();
    }

    /// The instances of the emitters `keep` accepts.
    pub fn filtered(&self, keep: impl Fn(ab_ecs::Entity) -> bool) -> Self {
        let mut out = Self::default();
        for (entity, range) in &self.sources {
            if keep(*entity) {
                let start = out.instances.len();
                out.instances
                    .extend_from_slice(&self.instances[range.clone()]);
                out.sources.push((*entity, start..out.instances.len()));
            }
        }
        out
    }
}
