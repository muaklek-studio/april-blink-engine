//! Undo/redo by scene snapshots: robust for every kind of edit, cheap for editor-sized
//! scenes.

use ab_engine::scene_file::SceneFile;

/// Undo steps kept.
const LIMIT: usize = 100;

/// A scene state plus what was selected (by path, since node handles change on restore).
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub scene: SceneFile,
    /// The active node.
    pub selection: Option<String>,
    /// Every selected node.
    pub selected: Vec<String>,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    /// Records the state before an edit.
    pub fn push(&mut self, before: Snapshot) {
        if self.undo.last() == Some(&before) {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The state to go back to; `current` becomes redoable.
    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    /// Undo steps recorded.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Forgets the undo steps after the first `len` (they are replaced by one step).
    pub fn truncate_undo(&mut self, len: usize) {
        self.undo.truncate(len);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(name: &str) -> Snapshot {
        Snapshot {
            scene: SceneFile {
                nodes: vec![ab_engine::scene_file::NodeDesc {
                    name: name.into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            selection: None,
            selected: Vec::new(),
        }
    }

    #[test]
    fn undo_redo_cycle() {
        let mut history = History::default();
        history.push(snap("a"));
        history.push(snap("a")); // duplicates collapse
        history.push(snap("b"));
        assert_eq!(history.undo(snap("c")), Some(snap("b")));
        assert_eq!(history.undo(snap("b")), Some(snap("a")));
        assert!(!history.can_undo());
        assert_eq!(history.redo(snap("a")), Some(snap("b")));
        // A new edit drops the redo branch.
        history.push(snap("x"));
        assert!(!history.can_redo());
    }
}
