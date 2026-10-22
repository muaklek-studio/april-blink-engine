//! UI nodes in scene files: `CanvasLayer` and the controls under it.
//!
//! ```toml
//! [[node]]
//! name = "Hud"
//! parent = "."
//! type = "CanvasLayer"
//!
//! [[node]]
//! name = "Health"
//! parent = "Hud"
//! type = "ProgressBar"
//! control = { preset = "top_left", size = [360, 28], margin = 32 }
//! progress = { value = 100, fill_color = "#d23c3c" }
//! ```

use super::format::*;
use super::SceneOrigin;
use ab_asset::AssetServer;
use ab_core::{Color, Vec2};
use ab_rendering::{ColorSpace, RenderAssets};
use ab_scene::{NodeHandle, SceneTree};
use ab_ui::{
    class, BoxContainer, Button, CanvasLayer, Control, HAlign, Label, LayoutPreset, MouseFilter,
    Panel, ProgressBar, StretchMode, StyleBox, TextureRect, UiNodes, VAlign,
};

/// Every UI node type a scene file can hold.
pub const UI_TYPES: [&str; 9] = [
    class::CANVAS_LAYER,
    class::CONTROL,
    class::PANEL,
    class::LABEL,
    class::BUTTON,
    class::PROGRESS_BAR,
    class::VBOX_CONTAINER,
    class::HBOX_CONTAINER,
    class::TEXTURE_RECT,
];

/// Layout preset names, as scene files write them.
pub const PRESETS: [(&str, LayoutPreset); 16] = [
    ("top_left", LayoutPreset::TopLeft),
    ("center_top", LayoutPreset::CenterTop),
    ("top_right", LayoutPreset::TopRight),
    ("center_left", LayoutPreset::CenterLeft),
    ("center", LayoutPreset::Center),
    ("center_right", LayoutPreset::CenterRight),
    ("bottom_left", LayoutPreset::BottomLeft),
    ("center_bottom", LayoutPreset::CenterBottom),
    ("bottom_right", LayoutPreset::BottomRight),
    ("left_wide", LayoutPreset::LeftWide),
    ("top_wide", LayoutPreset::TopWide),
    ("right_wide", LayoutPreset::RightWide),
    ("bottom_wide", LayoutPreset::BottomWide),
    ("vcenter_wide", LayoutPreset::VCenterWide),
    ("hcenter_wide", LayoutPreset::HCenterWide),
    ("full_rect", LayoutPreset::FullRect),
];

pub fn preset(name: &str) -> Result<LayoutPreset, String> {
    PRESETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| *p)
        .ok_or_else(|| {
            let names: Vec<&str> = PRESETS.iter().map(|(n, _)| *n).collect();
            format!("unknown layout preset `{name}` ({})", names.join(", "))
        })
}

/// The `Control` a `control` table describes (before the node type's own defaults, like
/// labels letting the pointer through).
pub fn control(desc: Option<&ControlDesc>) -> Result<Control, String> {
    let Some(d) = desc else {
        return Ok(Control::new());
    };
    let mut control = Control::new();
    if let Some(name) = &d.preset {
        let size = Vec2::from(d.size.unwrap_or([0.0; 2]));
        control = control.with_preset(preset(name)?, size, d.margin.unwrap_or(0.0));
    } else if let Some(size) = d.size {
        control.offsets[2] = control.offsets[0] + size[0];
        control.offsets[3] = control.offsets[1] + size[1];
    }
    if let Some(anchors) = d.anchors {
        control.anchors = anchors;
    }
    if let Some(offsets) = d.offsets {
        control.offsets = offsets;
    }
    if let Some(min) = d.min_size {
        control.min_size = Vec2::from(min);
    }
    control.expand = d.expand.unwrap_or(false);
    control.clip_contents = d.clip.unwrap_or(false);
    if let Some(modulate) = &d.modulate {
        control.modulate = modulate.to_color()?;
    }
    Ok(control)
}

fn mouse_filter(name: &str) -> Result<MouseFilter, String> {
    match name {
        "stop" => Ok(MouseFilter::Stop),
        "ignore" => Ok(MouseFilter::Ignore),
        other => Err(format!("unknown mouse filter `{other}` (stop or ignore)")),
    }
}

fn h_align(name: &str) -> Result<HAlign, String> {
    match name {
        "left" | "begin" => Ok(HAlign::Left),
        "center" => Ok(HAlign::Center),
        "right" | "end" => Ok(HAlign::Right),
        other => Err(format!(
            "unknown alignment `{other}` (left, center or right)"
        )),
    }
}

fn v_align(name: &str) -> Result<VAlign, String> {
    match name {
        "top" => Ok(VAlign::Top),
        "center" => Ok(VAlign::Center),
        "bottom" => Ok(VAlign::Bottom),
        other => Err(format!(
            "unknown alignment `{other}` (top, center or bottom)"
        )),
    }
}

fn stretch(name: &str) -> Result<StretchMode, String> {
    match name {
        "scale" => Ok(StretchMode::Scale),
        "keep_aspect" => Ok(StretchMode::KeepAspectCentered),
        "cover" => Ok(StretchMode::KeepAspectCovered),
        other => Err(format!(
            "unknown stretch `{other}` (scale, keep_aspect or cover)"
        )),
    }
}

fn opt_color(desc: &Option<ColorDesc>, default: Color) -> Result<Color, String> {
    desc.as_ref().map_or(Ok(default), ColorDesc::to_color)
}

/// `color` brightened (`amount` > 0) or darkened (< 0), alpha kept.
fn shade(color: Color, amount: f32) -> Color {
    let f = |c: f32| {
        if amount >= 0.0 {
            c + (1.0 - c) * amount
        } else {
            c * (1.0 + amount)
        }
    };
    Color::rgba(f(color.r), f(color.g), f(color.b), color.a)
}

/// The label a `label` table describes.
pub fn label(d: &LabelDesc) -> Result<Label, String> {
    let mut label = Label::new(d.text.clone());
    label.style.size = d.size.unwrap_or(label.style.size);
    label.style.color = opt_color(&d.color, label.style.color)?;
    label.style.outline_width = d.outline.unwrap_or(0.0);
    label.style.outline_color = opt_color(&d.outline_color, label.style.outline_color)?;
    label.h_align = d.align.as_deref().map_or(Ok(HAlign::Left), h_align)?;
    label.v_align = d.valign.as_deref().map_or(Ok(VAlign::Top), v_align)?;
    label.autowrap = d.autowrap.unwrap_or(false);
    Ok(label)
}

/// Applies a `button` table (the hover and press shades follow the color).
pub fn apply_button(button: &mut Button, d: &ButtonDesc) -> Result<(), String> {
    button.text = d.text.clone();
    if let Some(size) = d.size {
        button.text_style.size = size;
    }
    button.text_style.color = opt_color(&d.text_color, Color::WHITE)?;
    let base = Button::new("");
    let color = opt_color(&d.color, base.normal.color)?;
    button.normal.color = color;
    if d.color.is_some() {
        button.hover.color = shade(color, 0.15);
        button.pressed.color = shade(color, -0.3);
        button.disabled_style.color = Color {
            a: color.a * 0.6,
            ..shade(color, -0.4)
        };
    } else {
        button.hover.color = base.hover.color;
        button.pressed.color = base.pressed.color;
        button.disabled_style.color = base.disabled_style.color;
    }
    button.disabled = d.disabled.unwrap_or(false);
    Ok(())
}

pub fn panel(d: &PanelDesc) -> Result<Panel, String> {
    let default = StyleBox::default();
    Ok(Panel {
        style: StyleBox {
            color: opt_color(&d.color, default.color)?,
            corner_radius: d.corner_radius.unwrap_or(default.corner_radius),
            border_width: d.border_width.unwrap_or(default.border_width),
            border_color: opt_color(&d.border_color, Color::WHITE)?,
            padding: default.padding,
        },
    })
}

/// Applies a `progress` table.
pub fn apply_progress(bar: &mut ProgressBar, d: &ProgressDesc) -> Result<(), String> {
    let default = ProgressBar::default();
    bar.min = d.min.unwrap_or(default.min);
    bar.max = d.max.unwrap_or(default.max);
    bar.value = d.value.unwrap_or(default.value);
    bar.fill.color = opt_color(&d.fill_color, default.fill.color)?;
    bar.background.color = opt_color(&d.background_color, default.background.color)?;
    bar.show_percentage = d.show_percentage.unwrap_or(false);
    Ok(())
}

pub fn box_container(vertical: bool, d: Option<&BoxDesc>) -> Result<BoxContainer, String> {
    let mut container = if vertical {
        BoxContainer::vertical()
    } else {
        BoxContainer::horizontal()
    };
    if let Some(d) = d {
        container.separation = d.separation.unwrap_or(container.separation);
        container.alignment = d.alignment.as_deref().map_or(Ok(HAlign::Left), h_align)?;
    }
    Ok(container)
}

/// Loads an image's texture through the [`AssetServer`].
pub fn load_image(tree: &SceneTree, d: &ImageDesc) -> Result<TextureRect, String> {
    let world = tree.world();
    let mut server = world
        .resource_mut::<AssetServer>()
        .ok_or("the AssetServer resource is missing")?;
    let mut render = world
        .resource_mut::<RenderAssets>()
        .ok_or("the RenderAssets resource is missing")?;
    let texture = server
        .load_texture(&mut render, &d.texture, ColorSpace::Srgb)
        .map_err(|e| e.to_string())?;
    Ok(TextureRect {
        texture,
        stretch: d
            .stretch
            .as_deref()
            .map_or(Ok(StretchMode::Scale), stretch)?,
        tint: opt_color(&d.tint, Color::WHITE)?,
    })
}

/// Creates the UI node `kind` describes, or `None` when `kind` is not a UI type.
pub fn create(
    tree: &mut SceneTree,
    kind: &str,
    desc: &NodeDesc,
) -> Option<Result<NodeHandle, String>> {
    UI_TYPES
        .contains(&kind)
        .then(|| create_ui(tree, kind, desc))
}

fn create_ui(tree: &mut SceneTree, kind: &str, desc: &NodeDesc) -> Result<NodeHandle, String> {
    let name = desc.name.as_str();
    if kind == class::CANVAS_LAYER {
        let layer = desc.canvas.as_ref().and_then(|c| c.layer).unwrap_or(0);
        return Ok(tree.create_canvas_layer(name, layer));
    }
    let c = control(desc.control.as_ref())?;
    let node = match kind {
        class::CONTROL => tree.create_control(name, c),
        class::PANEL => {
            let p = panel(&desc.panel.clone().unwrap_or_default())?;
            tree.create_panel(name, c, p)
        }
        class::LABEL => {
            let l = label(&desc.label.clone().unwrap_or_default())?;
            tree.create_label(name, c, l)
        }
        class::BUTTON => {
            let mut b = Button::new("");
            apply_button(&mut b, &desc.button.clone().unwrap_or_default())?;
            tree.create_button(name, c, b)
        }
        class::PROGRESS_BAR => {
            let mut bar = ProgressBar::default();
            apply_progress(&mut bar, &desc.progress.clone().unwrap_or_default())?;
            tree.create_progress_bar(name, c, bar)
        }
        class::VBOX_CONTAINER | class::HBOX_CONTAINER => {
            let vertical = kind == class::VBOX_CONTAINER;
            let container = box_container(vertical, desc.box_container.as_ref())?;
            tree.create_box_container(name, c, container)
        }
        class::TEXTURE_RECT => {
            let image = desc
                .image
                .clone()
                .ok_or("TextureRect needs `image = { texture = ... }`")?;
            let rect = load_image(tree, &image)?;
            let node = tree.create_texture_rect(name, c, rect);
            tree.insert_component(
                node,
                SceneOrigin {
                    image: Some(image),
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
            node
        }
        _ => unreachable!("checked by `create`"),
    };
    // The type's own default (labels let the pointer through) unless the file says.
    if let Some(mouse) = desc.control.as_ref().and_then(|c| c.mouse.as_deref()) {
        let filter = mouse_filter(mouse)?;
        if let Some(mut control) = tree.component_mut::<Control>(node) {
            control.mouse_filter = filter;
        }
    }
    Ok(node)
}

/// The UI tables of a description, to compare two descriptions' UI parts.
#[allow(clippy::type_complexity)]
pub fn ui_tables(
    d: &NodeDesc,
) -> (
    &Option<CanvasDesc>,
    &Option<ControlDesc>,
    &Option<LabelDesc>,
    &Option<ButtonDesc>,
    &Option<PanelDesc>,
    &Option<ProgressDesc>,
    &Option<BoxDesc>,
    &Option<ImageDesc>,
) {
    (
        &d.canvas,
        &d.control,
        &d.label,
        &d.button,
        &d.panel,
        &d.progress,
        &d.box_container,
        &d.image,
    )
}

/// Updates a UI node from its (edited) description in place: the node keeps its handle,
/// children and widget state.
pub fn apply(tree: &mut SceneTree, node: NodeHandle, desc: &NodeDesc) -> Result<(), String> {
    let class_name = tree.class_name(node).unwrap_or_default().to_owned();
    if class_name == class::CANVAS_LAYER {
        if let Some(mut canvas) = tree.component_mut::<CanvasLayer>(node) {
            canvas.layer = desc.canvas.as_ref().and_then(|c| c.layer).unwrap_or(0);
        }
        return Ok(());
    }
    let fresh = control(desc.control.as_ref())?;
    let mouse = match desc.control.as_ref().and_then(|c| c.mouse.as_deref()) {
        Some(name) => mouse_filter(name)?,
        None if passive_by_default(&class_name) => MouseFilter::Ignore,
        None => MouseFilter::Stop,
    };
    if let Some(mut c) = tree.component_mut::<Control>(node) {
        c.anchors = fresh.anchors;
        c.offsets = fresh.offsets;
        c.min_size = fresh.min_size;
        c.expand = fresh.expand;
        c.modulate = fresh.modulate;
        c.clip_contents = fresh.clip_contents;
        c.mouse_filter = mouse;
    }
    match class_name.as_str() {
        class::PANEL => {
            let p = panel(&desc.panel.clone().unwrap_or_default())?;
            if let Some(mut panel) = tree.component_mut::<Panel>(node) {
                panel.style = p.style;
            }
        }
        class::LABEL => {
            let l = label(&desc.label.clone().unwrap_or_default())?;
            if let Some(mut label) = tree.component_mut::<Label>(node) {
                label.text = l.text;
                label.style = l.style;
                label.h_align = l.h_align;
                label.v_align = l.v_align;
                label.autowrap = l.autowrap;
            }
        }
        class::BUTTON => {
            if let Some(mut button) = tree.component_mut::<Button>(node) {
                apply_button(&mut button, &desc.button.clone().unwrap_or_default())?;
            }
        }
        class::PROGRESS_BAR => {
            if let Some(mut bar) = tree.component_mut::<ProgressBar>(node) {
                apply_progress(&mut bar, &desc.progress.clone().unwrap_or_default())?;
            }
        }
        class::VBOX_CONTAINER | class::HBOX_CONTAINER => {
            let vertical = class_name == class::VBOX_CONTAINER;
            let fresh = box_container(vertical, desc.box_container.as_ref())?;
            if let Some(mut container) = tree.component_mut::<BoxContainer>(node) {
                *container = fresh;
            }
        }
        class::TEXTURE_RECT => {
            let image = desc
                .image
                .clone()
                .ok_or("TextureRect needs `image = { texture = ... }`")?;
            let rect = load_image(tree, &image)?;
            if let Some(mut current) = tree.component_mut::<TextureRect>(node) {
                *current = rect;
            }
            let mut origin = tree
                .component::<SceneOrigin>(node)
                .map(|o| o.clone())
                .unwrap_or_default();
            origin.image = Some(image);
            tree.insert_component(node, origin)
                .map_err(|e| e.to_string())?;
        }
        _ => {}
    }
    Ok(())
}

/// Whether a node of this class lets the pointer through unless told otherwise.
pub fn passive_by_default(class_name: &str) -> bool {
    matches!(
        class_name,
        class::CONTROL
            | class::LABEL
            | class::TEXTURE_RECT
            | class::VBOX_CONTAINER
            | class::HBOX_CONTAINER
    )
}

fn round(v: f32) -> f32 {
    let r = (v * 1e3).round() / 1e3;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

fn changed_color(color: Color, default: Color) -> Option<ColorDesc> {
    let desc = ColorDesc::from_color(color);
    (desc != ColorDesc::from_color(default)).then_some(desc)
}

/// The `control` table for a control (`None` when everything is at its default).
pub fn describe_control(control: &Control, class_name: &str) -> Option<ControlDesc> {
    let default_filter = if passive_by_default(class_name) {
        MouseFilter::Ignore
    } else {
        MouseFilter::Stop
    };
    let d = ControlDesc {
        anchors: (control.anchors != [0.0; 4]).then_some(control.anchors.map(round)),
        offsets: (control.offsets != [0.0; 4]).then_some(control.offsets.map(round)),
        min_size: (control.min_size != Vec2::ZERO)
            .then_some(control.min_size.to_array().map(round)),
        expand: control.expand.then_some(true),
        mouse: (control.mouse_filter != default_filter).then(|| {
            match control.mouse_filter {
                MouseFilter::Stop => "stop",
                MouseFilter::Ignore => "ignore",
            }
            .to_owned()
        }),
        modulate: changed_color(control.modulate, Color::WHITE),
        clip: control.clip_contents.then_some(true),
        ..Default::default()
    };
    (d != ControlDesc::default()).then_some(d)
}

fn h_align_name(align: HAlign, begin: &'static str, end: &'static str) -> &'static str {
    match align {
        HAlign::Left => begin,
        HAlign::Center => "center",
        HAlign::Right => end,
    }
}

pub fn describe_label(label: &Label) -> LabelDesc {
    let default = Label::default();
    let s = &label.style;
    LabelDesc {
        text: label.text.clone(),
        size: (s.size != default.style.size).then_some(round(s.size)),
        color: changed_color(s.color, default.style.color),
        align: (label.h_align != HAlign::Left)
            .then(|| h_align_name(label.h_align, "left", "right").to_owned()),
        valign: (label.v_align != VAlign::Top).then(|| {
            match label.v_align {
                VAlign::Top => "top",
                VAlign::Center => "center",
                VAlign::Bottom => "bottom",
            }
            .to_owned()
        }),
        outline: (s.outline_width > 0.0).then_some(round(s.outline_width)),
        outline_color: (s.outline_width > 0.0)
            .then(|| changed_color(s.outline_color, default.style.outline_color))
            .flatten(),
        autowrap: label.autowrap.then_some(true),
    }
}

pub fn describe_button(button: &Button) -> ButtonDesc {
    let default = Button::new("");
    ButtonDesc {
        text: button.text.clone(),
        size: (button.text_style.size != default.text_style.size)
            .then_some(round(button.text_style.size)),
        color: changed_color(button.normal.color, default.normal.color),
        text_color: changed_color(button.text_style.color, Color::WHITE),
        disabled: button.disabled.then_some(true),
    }
}

pub fn describe_panel(panel: &Panel) -> PanelDesc {
    let default = StyleBox::default();
    let s = &panel.style;
    PanelDesc {
        color: changed_color(s.color, default.color),
        corner_radius: (s.corner_radius != default.corner_radius).then_some(round(s.corner_radius)),
        border_width: (s.border_width != default.border_width).then_some(round(s.border_width)),
        border_color: (s.border_width > 0.0)
            .then(|| changed_color(s.border_color, Color::WHITE))
            .flatten(),
    }
}

pub fn describe_progress(bar: &ProgressBar) -> ProgressDesc {
    let default = ProgressBar::default();
    let changed = |v: f32, d: f32| (v != d).then_some(round(v));
    ProgressDesc {
        min: changed(bar.min, default.min),
        max: changed(bar.max, default.max),
        value: changed(bar.value, default.value),
        fill_color: changed_color(bar.fill.color, default.fill.color),
        background_color: changed_color(bar.background.color, default.background.color),
        show_percentage: bar.show_percentage.then_some(true),
    }
}

pub fn describe_box(container: &BoxContainer) -> BoxDesc {
    let default = BoxContainer::vertical();
    BoxDesc {
        separation: (container.separation != default.separation)
            .then_some(round(container.separation)),
        alignment: (container.alignment != HAlign::Left)
            .then(|| h_align_name(container.alignment, "begin", "end").to_owned()),
    }
}

fn stretch_name(mode: StretchMode) -> &'static str {
    match mode {
        StretchMode::Scale => "scale",
        StretchMode::KeepAspectCentered => "keep_aspect",
        StretchMode::KeepAspectCovered => "cover",
    }
}

/// Fills `desc` for a UI node. Returns `false` when the node cannot be described (an image
/// whose texture was made in code).
pub fn describe(tree: &SceneTree, node: NodeHandle, class_name: &str, desc: &mut NodeDesc) -> bool {
    if class_name == class::CANVAS_LAYER {
        let layer = tree.component::<CanvasLayer>(node).map_or(0, |c| c.layer);
        desc.canvas = (layer != 0).then_some(CanvasDesc { layer: Some(layer) });
        return true;
    }
    if let Some(c) = tree.component::<Control>(node) {
        desc.control = describe_control(&c, class_name);
    }
    match class_name {
        class::PANEL => {
            if let Some(p) = tree.component::<Panel>(node) {
                desc.panel = Some(describe_panel(&p)).filter(|d| *d != PanelDesc::default());
            }
        }
        class::LABEL => desc.label = tree.component::<Label>(node).map(|l| describe_label(&l)),
        class::BUTTON => desc.button = tree.component::<Button>(node).map(|b| describe_button(&b)),
        class::PROGRESS_BAR => {
            if let Some(bar) = tree.component::<ProgressBar>(node) {
                desc.progress =
                    Some(describe_progress(&bar)).filter(|d| *d != ProgressDesc::default());
            }
        }
        class::VBOX_CONTAINER | class::HBOX_CONTAINER => {
            if let Some(b) = tree.component::<BoxContainer>(node) {
                desc.box_container = Some(describe_box(&b)).filter(|d| *d != BoxDesc::default());
            }
        }
        class::TEXTURE_RECT => {
            let origin = tree
                .component::<SceneOrigin>(node)
                .and_then(|o| o.image.clone());
            let (Some(mut image), Some(rect)) = (origin, tree.component::<TextureRect>(node))
            else {
                return false;
            };
            image.stretch =
                (rect.stretch != StretchMode::Scale).then(|| stretch_name(rect.stretch).to_owned());
            image.tint = changed_color(rect.tint, Color::WHITE);
            desc.image = Some(image);
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_place_like_control_with_preset() {
        let c = control(Some(&ControlDesc {
            preset: Some("bottom_right".into()),
            size: Some([200.0, 50.0]),
            margin: Some(10.0),
            ..Default::default()
        }))
        .unwrap();
        assert_eq!(c.anchors, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(c.offsets, [-210.0, -60.0, -10.0, -10.0]);
        assert!(preset("sideways").is_err());
    }

    #[test]
    fn default_widgets_describe_as_empty_tables() {
        assert_eq!(describe_label(&Label::default()), LabelDesc::default());
        assert_eq!(describe_button(&Button::new("")), ButtonDesc::default());
        assert_eq!(describe_panel(&Panel::default()), PanelDesc::default());
        assert_eq!(
            describe_progress(&ProgressBar::default()),
            ProgressDesc::default()
        );
        assert_eq!(describe_box(&BoxContainer::vertical()), BoxDesc::default());
        assert_eq!(describe_control(&Control::new(), class::PANEL), None);
        let passive = Control::new().with_mouse_filter(MouseFilter::Ignore);
        assert_eq!(describe_control(&passive, class::LABEL), None);
    }

    #[test]
    fn widget_tables_round_trip() {
        let l = LabelDesc {
            text: "Score".into(),
            size: Some(40.0),
            color: Some(ColorDesc::Hex("#ffcc00".into())),
            align: Some("center".into()),
            valign: Some("bottom".into()),
            outline: Some(2.0),
            outline_color: Some(ColorDesc::Hex("#202020".into())),
            autowrap: Some(true),
        };
        assert_eq!(describe_label(&label(&l).unwrap()), l);
        let b = ButtonDesc {
            text: "Play".into(),
            size: Some(30.0),
            color: Some(ColorDesc::Hex("#3366cc".into())),
            text_color: Some(ColorDesc::Hex("#000000".into())),
            disabled: Some(true),
        };
        let mut button = Button::new("");
        apply_button(&mut button, &b).unwrap();
        assert_eq!(describe_button(&button), b);
        let p = ProgressDesc {
            min: Some(10.0),
            max: Some(50.0),
            value: Some(25.0),
            fill_color: Some(ColorDesc::Hex("#d23c3c".into())),
            background_color: Some(ColorDesc::Hex("#00000080".into())),
            show_percentage: Some(true),
        };
        let mut bar = ProgressBar::default();
        apply_progress(&mut bar, &p).unwrap();
        assert_eq!(describe_progress(&bar), p);
        let x = BoxDesc {
            separation: Some(12.0),
            alignment: Some("end".into()),
        };
        assert_eq!(describe_box(&box_container(false, Some(&x)).unwrap()), x);
    }
}
