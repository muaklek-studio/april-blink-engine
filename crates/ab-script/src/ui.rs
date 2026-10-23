//! Screen-space UI: canvas layers, controls, labels, buttons, bars, panels, animations.
//!
//! Controls are nodes: create them, then place them under a canvas layer (or another
//! control) with [`Tree::add_child`]. Style them through the accessors ([`Tree::label`], …).

use crate::host::method;
use crate::math::Vec2;
use crate::physics::property;
use crate::tree::Tree;
use crate::value::{Color, Error, IntoValue, Node};

/// Godot's layout presets: where a control sits in its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutPreset {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    CenterLeft,
    CenterTop,
    CenterRight,
    CenterBottom,
    Center,
    LeftWide,
    TopWide,
    RightWide,
    BottomWide,
    VCenterWide,
    HCenterWide,
    FullRect,
}

impl LayoutPreset {
    fn name(self) -> &'static str {
        match self {
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
            Self::CenterLeft => "center_left",
            Self::CenterTop => "center_top",
            Self::CenterRight => "center_right",
            Self::CenterBottom => "center_bottom",
            Self::Center => "center",
            Self::LeftWide => "left_wide",
            Self::TopWide => "top_wide",
            Self::RightWide => "right_wide",
            Self::BottomWide => "bottom_wide",
            Self::VCenterWide => "vcenter_wide",
            Self::HCenterWide => "hcenter_wide",
            Self::FullRect => "full_rect",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl HAlign {
    fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VAlign {
    #[default]
    Top,
    Center,
    Bottom,
}

impl VAlign {
    fn name(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Center => "center",
            Self::Bottom => "bottom",
        }
    }
}

/// Whether a control takes the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MouseFilter {
    /// Receives hover and clicks and blocks what is below (the default for panels/buttons).
    #[default]
    Stop,
    /// Lets the pointer through.
    Ignore,
}

/// Easing curve of an animation or tween.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Easing {
    Linear,
    EaseIn,
    #[default]
    EaseOut,
    EaseInOut,
    /// Overshoots a little, then settles.
    BackOut,
    /// Springs past the target and wobbles.
    ElasticOut,
    /// Bounces on arrival.
    BounceOut,
    /// Gentle sine in and out.
    SineInOut,
}

impl Easing {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::EaseIn => "ease_in",
            Self::EaseOut => "ease_out",
            Self::EaseInOut => "ease_in_out",
            Self::BackOut => "back_out",
            Self::ElasticOut => "elastic_out",
            Self::BounceOut => "bounce_out",
            Self::SineInOut => "sine_in_out",
        }
    }
}

/// Signal of buttons, emitted when clicked.
pub const PRESSED: &str = "pressed";

impl<'a> Tree<'a> {
    /// A layer drawing its controls over the 3D view; higher layers draw on top.
    pub fn create_canvas_layer(&mut self, name: &str, layer: i32) -> Result<Node, Error> {
        self.created(
            method!("ui.create_canvas_layer"),
            &[name.to_value(), layer.to_value()],
        )
    }

    /// A plain control, for grouping and anchoring (lets the pointer through).
    pub fn create_control(&mut self, name: &str) -> Result<Node, Error> {
        self.created(method!("ui.create_control"), &[name.to_value()])
    }

    /// A text label (lets the pointer through).
    pub fn create_label(&mut self, name: &str, text: &str) -> Result<Node, Error> {
        self.created(
            method!("ui.create_label"),
            &[name.to_value(), text.to_value()],
        )
    }

    /// A button; it emits [`PRESSED`] when clicked.
    pub fn create_button(&mut self, name: &str, text: &str) -> Result<Node, Error> {
        self.created(
            method!("ui.create_button"),
            &[name.to_value(), text.to_value()],
        )
    }

    /// A flat colored panel.
    pub fn create_panel(&mut self, name: &str, color: Color) -> Result<Node, Error> {
        self.created(
            method!("ui.create_panel"),
            &[name.to_value(), color.to_value()],
        )
    }

    pub fn create_progress_bar(
        &mut self,
        name: &str,
        min: f32,
        max: f32,
        value: f32,
    ) -> Result<Node, Error> {
        self.created(
            method!("ui.create_progress_bar"),
            &[
                name.to_value(),
                min.to_value(),
                max.to_value(),
                value.to_value(),
            ],
        )
    }

    /// Lays its children out in a column (`vertical`) or a row.
    pub fn create_box_container(&mut self, name: &str, vertical: bool) -> Result<Node, Error> {
        self.created(
            method!("ui.create_box_container"),
            &[name.to_value(), vertical.to_value()],
        )
    }

    /// Whether the mouse is over a control that takes it (clicks there are for the UI).
    pub fn is_pointer_over_ui(&self) -> bool {
        self.query(method!("ui.is_pointer_over"), &[])
            .unwrap_or(false)
    }

    /// The node's `Control`: placement, color, animations.
    pub fn control(&mut self, node: Node) -> ControlRef<'_, 'a> {
        ControlRef { tree: self, node }
    }

    pub fn label(&mut self, node: Node) -> LabelRef<'_, 'a> {
        LabelRef { tree: self, node }
    }

    pub fn button(&mut self, node: Node) -> ButtonRef<'_, 'a> {
        ButtonRef { tree: self, node }
    }

    pub fn progress_bar(&mut self, node: Node) -> ProgressBarRef<'_, 'a> {
        ProgressBarRef { tree: self, node }
    }

    pub fn panel(&mut self, node: Node) -> PanelRef<'_, 'a> {
        PanelRef { tree: self, node }
    }

    pub fn box_container(&mut self, node: Node) -> BoxContainerRef<'_, 'a> {
        BoxContainerRef { tree: self, node }
    }
}

/// A node's `Control` ([`Tree::control`]).
#[derive(Debug)]
pub struct ControlRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl ControlRef<'_, '_> {
    /// Places the control like Godot's layout presets: `size` along the axes that are not
    /// stretched, `margin` from the anchored edges.
    pub fn set_preset(
        &mut self,
        preset: LayoutPreset,
        size: Vec2,
        margin: f32,
    ) -> Result<(), Error> {
        self.tree.run(
            method!("control.set_preset"),
            &[
                self.node.to_value(),
                preset.name().to_value(),
                size.to_value(),
                margin.to_value(),
            ],
        )
    }

    /// Absolute placement in the parent: top-left corner and size.
    pub fn set_rect(&mut self, position: Vec2, size: Vec2) -> Result<(), Error> {
        self.tree.run(
            method!("control.set_rect"),
            &[self.node.to_value(), position.to_value(), size.to_value()],
        )
    }

    pub fn set_mouse_filter(&mut self, filter: MouseFilter) -> Result<(), Error> {
        let name = match filter {
            MouseFilter::Stop => "stop",
            MouseFilter::Ignore => "ignore",
        };
        self.tree.set(self.node, "control.mouse_filter", name)
    }

    property!(
        modulate,
        set_modulate,
        Color,
        "control.modulate",
        "Tint of the control and its children (alpha fades them)."
    );
    property!(
        alpha,
        set_alpha,
        f32,
        "control.alpha",
        "Opacity of the control and its children."
    );
    property!(
        translation,
        set_translation,
        Vec2,
        "control.translation",
        "Offset from its laid-out place (for animations)."
    );
    property!(
        min_size,
        set_min_size,
        Vec2,
        "control.min_size",
        "Smallest size containers give it."
    );
    property!(
        expand,
        set_expand,
        bool,
        "control.expand",
        "Takes the free space in a box container."
    );

    /// Animates the alpha from `from` to `to`.
    pub fn fade(&mut self, from: f32, to: f32, seconds: f32) -> Result<(), Error> {
        self.tween_alpha(from, to, seconds, 0.0, Easing::EaseOut)
    }

    /// Slides in from `offset` to its laid-out place.
    pub fn slide_in(&mut self, offset: Vec2, seconds: f32) -> Result<(), Error> {
        self.tree.run(
            method!("control.slide_in"),
            &[self.node.to_value(), offset.to_value(), seconds.to_value()],
        )
    }

    /// Slides from its laid-out place to `offset`.
    pub fn slide_out(&mut self, offset: Vec2, seconds: f32) -> Result<(), Error> {
        self.tree.run(
            method!("control.slide_out"),
            &[self.node.to_value(), offset.to_value(), seconds.to_value()],
        )
    }

    /// Animates the alpha after `delay` seconds; animations queue up and run together.
    pub fn tween_alpha(
        &mut self,
        from: f32,
        to: f32,
        seconds: f32,
        delay: f32,
        easing: Easing,
    ) -> Result<(), Error> {
        self.tree.run(
            method!("control.tween_alpha"),
            &[
                self.node.to_value(),
                from.to_value(),
                to.to_value(),
                seconds.to_value(),
                delay.to_value(),
                easing.name().to_value(),
            ],
        )
    }

    /// Stops its animations where they are.
    pub fn stop_animations(&mut self) -> Result<(), Error> {
        self.tree
            .run(method!("control.stop_animations"), &[self.node.to_value()])
    }

    pub fn is_animating(&self) -> bool {
        self.tree
            .query(method!("control.is_animating"), &[self.node.to_value()])
            .unwrap_or(false)
    }
}

/// A node's `Label` ([`Tree::label`]).
#[derive(Debug)]
pub struct LabelRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl LabelRef<'_, '_> {
    pub fn text(&self) -> Option<String> {
        self.tree.get(self.node, "label.text")
    }

    /// Sets the text (does nothing if it is the same, so calling it every frame is cheap).
    pub fn set_text(&mut self, text: &str) -> Result<(), Error> {
        self.tree.set(self.node, "label.text", text)
    }

    /// The translation key the label shows (empty without one).
    pub fn key(&self) -> Option<String> {
        self.tree.get(self.node, "label.key")
    }

    /// Shows the text of `key` in the game's language, following language changes;
    /// `""` removes the key (the text then stays as it is).
    pub fn set_key(&mut self, key: &str) -> Result<(), Error> {
        self.tree.set(self.node, "label.key", key)
    }

    property!(
        font_size,
        set_font_size,
        f32,
        "label.font_size",
        "Text size in UI units."
    );
    property!(color, set_color, Color, "label.color", "Text color.");
    property!(
        outline_width,
        set_outline_width,
        f32,
        "label.outline_width",
        "Outline thickness (0 = none)."
    );
    property!(
        outline_color,
        set_outline_color,
        Color,
        "label.outline_color",
        "Outline color."
    );
    property!(
        autowrap,
        set_autowrap,
        bool,
        "label.autowrap",
        "Wraps lines at the control's width."
    );

    pub fn set_align(&mut self, horizontal: HAlign, vertical: VAlign) -> Result<(), Error> {
        self.tree
            .set(self.node, "label.h_align", horizontal.name())?;
        self.tree.set(self.node, "label.v_align", vertical.name())
    }
}

/// A node's `Button` ([`Tree::button`]).
#[derive(Debug)]
pub struct ButtonRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl ButtonRef<'_, '_> {
    pub fn text(&self) -> Option<String> {
        self.tree.get(self.node, "button.text")
    }

    pub fn set_text(&mut self, text: &str) -> Result<(), Error> {
        self.tree.set(self.node, "button.text", text)
    }

    /// The translation key the button shows (empty without one).
    pub fn key(&self) -> Option<String> {
        self.tree.get(self.node, "button.key")
    }

    /// Like [`LabelRef::set_key`].
    pub fn set_key(&mut self, key: &str) -> Result<(), Error> {
        self.tree.set(self.node, "button.key", key)
    }

    property!(
        disabled,
        set_disabled,
        bool,
        "button.disabled",
        "A disabled button ignores clicks."
    );
    property!(
        font_size,
        set_font_size,
        f32,
        "button.font_size",
        "Text size in UI units."
    );
}

/// A node's `ProgressBar` ([`Tree::progress_bar`]).
#[derive(Debug)]
pub struct ProgressBarRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl ProgressBarRef<'_, '_> {
    property!(
        value,
        set_value,
        f32,
        "progress.value",
        "Current value (between min and max)."
    );
    property!(min, set_min, f32, "progress.min", "Value of an empty bar.");
    property!(max, set_max, f32, "progress.max", "Value of a full bar.");
    property!(
        fill_color,
        set_fill_color,
        Color,
        "progress.fill_color",
        "Color of the filled part."
    );
    property!(
        background_color,
        set_background_color,
        Color,
        "progress.background_color",
        "Color of the empty part."
    );
    property!(
        corner_radius,
        set_corner_radius,
        f32,
        "progress.corner_radius",
        "Rounding of both parts."
    );
    property!(
        show_percentage,
        set_show_percentage,
        bool,
        "progress.show_percentage",
        "Writes the percentage on the bar."
    );
}

/// A node's `Panel` ([`Tree::panel`]).
#[derive(Debug)]
pub struct PanelRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl PanelRef<'_, '_> {
    property!(color, set_color, Color, "panel.color", "Fill color.");
    property!(
        corner_radius,
        set_corner_radius,
        f32,
        "panel.corner_radius",
        "Corner rounding."
    );
    property!(
        border_width,
        set_border_width,
        f32,
        "panel.border_width",
        "Border thickness (0 = none)."
    );
    property!(
        border_color,
        set_border_color,
        Color,
        "panel.border_color",
        "Border color."
    );
}

/// A node's `BoxContainer` ([`Tree::box_container`]).
#[derive(Debug)]
pub struct BoxContainerRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl BoxContainerRef<'_, '_> {
    property!(
        separation,
        set_separation,
        f32,
        "box.separation",
        "Space between children."
    );

    /// Where children sit across the box (a column's horizontal alignment).
    pub fn set_alignment(&mut self, alignment: HAlign) -> Result<(), Error> {
        self.tree.set(self.node, "box.alignment", alignment.name())
    }
}
