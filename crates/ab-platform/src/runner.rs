use crate::event::PlatformEvent;
use crate::gamepad::Gamepads;
use anyhow::Context as _;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// Main window settings.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub title: String,
    /// Logical (DPI-independent) size.
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
    /// Borderless fullscreen on the current monitor.
    pub fullscreen: bool,
    /// Frame rate cap. `None` runs as fast as possible (the renderer's vsync will pace it).
    pub target_fps: Option<u32>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "April Blink Engine".to_owned(),
            width: 1280,
            height: 720,
            resizable: true,
            fullscreen: false,
            target_fps: None,
        }
    }
}

/// Whether the loop should keep running after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopControl {
    Continue,
    Exit,
}

/// Callbacks driven by [`run`].
pub trait PlatformApp {
    /// The window exists. Create GPU surfaces and load the initial scene here.
    fn init(&mut self, window: Arc<Window>) -> anyhow::Result<()>;

    /// An input or window event arrived.
    fn event(&mut self, event: &PlatformEvent);

    /// Time to update and draw one frame.
    fn frame(&mut self) -> LoopControl;

    /// The loop is ending; release resources that need the window.
    fn shutdown(&mut self) {}
}

/// Creates the window and runs the event loop until the app exits or the window closes.
/// Returns the error from [`PlatformApp::init`] if initialisation failed.
pub fn run<A: PlatformApp>(config: WindowConfig, app: A) -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("failed to create the OS event loop")?;
    let frame_interval = frame_interval(config.target_fps);
    let mut runner = Runner {
        config,
        app,
        window: None,
        frame_interval,
        next_frame: Instant::now(),
        occluded: false,
        init_error: None,
        shut_down: false,
        gamepads: None,
    };
    event_loop
        .run_app(&mut runner)
        .context("event loop terminated with an error")?;
    match runner.init_error.take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Frame pace while the window is minimised or hidden (30 Hz).
const HIDDEN_FRAME_INTERVAL: Duration = Duration::from_micros(33_333);

/// Time between frames for a frame rate cap; `None` (uncapped) for no cap or a cap of 0.
fn frame_interval(target_fps: Option<u32>) -> Option<Duration> {
    target_fps
        .filter(|&fps| fps > 0)
        .map(|fps| Duration::from_secs_f64(1.0 / fps as f64))
}

/// The pace to use this frame: a hidden window runs at most at [`HIDDEN_FRAME_INTERVAL`].
fn pacing(hidden: bool, frame_interval: Option<Duration>) -> Option<Duration> {
    if hidden {
        Some(HIDDEN_FRAME_INTERVAL.max(frame_interval.unwrap_or_default()))
    } else {
        frame_interval
    }
}

/// When the frame after one due at `next_frame` is due. Stays on the fixed cadence, but does
/// not try to catch up after a stall.
fn next_frame_after(next_frame: Instant, interval: Duration, now: Instant) -> Instant {
    let next = next_frame + interval;
    if next < now {
        now + interval
    } else {
        next
    }
}

struct Runner<A: PlatformApp> {
    config: WindowConfig,
    app: A,
    window: Option<Arc<Window>>,
    frame_interval: Option<Duration>,
    next_frame: Instant,
    /// The compositor reported the window hidden (another window covers it, other desktop).
    occluded: bool,
    init_error: Option<anyhow::Error>,
    shut_down: bool,
    gamepads: Option<Gamepads>,
}

impl<A: PlatformApp> Runner<A> {
    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        let attributes = Window::default_attributes()
            .with_title(&self.config.title)
            .with_inner_size(LogicalSize::new(self.config.width, self.config.height))
            .with_resizable(self.config.resizable)
            .with_fullscreen(
                self.config
                    .fullscreen
                    .then_some(winit::window::Fullscreen::Borderless(None)),
            );
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("failed to create window")?,
        );
        let size = window.inner_size();
        info!(width = size.width, height = size.height, "window created");
        self.app.init(window.clone())?;
        self.window = Some(window);
        self.gamepads = Gamepads::new();
        if let Some(gamepads) = &self.gamepads {
            let app = &mut self.app;
            gamepads.connected(|event| app.event(&event));
        }
        Ok(())
    }

    fn exit(&mut self, event_loop: &ActiveEventLoop) {
        if !self.shut_down {
            self.shut_down = true;
            self.app.shutdown();
        }
        event_loop.exit();
    }
}

impl<A: PlatformApp> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Desktop platforms resume exactly once; guard anyway.
        if self.window.is_some() || self.shut_down {
            return;
        }
        if let Err(err) = self.create_window(event_loop) {
            error!("initialisation failed: {err:#}");
            self.init_error = Some(err);
            self.exit(event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if self.shut_down {
            return;
        }
        if let WindowEvent::Occluded(occluded) = event {
            self.occluded = occluded;
        }
        if let WindowEvent::RedrawRequested = event {
            if self.app.frame() == LoopControl::Exit {
                self.exit(event_loop);
            }
            return;
        }
        if let Some(platform_event) = PlatformEvent::from_window_event(&event) {
            self.app.event(&platform_event);
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if self.shut_down {
            return;
        }
        if let Some(platform_event) = PlatformEvent::from_device_event(&event) {
            self.app.event(&platform_event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let (Some(gamepads), false) = (&mut self.gamepads, self.shut_down) {
            let app = &mut self.app;
            gamepads.poll(|event| app.event(&event));
        }
        let Some(window) = &self.window else {
            return;
        };
        // Nothing to show: keep the game running, but at a gentle pace instead of spinning a
        // core flat out (vsync no longer throttles a window that does not present).
        let hidden = self.occluded
            || window.is_minimized() == Some(true)
            || window.inner_size().width == 0
            || window.inner_size().height == 0;
        match pacing(hidden, self.frame_interval) {
            None => {
                window.request_redraw();
                event_loop.set_control_flow(ControlFlow::Poll);
            }
            Some(interval) => {
                let now = Instant::now();
                if now >= self.next_frame {
                    window.request_redraw();
                    self.next_frame = next_frame_after(self.next_frame, interval, now);
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
            }
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        if !self.shut_down {
            self.shut_down = true;
            self.app.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_config_defaults() {
        let config = WindowConfig::default();
        assert_eq!(config.title, "April Blink Engine");
        assert_eq!((config.width, config.height), (1280, 720));
        assert!(config.resizable);
        assert!(!config.fullscreen);
        assert_eq!(config.target_fps, None);
    }

    #[test]
    fn frame_cap_becomes_an_interval() {
        assert_eq!(frame_interval(None), None);
        assert_eq!(frame_interval(Some(0)), None);
        assert_eq!(frame_interval(Some(1)), Some(Duration::from_secs(1)));
        let interval = frame_interval(Some(60)).unwrap();
        assert!((interval.as_secs_f64() - 1.0 / 60.0).abs() < 1e-9);
    }

    #[test]
    fn hidden_windows_run_at_most_at_30_hz() {
        let fast = frame_interval(Some(144));
        assert_eq!(pacing(false, None), None);
        assert_eq!(pacing(false, fast), fast);
        assert_eq!(pacing(true, None), Some(HIDDEN_FRAME_INTERVAL));
        assert_eq!(pacing(true, fast), Some(HIDDEN_FRAME_INTERVAL));
        // A cap slower than 30 Hz is kept.
        let slow = frame_interval(Some(10));
        assert_eq!(pacing(true, slow), slow);
    }

    #[test]
    fn frames_stay_on_cadence() {
        let start = Instant::now();
        let interval = Duration::from_millis(10);
        // Woken a little late: the next frame is still on the original grid.
        let late = start + Duration::from_millis(3);
        assert_eq!(next_frame_after(start, interval, late), start + interval);
    }

    #[test]
    fn frames_do_not_catch_up_after_a_stall() {
        let start = Instant::now();
        let interval = Duration::from_millis(10);
        let stalled = start + Duration::from_millis(250);
        assert_eq!(
            next_frame_after(start, interval, stalled),
            stalled + interval
        );
    }
}
