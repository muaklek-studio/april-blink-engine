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
    /// Old steps dropped over [`LIMIT`]: [`History::undo_len`] counts them, so a length
    /// taken before a group of edits still finds the group after the oldest steps go.
    dropped: usize,
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
            self.dropped += 1;
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

    /// Undo steps recorded, counting those dropped over the limit (a position for
    /// [`History::truncate_undo`], which stays valid while new steps push old ones out).
    pub fn undo_len(&self) -> usize {
        self.dropped + self.undo.len()
    }

    /// Forgets the undo steps after the first `len` (they are replaced by one step).
    pub fn truncate_undo(&mut self, len: usize) {
        self.undo.truncate(len.saturating_sub(self.dropped));
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
        self.dropped = 0;
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

    /// `Editor::as_one_step` groups edits by the undo length before them: with a full
    /// history the length used to stay at the limit, so a group was never folded.
    #[test]
    fn grouping_works_with_a_full_history() {
        let mut history = History::default();
        for i in 0..LIMIT {
            history.push(snap(&format!("old{i}")));
        }
        let steps = history.undo_len();
        history.push(snap("a"));
        history.push(snap("b"));
        history.push(snap("c"));
        assert!(history.undo_len() > steps, "the group's steps are seen");
        history.truncate_undo(steps);
        history.push(snap("before"));
        assert_eq!(history.undo(snap("now")), Some(snap("before")), "one step");
        // The older steps are still there (minus those pushed out by the group).
        assert_eq!(
            history.undo(snap("x")),
            Some(snap(&format!("old{}", LIMIT - 1)))
        );
        assert!(history.undo.len() <= LIMIT);
    }
}
