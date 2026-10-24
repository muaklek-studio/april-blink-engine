//! Nodes with an editor of their own. A UI node (a canvas layer), an AnimationPlayer and an
//! effect are each worked on in a window made for them, opened by double-clicking the node
//! (or from the Window menu): the scene tree shows them as one node, not the controls or
//! layers inside, which belong to their editor.

use crate::state::Editor;
use ab_engine::animation::AnimationPlayer;
use ab_engine::scene::NodeHandle;

/// A particle layer's class.
const PARTICLES: &str = "Particles3D";

/// What a node's own editor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// A canvas layer: the UI Editor, scoped to it.
    Ui,
    /// An AnimationPlayer: the Animation Editor, on its clips.
    Animation,
    /// Particles, or a node made of particle layers: the Effects Editor.
    Effect,
}

impl Feature {
    /// The editor `node` opens in, if any.
    pub fn of(editor: &Editor, node: NodeHandle) -> Option<Feature> {
        let tree = editor.tree();
        if !tree.is_valid(node) || node == editor.scene_root {
            return None;
        }
        let class = tree.class_name(node);
        if class == Some(ab_engine::ui::class::CANVAS_LAYER) {
            return Some(Feature::Ui);
        }
        if tree.component::<AnimationPlayer>(node).is_some() {
            return Some(Feature::Animation);
        }
        if class == Some(PARTICLES) || is_effect_root(editor, node) {
            return Some(Feature::Effect);
        }
        None
    }

    /// Whether the node's children are edited in its editor, not in the scene tree.
    pub fn owns_children(self) -> bool {
        matches!(self, Feature::Ui | Feature::Effect)
    }
}

/// A node holding particle layers and nothing else (a layered effect).
fn is_effect_root(editor: &Editor, node: NodeHandle) -> bool {
    let tree = editor.tree();
    let children = tree.children(node);
    !children.is_empty()
        && children
            .iter()
            .all(|&c| tree.class_name(c) == Some(PARTICLES))
}

/// The node whose editor holds `node`: the UI node or effect it is inside, if any (the
/// node itself when it is one).
pub fn owner(editor: &Editor, node: NodeHandle) -> Option<NodeHandle> {
    let tree = editor.tree();
    std::iter::successors(Some(node), |&n| tree.parent(n))
        .take_while(|&n| n != editor.scene_root)
        .filter(|&n| Feature::of(editor, n).is_some_and(Feature::owns_children))
        .last()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create::{effect, UI};

    #[test]
    fn ui_nodes_players_and_effects_open_in_their_editors() {
        let assets = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut editor = Editor::new(assets, None, ab_engine::audio::AudioOutput::Silent);
        let add = |editor: &mut Editor, scene| {
            editor.select(None);
            editor.add_scene(scene, None);
            editor.selection.unwrap()
        };
        let ui = add(&mut editor, UI[0].scene());
        editor.add_scene(UI[2].scene(), None);
        let control = editor.selection.unwrap();
        let player = add(
            &mut editor,
            ab_engine::scene_file::SceneFile {
                environment: None,
                nodes: vec![crate::panels::animation::new_player()],
            },
        );
        let sparks = add(&mut editor, effect("sparks"));

        assert_eq!(Feature::of(&editor, ui), Some(Feature::Ui));
        assert_eq!(Feature::of(&editor, player), Some(Feature::Animation));
        assert_eq!(Feature::of(&editor, sparks), Some(Feature::Effect));
        assert_eq!(Feature::of(&editor, control), None);
        assert_eq!(Feature::of(&editor, editor.scene_root), None);
        // A control belongs to its UI node's editor; a player holds nothing.
        assert_eq!(owner(&editor, control), Some(ui));
        assert_eq!(owner(&editor, player), None);
    }
}
