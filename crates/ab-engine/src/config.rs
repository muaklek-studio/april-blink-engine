use ab_audio::AudioOutput;
use ab_input::{InputMap, KeyCode};
use ab_platform::WindowConfig;
use ab_rendering::RendererConfig;
use std::path::PathBuf;

/// Engine start-up settings.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub window: WindowConfig,
    pub renderer: RendererConfig,
    /// Fixed-step rate for `physics_process` and physics, in Hz.
    pub physics_hz: u32,
    /// Most fixed steps run in one frame; beyond this the simulation slows down instead of
    /// spiralling (each step making the next frame even later).
    pub max_physics_steps_per_frame: u32,
    /// Directory assets are loaded from. A relative path is looked up in the working
    /// directory first, then next to the executable.
    pub asset_root: PathBuf,
    /// Initial action bindings (also changeable at runtime through the `Actions` resource).
    pub input_map: InputMap,
    /// Sound output. [`run_headless`](crate::run_headless) always uses
    /// [`AudioOutput::Silent`].
    pub audio_output: AudioOutput,
    /// Key toggling the [`DebugOverlay`](crate::DebugOverlay) (`None` disables it).
    pub debug_overlay_key: Option<KeyCode>,
    /// Key toggling fullscreen (`None` disables it).
    pub fullscreen_key: Option<KeyCode>,
    /// Key toggling the physics debug view ([`DebugSettings::physics`](crate::DebugSettings)).
    pub debug_physics_key: Option<KeyCode>,
    /// Worker threads for parallel engine work ([`ab_core::jobs`]); `None` uses every
    /// logical core but one. The pool is process-wide: only the first engine's setting
    /// counts.
    pub worker_threads: Option<usize>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            renderer: RendererConfig::default(),
            physics_hz: 60,
            max_physics_steps_per_frame: 8,
            asset_root: PathBuf::from("assets"),
            input_map: InputMap::default(),
            audio_output: AudioOutput::Device,
            // Players do not get the developer views.
            debug_overlay_key: (!crate::SHIPPING).then_some(KeyCode::F3),
            debug_physics_key: (!crate::SHIPPING).then_some(KeyCode::F4),
            fullscreen_key: Some(KeyCode::F11),
            worker_threads: None,
        }
    }
}
