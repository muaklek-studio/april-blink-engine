use ab_core::{Color, Vec2};

/// Axis-aligned rectangle in UI units (y down).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub position: Vec2,
    pub size: Vec2,
}

impl Rect {
    pub const fn new(position: Vec2, size: Vec2) -> Self {
        Self { position, size }
    }

    pub fn from_min_max(min: Vec2, max: Vec2) -> Self {
        Self::new(min, (max - min).max(Vec2::ZERO))
    }

    pub fn min(&self) -> Vec2 {
        self.position
    }

    pub fn max(&self) -> Vec2 {
        self.position + self.size
    }

    pub fn center(&self) -> Vec2 {
        self.position + self.size * 0.5
    }

    pub fn contains(&self, point: Vec2) -> bool {
        point.cmpge(self.min()).all() && point.cmplt(self.max()).all()
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        Rect::from_min_max(self.min().max(other.min()), self.max().min(other.max()))
    }

    /// Shrinks by `amount` on each side: left, top, right, bottom.
    pub fn inset(&self, [left, top, right, bottom]: [f32; 4]) -> Rect {
        Rect::from_min_max(
            self.min() + Vec2::new(left, top),
            self.max() - Vec2::new(right, bottom),
        )
    }
}

/// Godot's layout presets: where a control is anchored in its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Whether a control takes mouse input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseFilter {
    /// Receives hover and clicks and blocks controls (and the game) below.
    #[default]
    Stop,
    /// Lets the pointer through to what is below.
    Ignore,
}

/// Base component of every UI node (Godot's `Control`).
///
/// The rect is placed relative to the parent control's rect (or the screen for top-level
/// controls): each edge sits at `parent edge + anchor × parent size + offset`. Units are UI
/// units: pixels at the reference resolution of [`UiSettings`](crate::UiSettings).
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    /// Anchor fractions of the parent rect: left, top, right, bottom.
    pub anchors: [f32; 4],
    /// Edge offsets from the anchors: left, top, right, bottom.
    pub offsets: [f32; 4],
    /// Smallest size the control may get (Godot's `custom_minimum_size`).
    pub min_size: Vec2,
    /// In a box container, take a share of the leftover space along its axis.
    pub expand: bool,
    pub mouse_filter: MouseFilter,
    /// Can receive keyboard/gamepad focus.
    pub focusable: bool,
    /// Color multiplier for this control and its children (fades use the alpha).
    pub modulate: Color,
    /// Visual offset applied after layout (slide animations), in UI units.
    pub translation: Vec2,
    /// Children are clipped to this control's rect.
    pub clip_contents: bool,
    pub(crate) rect: Rect,
    pub(crate) content_min: Vec2,
}

impl Control {
    /// Anchored to the top-left corner with zero size.
    pub fn new() -> Self {
        Self {
            anchors: [0.0; 4],
            offsets: [0.0; 4],
            min_size: Vec2::ZERO,
            expand: false,
            mouse_filter: MouseFilter::Stop,
            focusable: false,
            modulate: Color::WHITE,
            translation: Vec2::ZERO,
            clip_contents: false,
            rect: Rect::default(),
            content_min: Vec2::ZERO,
        }
    }

    /// Fills the parent.
    pub fn full_rect() -> Self {
        Self::new().with_preset(LayoutPreset::FullRect, Vec2::ZERO, 0.0)
    }

    /// Places the control like Godot's layout presets: `size` for the non-stretched axes,
    /// `margin` from the anchored edges.
    pub fn with_preset(mut self, preset: LayoutPreset, size: Vec2, margin: f32) -> Self {
        use LayoutPreset::*;
        let (ax, aw) = match preset {
            TopLeft | BottomLeft | CenterLeft | LeftWide => (0.0, 0.0),
            TopRight | BottomRight | CenterRight | RightWide => (1.0, 1.0),
            CenterTop | CenterBottom | Center | VCenterWide => (0.5, 0.5),
            TopWide | BottomWide | HCenterWide | FullRect => (0.0, 1.0),
        };
        let (ay, ah) = match preset {
            TopLeft | TopRight | CenterTop | TopWide => (0.0, 0.0),
            BottomLeft | BottomRight | CenterBottom | BottomWide => (1.0, 1.0),
            CenterLeft | CenterRight | Center | HCenterWide => (0.5, 0.5),
            LeftWide | RightWide | VCenterWide | FullRect => (0.0, 1.0),
        };
        self.anchors = [ax, ay, aw, ah];
        let axis = |start: f32, end: f32, size: f32| -> (f32, f32) {
            if start != end {
                (margin, -margin) // stretched
            } else if start == 0.0 {
                (margin, margin + size)
            } else if start == 1.0 {
                (-margin - size, -margin)
            } else {
                (-size * 0.5, size * 0.5)
            }
        };
        let (left, right) = axis(ax, aw, size.x);
        let (top, bottom) = axis(ay, ah, size.y);
        self.offsets = [left, top, right, bottom];
        self
    }

    /// Absolute placement inside the parent: top-left `position`, `size`.
    pub fn at(position: Vec2, size: Vec2) -> Self {
        let mut control = Self::new();
        control.offsets = [
            position.x,
            position.y,
            position.x + size.x,
            position.y + size.y,
        ];
        control
    }

    pub fn with_min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = min_size;
        self
    }

    pub fn with_expand(mut self, expand: bool) -> Self {
        self.expand = expand;
        self
    }

    pub fn with_mouse_filter(mut self, filter: MouseFilter) -> Self {
        self.mouse_filter = filter;
        self
    }

    /// Global rect as of the last layout, in UI units (including `translation`).
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Smallest size the control can take: `min_size` or what its content needs.
    pub fn combined_min_size(&self) -> Vec2 {
        self.min_size.max(self.content_min)
    }
}

impl Default for Control {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ops() {
        let r = Rect::new(Vec2::new(10.0, 10.0), Vec2::new(20.0, 10.0));
        assert!(r.contains(Vec2::new(10.0, 10.0)));
        assert!(!r.contains(Vec2::new(30.0, 15.0)));
        let i = r.intersect(&Rect::new(Vec2::new(25.0, 0.0), Vec2::new(100.0, 15.0)));
        assert_eq!(i, Rect::new(Vec2::new(25.0, 10.0), Vec2::new(5.0, 5.0)));
        assert_eq!(r.inset([1.0, 2.0, 3.0, 4.0]).size, Vec2::new(16.0, 4.0));
    }

    #[test]
    fn presets() {
        let c = Control::new().with_preset(LayoutPreset::BottomRight, Vec2::new(100.0, 40.0), 8.0);
        assert_eq!(c.anchors, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(c.offsets, [-108.0, -48.0, -8.0, -8.0]);
        let c = Control::new().with_preset(LayoutPreset::Center, Vec2::new(100.0, 40.0), 0.0);
        assert_eq!(c.offsets, [-50.0, -20.0, 50.0, 20.0]);
        let c = Control::new().with_preset(LayoutPreset::TopWide, Vec2::new(0.0, 40.0), 4.0);
        assert_eq!(c.anchors, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(c.offsets, [4.0, 4.0, -4.0, 44.0]);
        assert_eq!(Control::full_rect().anchors, [0.0, 0.0, 1.0, 1.0]);
    }
}
