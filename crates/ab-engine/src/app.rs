use crate::config::EngineConfig;
use crate::engine::{Engine, EngineControl, RenderStats};
use crate::profiler::Profiler;
use ab_platform::{LoopControl, PlatformApp, PlatformEvent, Window};
use ab_rendering::{
    GpuContext, OffscreenTarget, RenderError, RendererConfig, SceneRenderer, WindowRenderer,
};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

/// Opens the window and runs the game loop until the window closes or a quit is requested.
///
/// `setup` runs once the window and GPU exist, before the first frame; build the initial
/// scene there.
pub fn run<S>(config: EngineConfig, setup: S) -> anyhow::Result<()>
where
    S: FnOnce(&mut Engine) -> anyhow::Result<()>,
{
    info!(
        version = env!("CARGO_PKG_VERSION"),
        physics_hz = config.physics_hz,
        target_fps = ?config.window.target_fps,
        vsync = config.renderer.vsync,
        msaa = config.renderer.msaa_samples,
        "starting April Blink Engine"
    );
    let engine = Engine::new(&config);
    if let Some(mut control) = engine.tree().world().resource_mut::<EngineControl>() {
        control.set_fullscreen(config.window.fullscreen);
    }
    let app = WindowedApp {
        engine,
        setup: Some(setup),
        renderer_config: config.renderer.clone(),
        renderer: None,
        window: None,
        mouse_captured: false,
        fullscreen: config.window.fullscreen,
        last_frame: None,
        device_resets: 0,
    };
    ab_platform::run(config.window, app)
}

/// Runs `frames` frames of `delta` seconds without a window (CI smoke tests, tools,
/// server-side simulation). Stops early if a quit is requested. Nothing is rendered or heard
/// (audio is [`AudioOutput::Silent`](ab_audio::AudioOutput::Silent)).
pub fn run_headless<S>(
    config: EngineConfig,
    frames: u64,
    delta: f32,
    setup: S,
) -> anyhow::Result<Engine>
where
    S: FnOnce(&mut Engine) -> anyhow::Result<()>,
{
    let config = EngineConfig {
        audio_output: ab_audio::AudioOutput::Silent,
        ..config
    };
    let mut engine = Engine::new(&config);
    setup(&mut engine)?;
    for _ in 0..frames {
        engine.update(delta);
        if engine.is_quit_requested() {
            break;
        }
    }
    Ok(engine)
}

/// Renders the engine's current scene into an offscreen image without a window and returns
/// its sRGB RGBA8 pixels, row by row (screenshots, golden-image tests).
pub fn render_offscreen(
    engine: &Engine,
    config: &RendererConfig,
    width: u32,
    height: u32,
) -> Result<Vec<[u8; 4]>, RenderError> {
    let gpu = GpuContext::new_headless()?;
    // One frame is rendered: every texture must be there whole, not streaming in.
    let config = RendererConfig {
        texture_upload_budget: None,
        ..config.clone()
    };
    let mut renderer = SceneRenderer::new(&gpu, OffscreenTarget::FORMAT, &config);
    let target = OffscreenTarget::new(&gpu, width, height);
    let world = engine.tree().world();
    ab_ui::prepare_ui(world, width, height);
    renderer.render(&gpu, world, target.view(), width, height);
    target.read_pixels(&gpu)
}

struct WindowedApp<S> {
    engine: Engine,
    setup: Option<S>,
    renderer_config: RendererConfig,
    renderer: Option<WindowRenderer>,
    window: Option<Arc<Window>>,
    mouse_captured: bool,
    fullscreen: bool,
    last_frame: Option<Instant>,
    /// Times the renderer was rebuilt after the GPU device was lost.
    device_resets: u32,
}

/// Renderer rebuilds after lost GPU devices before the game gives up.
const MAX_DEVICE_RESETS: u32 = 3;

impl<S> WindowedApp<S> {
    fn apply_cursor_mode(&mut self) {
        let wanted = self
            .engine
            .tree()
            .world()
            .resource::<EngineControl>()
            .is_some_and(|c| c.is_mouse_captured());
        if wanted != self.mouse_captured {
            if let Some(window) = &self.window {
                ab_platform::set_cursor_captured(window, wanted);
            }
            self.mouse_captured = wanted;
        }
        let fullscreen = self
            .engine
            .tree()
            .world()
            .resource::<EngineControl>()
            .is_some_and(|c| c.is_fullscreen());
        if fullscreen != self.fullscreen {
            if let Some(window) = &self.window {
                ab_platform::set_fullscreen(window, fullscreen);
            }
            self.fullscreen = fullscreen;
        }
    }
}

impl<S> PlatformApp for WindowedApp<S>
where
    S: FnOnce(&mut Engine) -> anyhow::Result<()>,
{
    fn init(&mut self, window: Arc<Window>) -> anyhow::Result<()> {
        self.renderer = Some(WindowRenderer::new(window.clone(), &self.renderer_config)?);
        self.window = Some(window);
        if let Some(setup) = self.setup.take() {
            setup(&mut self.engine)?;
        }
        debug!(
            nodes = self.engine.tree().node_count(),
            "initial scene built"
        );
        Ok(())
    }

    fn event(&mut self, event: &PlatformEvent) {
        if let (PlatformEvent::Resized { width, height }, Some(renderer)) =
            (event, &mut self.renderer)
        {
            renderer.resize(*width, *height);
        }
        // Some platforms drop the cursor grab when the window loses focus; restore it.
        if let (PlatformEvent::Focused(true), true) = (event, self.mouse_captured) {
            self.mouse_captured = false;
        }
        self.engine.handle_event(event);
    }

    fn frame(&mut self) -> LoopControl {
        let now = Instant::now();
        let delta = self
            .last_frame
            .map_or(0.0, |last| (now - last).as_secs_f32());
        self.last_frame = Some(now);

        self.engine.update(delta);
        self.apply_cursor_mode();

        if let Some(renderer) = &mut self.renderer {
            let world = self.engine.tree().world();
            let ui_start = Instant::now();
            if let Some(window) = &self.window {
                let size = window.inner_size();
                ab_ui::prepare_ui(world, size.width, size.height);
            }
            let ui_time = ui_start.elapsed();
            let render_start = Instant::now();
            let result = renderer.render(world);
            if let Some(mut profiler) = world.resource_mut::<Profiler>() {
                profiler.record("ui_layout", ui_time);
                profiler.record("render", render_start.elapsed());
            }
            match result {
                Ok(Some(stats)) => {
                    if let Some(mut slot) = world.resource_mut::<RenderStats>() {
                        slot.0 = stats;
                    }
                }
                Ok(None) => {}
                Err(RenderError::DeviceLost) if self.device_resets < MAX_DEVICE_RESETS => {
                    self.device_resets += 1;
                    warn!(
                        attempt = self.device_resets,
                        "GPU device lost; recreating the renderer"
                    );
                    self.renderer = None;
                    if let Some(window) = self.window.clone() {
                        match WindowRenderer::new(window, &self.renderer_config) {
                            Ok(renderer) => self.renderer = Some(renderer),
                            Err(err) => {
                                error!("could not recreate the renderer: {err}");
                                return LoopControl::Exit;
                            }
                        }
                    }
                }
                Err(err) => {
                    error!("rendering failed: {err}");
                    return LoopControl::Exit;
                }
            }
        }

        if self.engine.is_quit_requested() {
            LoopControl::Exit
        } else {
            LoopControl::Continue
        }
    }

    fn shutdown(&mut self) {
        let time = self.engine.time();
        info!(
            frames = time.frame_count,
            seconds = format!("{:.1}", time.elapsed),
            "shutting down"
        );
    }
}
