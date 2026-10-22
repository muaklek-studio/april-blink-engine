//! The editor's look: a neutral dark theme with a blue accent, Prompt for text (Latin and
//! Thai) and Phosphor icons.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
};
use std::sync::Arc;

pub const ACCENT: Color32 = Color32::from_rgb(64, 138, 255);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(38, 79, 145);
pub const PLAY: Color32 = Color32::from_rgb(76, 194, 120);
pub const WARN: Color32 = Color32::from_rgb(232, 178, 64);
pub const ERROR: Color32 = Color32::from_rgb(235, 87, 87);
pub const TEXT_DIM: Color32 = Color32::from_rgb(140, 146, 158);
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

pub fn apply(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "prompt".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../ab-ui/fonts/Prompt-Regular.ttf"
        ))),
    );
    // Prompt first (Latin + Thai); egui's fonts stay as fallbacks (symbols, emoji).
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.insert(0, "prompt".into());
    }
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    let mut style = (*ctx.global_style()).clone();
    // Compact: dense panels like a desktop DCC tool, still readable (Thai needs ≥ 13 px).
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(15.0)),
        (TextStyle::Body, FontId::proportional(13.0)),
        (TextStyle::Button, FontId::proportional(13.0)),
        (TextStyle::Small, FontId::proportional(11.0)),
        (TextStyle::Monospace, FontId::monospace(12.0)),
    ]
    .into();
    let spacing = &mut style.spacing;
    spacing.item_spacing = egui::vec2(6.0, 3.0);
    spacing.button_padding = egui::vec2(6.0, 2.0);
    spacing.interact_size = egui::vec2(36.0, 20.0);
    spacing.icon_width = 13.0;
    spacing.icon_width_inner = 7.0;
    spacing.icon_spacing = 5.0;
    spacing.window_margin = egui::Margin::same(8);
    spacing.menu_margin = egui::Margin::same(4);
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
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(4);
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARN;
    v.error_fg_color = ERROR;
    v.striped = true;
    for (widgets, fill, stroke) in [
        (&mut v.widgets.noninteractive, BG_2, BORDER),
        (&mut v.widgets.inactive, BG_3, BG_3),
        (&mut v.widgets.hovered, BG_4, Color32::from_rgb(78, 82, 94)),
        (&mut v.widgets.active, ACCENT_DIM, ACCENT),
        (&mut v.widgets.open, BG_3, BORDER),
    ] {
        widgets.bg_fill = fill;
        widgets.weak_bg_fill = fill;
        widgets.bg_stroke = Stroke::new(1.0_f32, stroke);
        widgets.corner_radius = CornerRadius::same(3);
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(196, 200, 210));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(214, 218, 226));
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
        "MeshInstance3D" => Color32::from_rgb(120, 190, 255),
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
