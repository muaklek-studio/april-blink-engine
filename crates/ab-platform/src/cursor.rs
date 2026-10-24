use winit::window::{CursorGrabMode, Window};

/// Switches between borderless fullscreen (on the window's monitor) and a window.
pub fn set_fullscreen(window: &Window, fullscreen: bool) {
    window.set_fullscreen(fullscreen.then_some(winit::window::Fullscreen::Borderless(None)));
}

/// Captures (hides and locks) or releases the mouse cursor, for FPS-style mouse look.
///
/// Uses `Locked` where supported (Wayland, macOS) and falls back to `Confined` (X11,
/// Windows); raw mouse motion keeps arriving either way. Returns `false` if the platform
/// refused both.
pub fn set_cursor_captured(window: &Window, captured: bool) -> bool {
    if !captured {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        window.set_cursor_visible(true);
        return true;
    }
    let grabbed = window
        .set_cursor_grab(CursorGrabMode::Locked)
        .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
        .is_ok();
    if grabbed {
        window.set_cursor_visible(false);
    } else {
        tracing::warn!("this platform does not allow capturing the cursor");
    }
    grabbed
}
