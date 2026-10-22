//! The in-game HUD (built in code, owned by the `Game` node).

use ab_script::prelude::*;
use ab_script::Error;

/// Handles of the HUD nodes the game updates.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Hud {
    pub layer: Option<Node>,
    pub dawn: Option<Node>,
    pub relics: Option<Node>,
    pub toast: Option<Node>,
    pub prompt: Option<Node>,
    pub stamina: Option<Node>,
    pub end_panel: Option<Node>,
    pub end_title: Option<Node>,
    pub end_text: Option<Node>,
    pub hint: Option<Node>,
}

const OUTLINE: Color = Color::rgba(0.0, 0.0, 0.0, 0.85);

/// A label with an outline, placed by a preset.
#[allow(clippy::too_many_arguments)]
fn label(
    tree: &mut Tree<'_>,
    name: &str,
    text: &str,
    preset: Option<(LayoutPreset, Vec2, f32)>,
    size: f32,
    color: Color,
    align: (HAlign, VAlign),
) -> Result<Node, Error> {
    let node = tree.create_label(name, text)?;
    if let Some((preset, extent, margin)) = preset {
        tree.control(node).set_preset(preset, extent, margin)?;
    }
    let mut label = tree.label(node);
    label.set_font_size(size)?;
    label.set_color(color)?;
    label.set_outline_width(2.0)?;
    label.set_outline_color(OUTLINE)?;
    label.set_align(align.0, align.1)?;
    Ok(node)
}

pub fn build(tree: &mut Tree<'_>, parent: Node) -> Result<Hud, Error> {
    let layer = tree.create_canvas_layer("Hud", 1)?;
    tree.add_child(parent, layer)?;
    let soft = Color::from_srgb8(210, 215, 230, 255);
    let warm = Color::from_srgb8(255, 205, 120, 255);
    let centered = (HAlign::Center, VAlign::Center);

    let dawn = label(
        tree,
        "Dawn",
        "",
        Some((LayoutPreset::CenterTop, Vec2::new(700.0, 60.0), 24.0)),
        40.0,
        warm,
        centered,
    )?;
    let toast = label(
        tree,
        "Toast",
        "",
        Some((LayoutPreset::CenterTop, Vec2::new(900.0, 40.0), 92.0)),
        26.0,
        soft,
        centered,
    )?;
    let relics = label(
        tree,
        "Relics",
        "",
        Some((LayoutPreset::TopLeft, Vec2::new(620.0, 50.0), 32.0)),
        30.0,
        soft,
        (HAlign::Left, VAlign::Top),
    )?;
    let prompt = label(
        tree,
        "Prompt",
        "",
        Some((LayoutPreset::CenterBottom, Vec2::new(900.0, 44.0), 120.0)),
        28.0,
        Color::WHITE,
        centered,
    )?;
    let stamina = tree.create_progress_bar("Stamina", 0.0, 1.0, 1.0)?;
    tree.control(stamina)
        .set_preset(LayoutPreset::CenterBottom, Vec2::new(320.0, 10.0), 90.0)?;
    let mut bar = tree.progress_bar(stamina);
    bar.set_fill_color(Color::from_srgb8(150, 200, 255, 220))?;
    bar.set_corner_radius(4.0)?;

    let crosshair = tree.create_panel("Crosshair", Color::rgba(1.0, 1.0, 1.0, 0.7))?;
    let mut control = tree.control(crosshair);
    control.set_preset(LayoutPreset::Center, Vec2::new(6.0, 6.0), 0.0)?;
    control.set_mouse_filter(MouseFilter::Ignore)?;
    tree.panel(crosshair).set_corner_radius(3.0)?;

    let hint = label(
        tree,
        "Hint",
        "WASD เดิน · Shift วิ่ง · E หยิบ/วาง · Esc ปล่อยเมาส์/ออก · F3 ข้อมูลดีบัก\n\
         WASD move · Shift sprint · E pick up/place · Esc release mouse/quit · F3 debug",
        Some((LayoutPreset::BottomLeft, Vec2::new(1500.0, 64.0), 28.0)),
        20.0,
        soft,
        (HAlign::Left, VAlign::Bottom),
    )?;

    let end_panel = tree.create_panel("EndPanel", Color::rgba(0.01, 0.01, 0.02, 0.88))?;
    tree.control(end_panel)
        .set_preset(LayoutPreset::Center, Vec2::new(860.0, 260.0), 0.0)?;
    let mut panel = tree.panel(end_panel);
    panel.set_corner_radius(14.0)?;
    panel.set_border_width(2.0)?;
    panel.set_border_color(Color::rgba(1.0, 0.8, 0.5, 0.5))?;

    let column = tree.create_box_container("Column", true)?;
    tree.control(column)
        .set_preset(LayoutPreset::FullRect, Vec2::ZERO, 32.0)?;
    let mut boxed = tree.box_container(column);
    boxed.set_separation(16.0)?;
    boxed.set_alignment(HAlign::Center)?;
    let end_title = label(tree, "Title", "", None, 56.0, warm, centered)?;
    let end_text = label(tree, "Text", "", None, 26.0, soft, centered)?;

    for node in [
        dawn, toast, relics, prompt, stamina, crosshair, hint, end_panel,
    ] {
        tree.add_child(layer, node)?;
    }
    tree.add_child(end_panel, column)?;
    tree.add_child(column, end_title)?;
    tree.add_child(column, end_text)?;
    tree.set_visible(end_panel, false)?;
    Ok(Hud {
        layer: Some(layer),
        dawn: Some(dawn),
        relics: Some(relics),
        toast: Some(toast),
        prompt: Some(prompt),
        stamina: Some(stamina),
        end_panel: Some(end_panel),
        end_title: Some(end_title),
        end_text: Some(end_text),
        hint: Some(hint),
    })
}

/// Sets a label's text if the node exists.
pub fn set_text(tree: &mut Tree<'_>, label: Option<Node>, text: &str) {
    if let Some(label) = label {
        let _ = tree.label(label).set_text(text);
    }
}

/// Shows `text` in the toast line, fading in and out.
pub fn toast(tree: &mut Tree<'_>, hud: &Hud, text: &str) {
    let Some(node) = hud.toast else {
        return;
    };
    set_text(tree, Some(node), text);
    let mut control = tree.control(node);
    let _ = control.stop_animations();
    let _ = control.fade(0.0, 1.0, 0.3);
    let _ = control.tween_alpha(1.0, 0.0, 0.8, 3.0, Easing::EaseIn);
}
