//! What a particle effect looks like, with ready-made presets.

use ab_core::{Color, Vec3};
use serde::{Deserialize, Serialize};

/// Where new particles appear, around the emitter's origin (local space).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmissionShape {
    Point,
    /// Inside a sphere.
    Sphere {
        radius: f32,
    },
    /// Inside a box with these full extents.
    Box {
        size: Vec3,
    },
    /// On a flat ring in the XZ plane.
    Ring {
        radius: f32,
    },
}

/// Color and opacity over a particle's life: linear interpolation between stops at
/// `t` = 0..1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorRamp(pub Vec<(f32, Color)>);

impl ColorRamp {
    /// From `start` to `end`.
    pub fn two(start: Color, end: Color) -> Self {
        Self(vec![(0.0, start), (1.0, end)])
    }

    pub fn sample(&self, t: f32) -> Color {
        let stops = &self.0;
        let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
            return Color::WHITE;
        };
        if t <= first.0 {
            return first.1;
        }
        if t >= last.0 {
            return last.1;
        }
        let i = stops
            .iter()
            .position(|(at, _)| *at > t)
            .unwrap_or(stops.len() - 1);
        let (a, b) = (stops[i - 1], stops[i]);
        let k = ((t - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
        let lerp = |x: f32, y: f32| x + (y - x) * k;
        Color::rgba(
            lerp(a.1.r, b.1.r),
            lerp(a.1.g, b.1.g),
            lerp(a.1.b, b.1.b),
            lerp(a.1.a, b.1.a),
        )
    }
}

/// Everything about how particles are born, move and look. Start from a preset
/// ([`ParticleSettings::smoke`], [`fire`](ParticleSettings::fire), ...) and adjust with the
/// `with_*` builders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParticleSettings {
    /// Particles alive at once (continuous emitters) or emitted at once (one-shot).
    pub amount: u32,
    /// Seconds a particle lives.
    pub lifetime: f32,
    /// 0..1: how much lifetimes vary.
    pub lifetime_random: f32,
    pub shape: EmissionShape,
    /// Initial direction (emitter's local space).
    pub direction: Vec3,
    /// Cone half-angle around `direction`, in degrees (180 = every direction).
    pub spread: f32,
    /// Initial speed range in m/s.
    pub speed: [f32; 2],
    /// Acceleration in m/s² (world space). Positive Y makes smoke rise.
    pub gravity: Vec3,
    /// Velocity damping per second (air drag).
    pub damping: f32,
    /// Random swirling acceleration in m/s².
    pub turbulence: f32,
    /// Size at birth and at death, in metres.
    pub size: [f32; 2],
    /// 0..1: how much sizes vary.
    pub size_random: f32,
    pub color: ColorRamp,
    /// Brightness multiplier; above 1 the effect glows (HDR + bloom-like tonemapping).
    pub emission: f32,
    /// Spin speed range in radians per second.
    pub spin: [f32; 2],
    /// Edge profile: 0 = very soft (smoke), 1 = hard dot.
    pub hardness: f32,
    /// Elongation along the velocity (sparks, rain), per m/s.
    pub stretch: f32,
    /// Adds light (fire, sparks, magic) instead of covering what is behind (smoke, dust).
    pub additive: bool,
}

impl Default for ParticleSettings {
    fn default() -> Self {
        Self {
            amount: 32,
            lifetime: 1.0,
            lifetime_random: 0.0,
            shape: EmissionShape::Point,
            direction: Vec3::Y,
            spread: 15.0,
            speed: [1.0, 1.0],
            gravity: Vec3::ZERO,
            damping: 0.0,
            turbulence: 0.0,
            size: [0.2, 0.2],
            size_random: 0.0,
            color: ColorRamp::two(Color::WHITE, Color::rgba(1.0, 1.0, 1.0, 0.0)),
            emission: 1.0,
            spin: [0.0, 0.0],
            hardness: 0.3,
            stretch: 0.0,
            additive: false,
        }
    }
}

const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
    Color::rgba(r, g, b, a)
}

impl ParticleSettings {
    /// Slow, rising, swirling grey smoke.
    pub fn smoke() -> Self {
        Self {
            amount: 40,
            lifetime: 4.0,
            lifetime_random: 0.3,
            shape: EmissionShape::Sphere { radius: 0.2 },
            spread: 20.0,
            speed: [0.3, 0.7],
            gravity: Vec3::new(0.0, 0.15, 0.0),
            damping: 0.3,
            turbulence: 0.35,
            size: [0.4, 2.2],
            size_random: 0.3,
            color: ColorRamp(vec![
                (0.0, rgba(0.35, 0.35, 0.37, 0.0)),
                (0.15, rgba(0.32, 0.32, 0.34, 0.45)),
                (1.0, rgba(0.25, 0.25, 0.27, 0.0)),
            ]),
            spin: [-0.4, 0.4],
            hardness: 0.0,
            ..Self::default()
        }
    }

    /// Flickering flames.
    pub fn fire() -> Self {
        Self {
            amount: 60,
            lifetime: 0.9,
            lifetime_random: 0.4,
            shape: EmissionShape::Sphere { radius: 0.15 },
            spread: 15.0,
            speed: [0.8, 1.6],
            gravity: Vec3::new(0.0, 1.2, 0.0),
            turbulence: 0.6,
            size: [0.6, 0.12],
            size_random: 0.3,
            color: ColorRamp(vec![
                (0.0, rgba(1.0, 0.75, 0.3, 0.0)),
                (0.15, rgba(1.0, 0.45, 0.08, 0.9)),
                (0.6, rgba(0.85, 0.15, 0.02, 0.55)),
                (1.0, rgba(0.2, 0.05, 0.02, 0.0)),
            ]),
            emission: 1.4,
            spin: [-1.0, 1.0],
            hardness: 0.2,
            additive: true,
            ..Self::default()
        }
    }

    /// Glowing embers drifting up from a fire.
    pub fn embers() -> Self {
        Self {
            amount: 20,
            lifetime: 2.5,
            lifetime_random: 0.5,
            shape: EmissionShape::Sphere { radius: 0.2 },
            spread: 25.0,
            speed: [0.5, 1.5],
            gravity: Vec3::new(0.0, 0.3, 0.0),
            turbulence: 0.8,
            size: [0.05, 0.03],
            color: ColorRamp(vec![
                (0.0, rgba(1.0, 0.7, 0.3, 1.0)),
                (0.7, rgba(1.0, 0.35, 0.1, 0.9)),
                (1.0, rgba(0.6, 0.1, 0.0, 0.0)),
            ]),
            emission: 2.0,
            hardness: 0.7,
            additive: true,
            ..Self::default()
        }
    }

    /// A burst of hot sparks that fall and streak (use one-shot).
    pub fn sparks() -> Self {
        Self {
            amount: 40,
            lifetime: 0.8,
            lifetime_random: 0.4,
            shape: EmissionShape::Sphere { radius: 0.05 },
            spread: 70.0,
            speed: [3.0, 7.0],
            gravity: Vec3::new(0.0, -9.81, 0.0),
            damping: 0.5,
            size: [0.05, 0.02],
            color: ColorRamp(vec![
                (0.0, rgba(1.0, 0.95, 0.7, 1.0)),
                (0.5, rgba(1.0, 0.6, 0.2, 1.0)),
                (1.0, rgba(1.0, 0.3, 0.05, 0.0)),
            ]),
            emission: 3.0,
            hardness: 0.6,
            stretch: 0.08,
            additive: true,
            ..Self::default()
        }
    }

    /// Dust motes hanging in the air of a room.
    pub fn dust() -> Self {
        Self {
            amount: 30,
            lifetime: 6.0,
            lifetime_random: 0.4,
            shape: EmissionShape::Box {
                size: Vec3::new(3.0, 1.5, 3.0),
            },
            spread: 180.0,
            speed: [0.02, 0.08],
            turbulence: 0.1,
            size: [0.04, 0.04],
            size_random: 0.5,
            color: ColorRamp(vec![
                (0.0, rgba(0.9, 0.85, 0.75, 0.0)),
                (0.3, rgba(0.9, 0.85, 0.75, 0.3)),
                (1.0, rgba(0.9, 0.85, 0.75, 0.0)),
            ]),
            hardness: 0.3,
            ..Self::default()
        }
    }

    /// Sparkling magic swirling upwards.
    pub fn magic() -> Self {
        Self {
            amount: 50,
            lifetime: 1.5,
            lifetime_random: 0.3,
            shape: EmissionShape::Sphere { radius: 0.4 },
            spread: 180.0,
            speed: [0.2, 0.6],
            gravity: Vec3::new(0.0, 0.5, 0.0),
            turbulence: 1.0,
            size: [0.12, 0.0],
            color: ColorRamp(vec![
                (0.0, rgba(0.2, 0.7, 1.0, 1.0)),
                (1.0, rgba(0.6, 0.15, 1.0, 0.0)),
            ]),
            emission: 1.5,
            hardness: 0.5,
            additive: true,
            ..Self::default()
        }
    }

    /// Pale, slow wisps trailing a ghost.
    pub fn wisps() -> Self {
        Self {
            amount: 35,
            lifetime: 1.8,
            lifetime_random: 0.3,
            shape: EmissionShape::Sphere { radius: 0.35 },
            spread: 180.0,
            speed: [0.05, 0.2],
            gravity: Vec3::new(0.0, 0.25, 0.0),
            turbulence: 0.5,
            size: [0.5, 0.9],
            color: ColorRamp(vec![
                (0.0, rgba(0.6, 0.75, 1.0, 0.0)),
                (0.3, rgba(0.45, 0.6, 1.0, 0.18)),
                (1.0, rgba(0.5, 0.6, 1.0, 0.0)),
            ]),
            emission: 0.7,
            hardness: 0.0,
            additive: true,
            ..Self::default()
        }
    }

    /// The fireball of an explosion (use one-shot; see `spawn_explosion`).
    pub fn explosion() -> Self {
        Self {
            amount: 50,
            lifetime: 0.8,
            lifetime_random: 0.4,
            shape: EmissionShape::Sphere { radius: 0.3 },
            spread: 180.0,
            speed: [2.0, 5.0],
            damping: 3.0,
            size: [0.8, 2.4],
            size_random: 0.3,
            color: ColorRamp(vec![
                (0.0, rgba(1.0, 0.8, 0.45, 1.0)),
                (0.3, rgba(1.0, 0.4, 0.06, 0.8)),
                (1.0, rgba(0.3, 0.05, 0.0, 0.0)),
            ]),
            emission: 1.8,
            spin: [-2.0, 2.0],
            hardness: 0.1,
            additive: true,
            ..Self::default()
        }
    }

    /// Rain over a 20 × 20 m area above the emitter (attach it to the camera).
    pub fn rain() -> Self {
        Self {
            amount: 600,
            lifetime: 1.2,
            shape: EmissionShape::Box {
                size: Vec3::new(20.0, 0.0, 20.0),
            },
            direction: Vec3::NEG_Y,
            spread: 3.0,
            speed: [9.0, 11.0],
            size: [0.03, 0.03],
            color: ColorRamp::two(rgba(0.7, 0.75, 0.85, 0.5), rgba(0.7, 0.75, 0.85, 0.4)),
            hardness: 0.5,
            stretch: 0.06,
            ..Self::default()
        }
    }

    /// Snow drifting down over a 20 × 20 m area.
    pub fn snow() -> Self {
        Self {
            amount: 400,
            lifetime: 8.0,
            shape: EmissionShape::Box {
                size: Vec3::new(20.0, 0.0, 20.0),
            },
            direction: Vec3::NEG_Y,
            spread: 10.0,
            speed: [0.6, 1.2],
            turbulence: 0.4,
            size: [0.05, 0.05],
            size_random: 0.4,
            color: ColorRamp::two(rgba(1.0, 1.0, 1.0, 0.85), rgba(1.0, 1.0, 1.0, 0.6)),
            hardness: 0.6,
            ..Self::default()
        }
    }

    /// Green motes and soft glow rising from the ground: healing, buffs, save points.
    pub fn heal() -> Self {
        Self {
            amount: 40,
            lifetime: 1.4,
            lifetime_random: 0.3,
            shape: EmissionShape::Ring { radius: 0.5 },
            direction: Vec3::Y,
            spread: 10.0,
            speed: [0.6, 1.2],
            damping: 0.8,
            turbulence: 0.3,
            size: [0.1, 0.02],
            size_random: 0.4,
            color: ColorRamp(vec![
                (0.0, rgba(0.4, 1.0, 0.5, 0.0)),
                (0.2, rgba(0.45, 1.0, 0.55, 1.0)),
                (1.0, rgba(0.9, 1.0, 0.6, 0.0)),
            ]),
            emission: 2.0,
            hardness: 0.6,
            additive: true,
            ..Self::default()
        }
    }

    /// A few warm glowing dots drifting slowly in a 6 × 2 × 6 m volume (night ambience).
    pub fn fireflies() -> Self {
        Self {
            amount: 24,
            lifetime: 6.0,
            lifetime_random: 0.5,
            shape: EmissionShape::Box {
                size: Vec3::new(6.0, 2.0, 6.0),
            },
            spread: 180.0,
            speed: [0.05, 0.2],
            turbulence: 0.6,
            size: [0.05, 0.05],
            size_random: 0.3,
            color: ColorRamp(vec![
                (0.0, rgba(0.9, 1.0, 0.3, 0.0)),
                (0.2, rgba(0.9, 1.0, 0.35, 1.0)),
                (0.5, rgba(0.8, 1.0, 0.3, 0.3)),
                (0.8, rgba(0.9, 1.0, 0.35, 1.0)),
                (1.0, rgba(0.9, 1.0, 0.3, 0.0)),
            ]),
            emission: 3.0,
            hardness: 0.7,
            additive: true,
            ..Self::default()
        }
    }

    /// Water droplets thrown up and falling back (use one-shot: footsteps in water,
    /// something landing in a pond).
    pub fn splash() -> Self {
        Self {
            amount: 40,
            lifetime: 0.8,
            lifetime_random: 0.3,
            shape: EmissionShape::Ring { radius: 0.15 },
            direction: Vec3::Y,
            spread: 35.0,
            speed: [1.5, 3.5],
            gravity: Vec3::new(0.0, -9.8, 0.0),
            size: [0.06, 0.03],
            size_random: 0.4,
            color: ColorRamp::two(rgba(0.75, 0.85, 1.0, 0.8), rgba(0.7, 0.8, 0.95, 0.0)),
            hardness: 0.6,
            stretch: 0.03,
            ..Self::default()
        }
    }

    /// Autumn leaves tumbling down over a 10 × 10 m area.
    pub fn leaves() -> Self {
        Self {
            amount: 40,
            lifetime: 6.0,
            lifetime_random: 0.3,
            shape: EmissionShape::Box {
                size: Vec3::new(10.0, 0.0, 10.0),
            },
            direction: Vec3::NEG_Y,
            spread: 20.0,
            speed: [0.4, 0.9],
            turbulence: 1.2,
            size: [0.12, 0.12],
            size_random: 0.3,
            color: ColorRamp(vec![
                (0.0, rgba(0.8, 0.35, 0.08, 0.0)),
                (0.1, rgba(0.8, 0.4, 0.1, 1.0)),
                (0.9, rgba(0.55, 0.25, 0.05, 1.0)),
                (1.0, rgba(0.5, 0.2, 0.05, 0.0)),
            ]),
            spin: [-3.0, 3.0],
            hardness: 0.9,
            ..Self::default()
        }
    }

    /// Droplets of blood thrown from a wound, arcing down and streaking (use one-shot, aimed
    /// along the hit with `with_direction`; `spawn_blood` in the engine adds a mist and
    /// splats on the ground).
    pub fn blood() -> Self {
        Self {
            amount: 45,
            lifetime: 0.9,
            lifetime_random: 0.4,
            shape: EmissionShape::Sphere { radius: 0.04 },
            spread: 28.0,
            speed: [2.0, 5.5],
            gravity: Vec3::new(0.0, -9.81, 0.0),
            damping: 0.6,
            size: [0.045, 0.02],
            size_random: 0.6,
            color: ColorRamp(vec![
                (0.0, rgba(0.42, 0.01, 0.015, 1.0)),
                (0.7, rgba(0.3, 0.0, 0.01, 1.0)),
                (1.0, rgba(0.2, 0.0, 0.005, 0.0)),
            ]),
            hardness: 0.75,
            stretch: 0.045,
            ..Self::default()
        }
    }

    /// The fine red spray of a hit, hanging in the air for a moment (use one-shot with
    /// [`ParticleSettings::blood`]).
    pub fn blood_mist() -> Self {
        Self {
            amount: 14,
            lifetime: 0.7,
            lifetime_random: 0.3,
            shape: EmissionShape::Sphere { radius: 0.06 },
            spread: 40.0,
            speed: [0.6, 1.8],
            gravity: Vec3::new(0.0, -0.6, 0.0),
            damping: 3.5,
            turbulence: 0.4,
            size: [0.12, 0.45],
            size_random: 0.4,
            color: ColorRamp(vec![
                (0.0, rgba(0.38, 0.02, 0.02, 0.0)),
                (0.12, rgba(0.36, 0.02, 0.02, 0.45)),
                (1.0, rgba(0.25, 0.01, 0.01, 0.0)),
            ]),
            spin: [-1.0, 1.0],
            hardness: 0.0,
            ..Self::default()
        }
    }

    /// A preset by name (one of [`ParticleSettings::PRESETS`]), for scene files and tools.
    pub fn preset(name: &str) -> Option<Self> {
        Some(match name {
            "smoke" => Self::smoke(),
            "fire" => Self::fire(),
            "embers" => Self::embers(),
            "sparks" => Self::sparks(),
            "dust" => Self::dust(),
            "magic" => Self::magic(),
            "wisps" => Self::wisps(),
            "explosion" => Self::explosion(),
            "rain" => Self::rain(),
            "snow" => Self::snow(),
            "heal" => Self::heal(),
            "fireflies" => Self::fireflies(),
            "splash" => Self::splash(),
            "leaves" => Self::leaves(),
            "blood" => Self::blood(),
            "blood_mist" => Self::blood_mist(),
            _ => return None,
        })
    }

    /// Names accepted by [`ParticleSettings::preset`].
    pub const PRESETS: [&'static str; 16] = [
        "smoke",
        "fire",
        "embers",
        "sparks",
        "dust",
        "magic",
        "wisps",
        "explosion",
        "rain",
        "snow",
        "heal",
        "fireflies",
        "splash",
        "leaves",
        "blood",
        "blood_mist",
    ];

    pub fn with_amount(mut self, amount: u32) -> Self {
        self.amount = amount;
        self
    }

    pub fn with_lifetime(mut self, lifetime: f32) -> Self {
        self.lifetime = lifetime;
        self
    }

    pub fn with_size(mut self, start: f32, end: f32) -> Self {
        self.size = [start, end];
        self
    }

    pub fn with_speed(mut self, min: f32, max: f32) -> Self {
        self.speed = [min, max];
        self
    }

    /// Replaces the color ramp with a fade from `start` to `end`.
    pub fn with_colors(mut self, start: Color, end: Color) -> Self {
        self.color = ColorRamp::two(start, end);
        self
    }

    pub fn with_emission(mut self, emission: f32) -> Self {
        self.emission = emission;
        self
    }

    pub fn with_shape(mut self, shape: EmissionShape) -> Self {
        self.shape = shape;
        self
    }

    pub fn with_gravity(mut self, gravity: Vec3) -> Self {
        self.gravity = gravity;
        self
    }

    pub fn with_direction(mut self, direction: Vec3, spread: f32) -> Self {
        self.direction = direction;
        self.spread = spread;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_interpolates_and_clamps() {
        let ramp = ColorRamp(vec![
            (0.0, Color::rgba(0.0, 0.0, 0.0, 0.0)),
            (0.5, Color::rgba(1.0, 0.0, 0.0, 1.0)),
            (1.0, Color::rgba(1.0, 1.0, 0.0, 0.0)),
        ]);
        assert_eq!(ramp.sample(-1.0), Color::rgba(0.0, 0.0, 0.0, 0.0));
        let quarter = ramp.sample(0.25);
        assert!((quarter.r - 0.5).abs() < 1e-6 && (quarter.a - 0.5).abs() < 1e-6);
        let late = ramp.sample(0.75);
        assert!((late.g - 0.5).abs() < 1e-6);
        assert_eq!(ramp.sample(2.0), Color::rgba(1.0, 1.0, 0.0, 0.0));
        assert_eq!(ColorRamp(Vec::new()).sample(0.5), Color::WHITE);
    }

    #[test]
    fn presets_by_name() {
        for name in ParticleSettings::PRESETS {
            let preset = ParticleSettings::preset(name).unwrap();
            assert!(preset.amount > 0 && preset.lifetime > 0.0, "{name}");
        }
        assert!(ParticleSettings::preset("teapot").is_none());
        assert!(ParticleSettings::fire().additive && !ParticleSettings::smoke().additive);
    }
}
