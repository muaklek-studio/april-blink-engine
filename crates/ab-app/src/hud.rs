//! On-screen UI of the demo: title, FPS, a "crates at rest" bar, buttons and a welcome toast.
//! Behaviour lives in `ab_demo_scripts::HudScript`.
//! Text mixes Thai and English to exercise shaping.

use ab_engine::prelude::*;
use ab_engine::ui::{Easing, TweenTarget};

/// Builds the HUD (its behaviour is `ab_demo_scripts::HudScript`).
pub fn build_hud(tree: &mut SceneTree) -> anyhow::Result<()> {
    let layer = tree.create_canvas_layer("Hud", 1);
    tree.add_child(tree.root(), layer)?;
    let accent = Color::from_srgb8(140, 190, 255, 255);
    let soft = Color::from_srgb8(200, 210, 230, 255);

    // Title card, top left.
    let card = tree.create_panel(
        "TitleCard",
        Control::new().with_preset(LayoutPreset::TopLeft, Vec2::new(620.0, 168.0), 32.0),
        Panel {
            style: StyleBox::default().with_border(1.0, Color::rgba(0.55, 0.75, 1.0, 0.35)),
        },
    );
    tree.add_child(layer, card)?;
    let column = tree.create_box_container(
        "Column",
        Control::new().with_preset(LayoutPreset::FullRect, Vec2::ZERO, 20.0),
        BoxContainer::vertical().with_separation(2.0),
    );
    tree.add_child(card, column)?;
    let title = tree.create_label(
        "Title",
        Control::new(),
        Label::new("April Blink Engine").with_style(TextStyle::new(44.0).with_shadow(TextShadow {
            color: Color::rgba(0.3, 0.55, 1.0, 0.7),
            offset: Vec2::ZERO,
            softness: 6.0,
            spread: 1.0,
        })),
    );
    tree.add_child(column, title)?;
    let subtitle = tree.create_label(
        "Subtitle",
        Control::new(),
        Label::new("ซากปรักหักพังยามค่ำคืน · Night ruin demo")
            .with_style(TextStyle::new(26.0).with_color(soft)),
    );
    tree.add_child(column, subtitle)?;
    let fps = tree.create_label(
        "Fps",
        Control::new(),
        Label::new("FPS --").with_style(TextStyle::new(22.0).with_color(accent)),
    );
    tree.add_child(column, fps)?;

    // Physics panel, top right: progress bar and two buttons.
    let side = tree.create_panel(
        "PhysicsPanel",
        Control::new().with_preset(LayoutPreset::TopRight, Vec2::new(460.0, 190.0), 32.0),
        Panel::default(),
    );
    tree.add_child(layer, side)?;
    let stack = tree.create_box_container(
        "Stack",
        Control::new().with_preset(LayoutPreset::FullRect, Vec2::ZERO, 18.0),
        BoxContainer::vertical().with_separation(10.0),
    );
    tree.add_child(side, stack)?;
    let caption = tree.create_label(
        "Caption",
        Control::new(),
        Label::new("ลังที่หยุดนิ่ง · Crates at rest").with_style(TextStyle::new(24.0)),
    );
    tree.add_child(stack, caption)?;
    // The script sets the maximum to the number of crates.
    let mut bar = ProgressBar::new(0.0, 1.0, 0.0);
    bar.show_percentage = true;
    let bar = tree.create_progress_bar(
        "AtRest",
        Control::new().with_min_size(Vec2::new(0.0, 30.0)),
        bar,
    );
    tree.add_child(stack, bar)?;
    let row = tree.create_box_container(
        "Buttons",
        Control::new().with_expand(true),
        BoxContainer::horizontal().with_separation(10.0),
    );
    tree.add_child(stack, row)?;
    let make_button = |tree: &mut SceneTree, name: &str, text: &str| {
        let button = tree.create_button(name, Control::new().with_expand(true), Button::new(text));
        // HUD buttons are mouse-only: Tab and Space stay with the camera controls.
        if let Some(mut control) = tree.component_mut::<Control>(button) {
            control.focusable = false;
        }
        tree.add_child(row, button).map(|_| button)
    };
    make_button(tree, "Drop", "ปล่อยลัง · Drop")?;
    make_button(tree, "Mute", "ปิดเสียง · Mute")?;

    // Controls hint, bottom left.
    let hint = tree.create_label(
        "Hint",
        Control::new().with_preset(LayoutPreset::BottomLeft, Vec2::new(1400.0, 70.0), 32.0),
        Label::new(
            "WASD เดิน · Space/Q ขึ้น-ลง · Tab มองรอบ · คลิกซ้าย/E ผลักลัง · Esc ออก\n\
             WASD move · Space/Q up/down · Tab look · Left click/E shove · Esc quit",
        )
        .with_style(
            TextStyle::new(22.0)
                .with_color(soft)
                .with_outline(2.0, Color::BLACK),
        )
        .with_align(HAlign::Left, VAlign::Bottom),
    );
    tree.add_child(layer, hint)?;

    // Welcome toast: slides in, stays, fades out.
    let toast = tree.create_panel(
        "Toast",
        Control::new().with_preset(LayoutPreset::CenterTop, Vec2::new(520.0, 64.0), 48.0),
        Panel {
            style: StyleBox::default().with_corner_radius(32.0),
        },
    );
    tree.add_child(layer, toast)?;
    let toast_text = tree.create_label(
        "Text",
        Control::full_rect(),
        Label::new("ยินดีต้อนรับ · Welcome")
            .with_style(TextStyle::new(28.0))
            .with_align(HAlign::Center, VAlign::Center),
    );
    tree.add_child(toast, toast_text)?;
    if let Some(mut animator) = tree.animate(toast) {
        animator
            .fade(0.0, 1.0, 0.6)
            .slide_in(Vec2::new(0.0, -120.0), 0.6)
            .tween(
                TweenTarget::Alpha { from: 1.0, to: 0.0 },
                0.8,
                4.0,
                Easing::EaseIn,
            );
    }

    // The script finds the panels by path, so it goes in last.
    let node = tree.create_node("HudScript");
    tree.set_script_named(node, "HudScript")?;
    tree.add_child(layer, node)?;
    Ok(())
}
