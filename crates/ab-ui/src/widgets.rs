//! Widget components. Each sits on a node next to a [`Control`](crate::Control).

use crate::text::{HAlign, TextBlock, TextStyle, VAlign};
use ab_core::{Color, Vec2};
use ab_rendering::TextureHandle;

/// Background box: fill, border, rounded corners and inner padding (Godot's
/// `StyleBoxFlat`). Sizes in UI units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleBox {
    pub color: Color,
    pub border_width: f32,
    pub border_color: Color,
    pub corner_radius: f32,
    /// Space between the box edge and its content: left, top, right, bottom.
    pub padding: [f32; 4],
}

impl StyleBox {
    pub fn flat(color: Color) -> Self {
        Self {
            color,
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            corner_radius: 0.0,
            padding: [0.0; 4],
        }
    }

    /// Draws nothing.
    pub fn empty() -> Self {
        Self::flat(Color::TRANSPARENT)
    }

    pub fn with_border(mut self, width: f32, color: Color) -> Self {
        self.border_width = width;
        self.border_color = color;
        self
    }

    pub fn with_corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    pub fn with_padding(mut self, horizontal: f32, vertical: f32) -> Self {
        self.padding = [horizontal, vertical, horizontal, vertical];
        self
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.color.a > 0.0 || (self.border_width > 0.0 && self.border_color.a > 0.0)
    }

    pub(crate) fn padding_size(&self) -> Vec2 {
        Vec2::new(
            self.padding[0] + self.padding[2],
            self.padding[1] + self.padding[3],
        )
    }
}

impl Default for StyleBox {
    fn default() -> Self {
        Self::flat(Color::rgba(0.03, 0.035, 0.05, 0.85))
            .with_corner_radius(6.0)
            .with_padding(12.0, 8.0)
    }
}

/// Shaped text cached between frames; reshaped only when its inputs change.
#[derive(Debug, Clone, Default)]
pub(crate) struct TextCache {
    key: Option<(String, TextStyle, Option<u32>, HAlign, u64)>,
    pub(crate) block: TextBlock,
}

impl TextCache {
    pub(crate) fn get(
        &mut self,
        fonts: &crate::Fonts,
        text: &str,
        style: &TextStyle,
        wrap_width: Option<f32>,
        align: HAlign,
    ) -> &TextBlock {
        // Widths are compared at 1/64 unit so layout jitter does not reshape every frame.
        let wrap_key = wrap_width.map(|w| (w * 64.0).round() as u32);
        let generation = fonts.generation();
        let stale = self.key.as_ref().is_none_or(|(t, s, w, a, g)| {
            t != text || s != style || *w != wrap_key || *a != align || *g != generation
        });
        if stale {
            self.block = crate::text::layout_text(fonts, text, style, wrap_width, align);
            self.key = Some((text.to_owned(), style.clone(), wrap_key, align, generation));
        }
        &self.block
    }
}

/// Root of a layer of UI drawn above the 3D scene; higher `layer`s draw on top (Godot's
/// `CanvasLayer`). Controls outside any layer are on layer 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CanvasLayer {
    pub layer: i32,
}

/// A background box (Godot's `Panel`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Panel {
    pub style: StyleBox,
}

/// Text (Godot's `Label`).
#[derive(Debug, Clone, Default)]
pub struct Label {
    pub text: String,
    pub style: TextStyle,
    pub h_align: HAlign,
    pub v_align: VAlign,
    /// Wrap lines to the control's width.
    pub autowrap: bool,
    pub(crate) cache: TextCache,
}

impl Label {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    pub fn with_style(mut self, style: TextStyle) -> Self {
        self.style = style;
        self
    }

    pub fn with_align(mut self, h_align: HAlign, v_align: VAlign) -> Self {
        self.h_align = h_align;
        self.v_align = v_align;
        self
    }

    pub fn with_autowrap(mut self, autowrap: bool) -> Self {
        self.autowrap = autowrap;
        self
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }
}

/// A clickable button with a text caption (Godot's `Button`). React to clicks with
/// [`Button::just_pressed`] or the [`UiEvent::Pressed`](crate::UiEvent) event.
#[derive(Debug, Clone)]
pub struct Button {
    pub text: String,
    pub text_style: TextStyle,
    pub normal: StyleBox,
    pub hover: StyleBox,
    pub pressed: StyleBox,
    pub disabled_style: StyleBox,
    /// Drawn over the current style while the button has focus.
    pub focus: StyleBox,
    pub disabled: bool,
    pub(crate) hovered: bool,
    pub(crate) held: bool,
    pub(crate) focused: bool,
    pub(crate) clicked: bool,
    pub(crate) cache: TextCache,
}

impl Button {
    /// Signal emitted (on the button node) when it is activated.
    pub const PRESSED: &'static str = "pressed";

    pub fn new(text: impl Into<String>) -> Self {
        let normal = StyleBox::flat(Color::from_srgb8(52, 58, 74, 235))
            .with_corner_radius(6.0)
            .with_padding(16.0, 8.0);
        Self {
            text: text.into(),
            text_style: TextStyle::new(22.0),
            normal,
            hover: StyleBox {
                color: Color::from_srgb8(72, 82, 104, 245),
                ..normal
            },
            pressed: StyleBox {
                color: Color::from_srgb8(36, 40, 52, 255),
                ..normal
            },
            disabled_style: StyleBox {
                color: Color::from_srgb8(40, 42, 48, 160),
                ..normal
            },
            focus: StyleBox::empty()
                .with_border(2.0, Color::from_srgb8(140, 190, 255, 255))
                .with_corner_radius(6.0),
            disabled: false,
            hovered: false,
            held: false,
            focused: false,
            clicked: false,
            cache: TextCache::default(),
        }
    }

    /// Whether the button was activated this frame (mouse release over it, or Enter /
    /// Space / gamepad A while focused).
    pub fn just_pressed(&self) -> bool {
        self.clicked
    }

    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Held down by the mouse.
    pub fn is_held(&self) -> bool {
        self.held
    }

    pub fn has_focus(&self) -> bool {
        self.focused
    }

    pub(crate) fn current_style(&self) -> &StyleBox {
        if self.disabled {
            &self.disabled_style
        } else if self.held {
            &self.pressed
        } else if self.hovered {
            &self.hover
        } else {
            &self.normal
        }
    }
}

/// How a [`TextureRect`] fits its texture into its rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StretchMode {
    /// Fill the rect, ignoring the aspect ratio.
    #[default]
    Scale,
    /// Largest size that fits, centered, keeping the aspect ratio.
    KeepAspectCentered,
    /// Cover the whole rect keeping the aspect ratio (the excess is clipped).
    KeepAspectCovered,
}

/// An image (Godot's `TextureRect`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureRect {
    pub texture: TextureHandle,
    pub stretch: StretchMode,
    pub tint: Color,
}

impl TextureRect {
    pub fn new(texture: TextureHandle) -> Self {
        Self {
            texture,
            stretch: StretchMode::default(),
            tint: Color::WHITE,
        }
    }
}

/// A horizontal bar filled in proportion to `value` (Godot's `ProgressBar`).
#[derive(Debug, Clone)]
pub struct ProgressBar {
    pub min: f32,
    pub max: f32,
    pub value: f32,
    pub background: StyleBox,
    pub fill: StyleBox,
    /// Draw "NN%" centered on the bar.
    pub show_percentage: bool,
    pub text_style: TextStyle,
    pub(crate) cache: TextCache,
}

impl ProgressBar {
    pub fn new(min: f32, max: f32, value: f32) -> Self {
        Self {
            min,
            max,
            value,
            background: StyleBox::flat(Color::rgba(0.02, 0.02, 0.03, 0.8))
                .with_corner_radius(4.0)
                .with_border(1.0, Color::rgba(1.0, 1.0, 1.0, 0.15)),
            fill: StyleBox::flat(Color::from_srgb8(86, 196, 120, 255)).with_corner_radius(4.0),
            show_percentage: false,
            text_style: TextStyle::new(16.0),
            cache: TextCache::default(),
        }
    }

    /// Fill fraction in `0..=1`.
    pub fn ratio(&self) -> f32 {
        let span = self.max - self.min;
        if span.abs() <= f32::EPSILON {
            return 0.0;
        }
        ((self.value - self.min) / span).clamp(0.0, 1.0)
    }
}

impl Default for ProgressBar {
    fn default() -> Self {
        Self::new(0.0, 100.0, 0.0)
    }
}

/// Stacks its children vertically or horizontally (Godot's `VBoxContainer` /
/// `HBoxContainer`). Children get their minimum size along the axis plus a share of the
/// leftover space if they `expand`, and fill the cross axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxContainer {
    pub vertical: bool,
    /// Gap between children in UI units.
    pub separation: f32,
    /// Where children go along the axis when none expands.
    pub alignment: HAlign,
}

impl BoxContainer {
    pub fn vertical() -> Self {
        Self {
            vertical: true,
            separation: 6.0,
            alignment: HAlign::Left,
        }
    }

    pub fn horizontal() -> Self {
        Self {
            vertical: false,
            ..Self::vertical()
        }
    }

    pub fn with_separation(mut self, separation: f32) -> Self {
        self.separation = separation;
        self
    }

    pub fn with_alignment(mut self, alignment: HAlign) -> Self {
        self.alignment = alignment;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontId, Fonts};

    #[test]
    fn text_cache_reshapes_when_fonts_change() {
        let mut fonts = Fonts::new();
        let mut cache = TextCache::default();
        let style = TextStyle::new(20.0).with_font(FontId(1));
        let font_of = |cache: &mut TextCache, fonts: &Fonts| {
            cache.get(fonts, "Hi", &style, None, HAlign::Left).lines[0].glyphs[0].font
        };
        // Font 1 is not loaded yet: the default font stands in.
        assert_eq!(font_of(&mut cache, &fonts), FontId::DEFAULT);
        let bold = fonts
            .add(include_bytes!("../fonts/Prompt-SemiBold.ttf").to_vec())
            .unwrap();
        assert_eq!(bold, FontId(1));
        assert_eq!(
            font_of(&mut cache, &fonts),
            bold,
            "reshaped with the new font"
        );
        let generation = fonts.generation();
        fonts.set_fallbacks(vec![bold]);
        assert_ne!(fonts.generation(), generation);
    }

    #[test]
    fn progress_ratio_clamps() {
        let mut bar = ProgressBar::new(10.0, 20.0, 15.0);
        assert_eq!(bar.ratio(), 0.5);
        bar.value = 30.0;
        assert_eq!(bar.ratio(), 1.0);
        bar.max = 10.0;
        assert_eq!(bar.ratio(), 0.0);
    }

    #[test]
    fn button_style_follows_state() {
        let mut button = Button::new("Go");
        assert_eq!(button.current_style(), &button.normal);
        button.hovered = true;
        assert_eq!(button.current_style(), &button.hover);
        button.held = true;
        assert_eq!(button.current_style(), &button.pressed);
        button.disabled = true;
        assert_eq!(button.current_style(), &button.disabled_style);
    }
}
