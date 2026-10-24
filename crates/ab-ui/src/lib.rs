//! Screen-space UI for HUDs and menus: Godot-style controls, anchors and box containers,
//! pointer/keyboard/gamepad input, simple animations, and text drawn from multi-channel
//! signed distance fields so it stays sharp at any resolution.
//!
//! * Nodes ([`UiNodes`]): `CanvasLayer`, `Control`, `Panel`, `Label`, `Button`,
//!   `TextureRect`, `ProgressBar`, `VBoxContainer` / `HBoxContainer`.
//! * Layout is in UI units at [`UiSettings::reference_size`] (1920 × 1080 by default) and
//!   scaled to the screen; each [`Control`] is placed by anchors + offsets inside its parent,
//!   or by its [`BoxContainer`].
//! * Text ([`Fonts`]): shaped with rustybuzz (kerning, ligatures, Thai vowel and tone-mark
//!   stacking), wrapped, and drawn from glyphs baked on demand into an MTSDF atlas, with
//!   outline, shadow and glow. The built-in font (Prompt, SIL OFL 1.1) covers Latin and
//!   Thai.
//! * Each frame: [`ui_input_system`] (`PreUpdate`) handles hover, clicks and focus using last
//!   frame's layout; [`ui_animation_system`] (`Update`) runs fades and slides; before
//!   rendering, [`prepare_ui`] lays everything out for the target size and fills the
//!   renderer's [`UiDrawList`](ab_rendering::UiDrawList).
//!
//! ```
//! use ab_core::Vec2;
//! use ab_ui::{Control, Label, LayoutPreset, TextStyle, UiNodes};
//! use ab_scene::SceneTree;
//!
//! let mut tree = SceneTree::new();
//! ab_ui::register(tree.world_mut());
//! let hud = tree.create_canvas_layer("Hud", 1);
//! tree.add_child(tree.root(), hud).unwrap();
//! let score = tree.create_label(
//!     "Score",
//!     Control::new().with_preset(LayoutPreset::TopRight, Vec2::new(300.0, 50.0), 24.0),
//!     Label::new("Score: 0").with_style(TextStyle::new(36.0)),
//! );
//! tree.add_child(hud, score).unwrap();
//!
//! ab_ui::prepare_ui(tree.world(), 1920, 1080);
//! let rect = tree.component::<Control>(score).unwrap().rect();
//! assert_eq!(rect.position, Vec2::new(1920.0 - 24.0 - 300.0, 24.0));
//! ```

mod anim;
mod control;
mod font;
mod frame;
mod input;
mod nodes;
mod text;
mod widgets;

/// The built-in font's license (SIL Open Font License 1.1). It must ship with every copy of a
/// game: exports put it in the game's assets, and `--licenses` shows it.
pub const FONT_LICENSE: &str = include_str!("../fonts/OFL.txt");
pub use anim::{ui_animation_system, Easing, TweenTarget, UiAnimator, UiTween};
pub use control::{Control, LayoutPreset, MouseFilter, Rect};
pub use font::{FontId, Fonts, LineMetrics, ShapedGlyph, ShapedRun};
pub use frame::{prepare_ui, ScaleMode, UiSettings};
pub use input::{ui_input_system, UiEvent, UiState};
pub use nodes::UiNodes;
pub use text::{layout_text, HAlign, TextBlock, TextLine, TextShadow, TextStyle, VAlign};
pub use widgets::{
    BoxContainer, Button, CanvasLayer, Label, Panel, ProgressBar, StretchMode, StyleBox,
    TextureRect,
};

use ab_ecs::World;
use ab_rendering::UiDrawList;

/// Class names of the UI node types.
pub mod class {
    pub const CANVAS_LAYER: &str = "CanvasLayer";
    pub const CONTROL: &str = "Control";
    pub const PANEL: &str = "Panel";
    pub const LABEL: &str = "Label";
    pub const BUTTON: &str = "Button";
    pub const TEXTURE_RECT: &str = "TextureRect";
    pub const PROGRESS_BAR: &str = "ProgressBar";
    pub const VBOX_CONTAINER: &str = "VBoxContainer";
    pub const HBOX_CONTAINER: &str = "HBoxContainer";
}

/// Inserts the UI resources ([`Fonts`], [`UiSettings`], [`UiState`], [`UiDrawList`]) that
/// are missing.
pub fn register(world: &mut World) {
    world.resource_or_insert_with(Fonts::new);
    world.resource_or_insert_with(UiSettings::default);
    world.resource_or_insert_with(UiState::new);
    world.resource_or_insert_with(UiDrawList::default);
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UiError {
    #[error("invalid font: {0}")]
    InvalidFont(String),
}
