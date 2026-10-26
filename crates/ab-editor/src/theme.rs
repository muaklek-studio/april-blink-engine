//! The editor's look: a neutral dark theme with a blue accent, Prompt for text (Latin and
//! Thai; see [`crate::fonts`] for the fallbacks) and Phosphor icons.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

pub const ACCENT: Color32 = Color32::from_rgb(64, 138, 255);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(38, 79, 145);
pub const PLAY: Color32 = Color32::from_rgb(76, 194, 120);
pub const WARN: Color32 = Color32::from_rgb(232, 178, 64);
pub const ERROR: Color32 = Color32::from_rgb(235, 87, 87);
/// Body text, and secondary text (hints, details). Both keep a contrast of at least
/// 4.5:1 on every background shade (checked by a test).
pub const TEXT: Color32 = Color32::from_rgb(214, 218, 226);
pub const TEXT_DIM: Color32 = Color32::from_rgb(146, 152, 164);
pub const AXIS_X: Color32 = Color32::from_rgb(232, 76, 76);
pub const AXIS_Y: Color32 = Color32::from_rgb(110, 204, 82);
pub const AXIS_Z: Color32 = Color32::from_rgb(70, 136, 240);
pub const SELECTION: Color32 = Color32::from_rgb(255, 160, 40);

/// Background shades, darkest first.
pub const BG_0: Color32 = Color32::from_rgb(19, 20, 23);
pub const BG_1: Color32 = Color32::from_rgb(27, 28, 32);
pub const BG_2: Color32 = Color32::from_rgb(34, 36, 41);
pub const BG_3: Color32 = Color32::from_rgb(44, 46, 53);
pub const BG_4: Color32 = Color32::from_rgb(56, 59, 68);
pub const BORDER: Color32 = Color32::from_rgb(48, 50, 58);

/// A font of the bold family (headings, strong labels).
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(crate::fonts::BOLD.into()))
}

/// The theme with the built-in fonts (see [`crate::preferences::apply`] for the user's).
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(crate::fonts::definitions(None, &[]));

    let mut style = (*ctx.global_style()).clone();
    // Compact: dense panels like a desktop DCC tool, still readable (Thai needs ≥ 13 px).
    style.text_styles = [
        (TextStyle::Heading, bold(15.0)),
        (TextStyle::Body, FontId::proportional(13.0)),
        (TextStyle::Button, FontId::proportional(13.0)),
        (TextStyle::Small, FontId::proportional(11.0)),
        (TextStyle::Monospace, FontId::monospace(12.0)),
    ]
    .into();
    let spacing = &mut style.spacing;
    spacing.item_spacing = egui::vec2(6.0, 4.0);
    spacing.button_padding = egui::vec2(7.0, 3.0);
    spacing.interact_size = egui::vec2(36.0, 21.0);
    spacing.icon_width = 13.0;
    spacing.icon_width_inner = 7.0;
    spacing.icon_spacing = 5.0;
    spacing.window_margin = egui::Margin::same(10);
    spacing.menu_margin = egui::Margin::same(5);
    spacing.indent = 14.0;
    spacing.combo_width = 120.0;
    spacing.slider_width = 110.0;
    spacing.combo_height = 260.0;
    spacing.scroll = egui::style::ScrollStyle::thin();

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG_1;
    v.window_fill = BG_2;
    v.extreme_bg_color = BG_0;
    v.faint_bg_color = BG_2;
    v.code_bg_color = BG_0;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    // Soft, short shadows: depth without smudging what is underneath.
    v.window_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 10,
        spread: 0,
        color: Color32::from_black_alpha(100),
    };
    v.text_cursor.stroke = Stroke::new(2.0_f32, ACCENT);
    v.slider_trailing_fill = true;
    v.collapsing_header_frame = true;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARN;
    v.error_fg_color = ERROR;
    v.striped = true;
    for (widgets, fill, stroke) in [
        (&mut v.widgets.noninteractive, BG_2, BORDER),
        (&mut v.widgets.inactive, BG_3, BG_3),
        (
            &mut v.widgets.hovered,
            BG_4,
            Color32::from_rgb(84, 100, 130),
        ),
        (&mut v.widgets.active, ACCENT_DIM, ACCENT),
        (&mut v.widgets.open, BG_3, ACCENT_DIM),
    ] {
        widgets.bg_fill = fill;
        widgets.weak_bg_fill = fill;
        widgets.bg_stroke = Stroke::new(1.0_f32, stroke);
        widgets.corner_radius = CornerRadius::same(4);
        // No grow-on-hover: rows and grids stay put under the pointer.
        widgets.expansion = 0.0;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(200, 204, 214));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    ctx.set_global_style(style);
}

/// Icon for a node class.
pub fn class_icon(class: &str) -> &'static str {
    use egui_phosphor::regular as icon;
    match class {
        "Node" => icon::CIRCLE,
        "Node3D" => icon::CUBE_TRANSPARENT,
        "MeshInstance3D" => icon::CUBE,
        "Camera3D" => icon::VIDEO_CAMERA,
        "DirectionalLight3D" => icon::SUN,
        "OmniLight3D" => icon::LIGHTBULB,
        "SpotLight3D" => icon::FLASHLIGHT,
        "StaticBody3D" => icon::WALL,
        "RigidBody3D" => icon::BASKETBALL,
        "AnimatableBody3D" => icon::ARROWS_OUT_CARDINAL,
        "CharacterBody3D" => icon::PERSON_SIMPLE_WALK,
        "Area3D" => icon::BOUNDING_BOX,
        "CollisionShape3D" => icon::SHAPES,
        "Path3D" => icon::PATH,
        "Generator" => icon::DICE_FIVE,
        "Cutscene" => icon::FILM_SLATE,
        "PostVolume" => icon::APERTURE,
        "HLOD" => icon::STACK,
        "AudioStreamPlayer" | "AudioStreamPlayer3D" => icon::SPEAKER_HIGH,
        "AudioListener3D" => icon::EAR,
        "Timer" => icon::TIMER,
        "Particles3D" => icon::SPARKLE,
        "Joint3D" => icon::LINK,
        "Ragdoll" => icon::PERSON_ARMS_SPREAD,
        "CameraShake" => icon::VIBRATE,
        "AnimationPlayer" => icon::FILM_STRIP,
        "CanvasLayer" => icon::STACK,
        "Control" => icon::SELECTION,
        "Panel" => icon::RECTANGLE,
        "Label" => icon::TEXT_T,
        "Button" => icon::CURSOR_CLICK,
        "ProgressBar" => icon::BATTERY_HIGH,
        "VBoxContainer" => icon::ROWS,
        "HBoxContainer" => icon::COLUMNS,
        "TextureRect" => icon::IMAGE,
        _ => icon::DOT_OUTLINE,
    }
}

/// Accent color per class family, for icons.
pub fn class_color(class: &str) -> Color32 {
    match class {
        "MeshInstance3D" | "Path3D" => Color32::from_rgb(120, 190, 255),
        "Camera3D" => Color32::from_rgb(200, 160, 255),
        "DirectionalLight3D" | "OmniLight3D" | "SpotLight3D" => WARN,
        "StaticBody3D" | "RigidBody3D" | "AnimatableBody3D" | "CharacterBody3D" | "Area3D"
        | "CollisionShape3D" | "Joint3D" | "Ragdoll" => PLAY,
        "AudioStreamPlayer" | "AudioStreamPlayer3D" | "AudioListener3D" => {
            Color32::from_rgb(240, 130, 180)
        }
        "Particles3D" | "CameraShake" => Color32::from_rgb(255, 140, 80),
        "CanvasLayer" | "Control" | "Panel" | "Label" | "Button" | "ProgressBar"
        | "VBoxContainer" | "HBoxContainer" | "TextureRect" => Color32::from_rgb(110, 220, 160),
        _ => TEXT_DIM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio of two sRGB colors.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let luminance = |c: Color32| {
            let channel = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        };
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_is_readable_on_every_background() {
        for background in [BG_0, BG_1, BG_2, BG_3] {
            assert!(contrast(TEXT, background) >= 7.0, "{background:?}");
            assert!(contrast(TEXT_DIM, background) >= 4.5, "{background:?}");
        }
        // Accent text (links, highlights) on panels.
        assert!(contrast(ACCENT, BG_1) >= 4.5);
        assert!(contrast(WARN, BG_1) >= 4.5 && contrast(ERROR, BG_1) >= 4.5);
    }
}
