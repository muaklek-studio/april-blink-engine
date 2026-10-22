//! Global look of the scene (Godot's `WorldEnvironment`), stored as an ECS resource.

use ab_core::Color;

#[derive(Debug, Clone, PartialEq)]
pub struct Environment {
    /// Clear color behind all geometry (linear).
    pub background: Color,
    pub ambient_color: Color,
    pub ambient_energy: f32,
    pub fog: Fog,
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
            post: PostProcess::default(),
            shadows: ShadowSettings::default(),
        }
    }
}

/// Exponential height fog: thickest below `height`, thinning above it.
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
