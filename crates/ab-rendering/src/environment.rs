//! Global look of the scene (Godot's `WorldEnvironment`), stored as an ECS resource.

use ab_core::{Color, Vec3};

#[derive(Debug, Clone, PartialEq)]
pub struct Environment {
    /// Clear color behind all geometry (linear).
    pub background: Color,
    pub ambient_color: Color,
    pub ambient_energy: f32,
    pub fog: Fog,
    pub light_shafts: LightShafts,
    pub post: PostProcess,
    pub shadows: ShadowSettings,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            background: Color::rgb(0.05, 0.06, 0.08),
            ambient_color: Color::WHITE,
            ambient_energy: 0.15,
            fog: Fog::default(),
            light_shafts: LightShafts::default(),
            post: PostProcess::default(),
            shadows: ShadowSettings::default(),
        }
    }
}

/// Volumetric light: beams where the sun and spot lights shine through openings (windows,
/// gaps between trees, a flashlight's cone in dusty air), shadowed by everything that casts
/// shadows. Traced through the shadow maps at half resolution; lights without shadows light
/// the air evenly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightShafts {
    pub enabled: bool,
    /// Brightness of the beams.
    pub intensity: f32,
    /// How thick the air is, per metre. With fog on, the fog's density adds to it.
    pub density: f32,
    /// How much light scatters forwards: 0 every way, 0.8 bright looking towards the light.
    pub anisotropy: f32,
    /// How far from the camera the beams are traced, in metres.
    pub max_distance: f32,
}

impl Default for LightShafts {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 1.0,
            density: 0.03,
            anisotropy: 0.6,
            max_distance: 60.0,
        }
    }
}

/// Exponential height fog: thickest below `height`, thinning above it, glowing towards the
/// sun, and broken into patches that the wind blows along.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub enabled: bool,
    pub color: Color,
    /// Extinction per metre at `height`.
    pub density: f32,
    /// World height where fog has `density`.
    pub height: f32,
    /// How fast fog thins above `height` (per metre). 0 = uniform fog.
    pub height_falloff: f32,
    /// Upper bound on fog opacity (keeps distant silhouettes faintly visible).
    pub max_opacity: f32,
    /// Brightness of the sun's light scattered by the fog towards the camera (0 = none).
    pub sun_scattering: f32,
    /// How tightly that glow gathers around the sun: 0 even, 0.9 a tight halo.
    pub sun_scattering_anisotropy: f32,
    /// How uneven the fog is near the camera: 0 even, 1 clear gaps between thick banks.
    pub patchiness: f32,
    /// Size of the patches in metres.
    pub patch_size: f32,
    /// Wind that blows the patches along, in metres per second.
    pub wind: Vec3,
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            enabled: false,
            color: Color::rgb(0.5, 0.6, 0.7),
            density: 0.02,
            height: 0.0,
            height_falloff: 0.0,
            max_opacity: 1.0,
            sun_scattering: 0.35,
            sun_scattering_anisotropy: 0.65,
            patchiness: 0.0,
            patch_size: 14.0,
            wind: Vec3::new(1.2, 0.0, 0.5),
        }
    }
}

/// Final full-screen pass: bloom, exposure, tonemapping, simple color grading, vignette,
/// chromatic aberration and film grain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PostProcess {
    pub exposure: f32,
    /// ACES filmic tonemapping; when off, colors are clamped.
    pub tonemap: bool,
    /// 1 = neutral.
    pub contrast: f32,
    /// 1 = neutral, 0 = greyscale.
    pub saturation: f32,
    /// Multiplied into the final color (e.g. a cold blue for night scenes).
    pub tint: Color,
    /// 0 = off. Darkening at the screen corners.
    pub vignette_intensity: f32,
    /// Size of the vignette's transition (0..1).
    pub vignette_smoothness: f32,
    /// Glow around bright (HDR) pixels: emissive materials, fire, magic, lamps.
    pub bloom: Bloom,
    /// 0 = off. Red and blue split towards the screen edges (about 0..1).
    pub chromatic_aberration: f32,
    /// 0 = off. Animated film grain (about 0..1).
    pub grain: f32,
}

/// Bloom settings of [`PostProcess`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bloom {
    /// 0 = off. How much of the blurred bright light is added (0.05..0.3 looks natural).
    pub intensity: f32,
    /// Brightness (HDR, before exposure) where pixels start to glow. 1 = only things brighter
    /// than white, lower values make the whole scene softer.
    pub threshold: f32,
    /// 0..1: how gradually pixels near the threshold start to glow.
    pub soft_knee: f32,
}

impl Default for Bloom {
    fn default() -> Self {
        Self {
            intensity: 0.0,
            threshold: 1.0,
            soft_knee: 0.5,
        }
    }
}

impl Bloom {
    pub fn is_enabled(&self) -> bool {
        self.intensity > 0.0 && self.intensity.is_finite()
    }
}

/// Short-lived full-screen effects layered over the [`Environment`]'s post-processing:
/// damage flashes, fades to black, impact distortion. Gameplay owns this resource (usually
/// through `ab_effects::ScreenEffects`), so it never overwrites the scene's authored look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenOverlay {
    /// Mixed over the final image; `a` is the amount (0 = invisible, 1 = solid color).
    pub color: Color,
    /// Added to [`PostProcess::chromatic_aberration`].
    pub chromatic_aberration: f32,
}

impl Default for ScreenOverlay {
    fn default() -> Self {
        Self {
            color: Color::TRANSPARENT,
            chromatic_aberration: 0.0,
        }
    }
}

impl Default for PostProcess {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            tonemap: true,
            contrast: 1.0,
            saturation: 1.0,
            tint: Color::WHITE,
            vignette_intensity: 0.0,
            vignette_smoothness: 0.5,
            bloom: Bloom::default(),
            chromatic_aberration: 0.0,
            grain: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowSettings {
    /// How far from the camera the directional shadow reaches, in metres.
    pub directional_distance: f32,
    /// Constant depth bias (in shadow-map depth units).
    pub depth_bias: f32,
    /// Offset along the surface normal, in shadow-map texels.
    pub normal_bias: f32,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            directional_distance: 40.0,
            depth_bias: 0.0015,
            normal_bias: 1.5,
        }
    }
}
