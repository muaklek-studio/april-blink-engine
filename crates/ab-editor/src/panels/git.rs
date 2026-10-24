//! Source Control: the project's git repository. Branch, sync with the remote, stage and
//! commit changes, read diffs and recent history. Git runs on its own thread
//! ([`GitClient`]); this panel only shows its latest answers and queues commands.

use super::section;
use crate::git::{FileChange, FileState, GitClient, GitOp, RepoState, RepoStatus};
use crate::theme;
use eframe::egui::{self, Key, RichText};
use egui_phosphor::regular as icon;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the status is re-read while the panel is open.
const REFRESH_EVERY: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct GitPanel {
    message: String,
    amend: bool,
    /// A commit is running: the message is cleared once it is made, kept if it fails.
    committing: Option<u64>,
    /// Name typed for a new branch (the field is open while `Some`).
    new_branch: Option<String>,
    /// List only changes inside the project folder (when the repository is bigger).
    whole_repository: bool,
    /// Discard waiting for confirmation: (tracked, untracked) paths.
    confirm_discard: Option<(Vec<String>, Vec<String>)>,
}

/// The folder a game project lives in: the parent of its asset folder when that is the
/// project (its `game.toml`), else the asset folder itself.
pub fn project_root(assets: &Path) -> PathBuf {
    match assets.parent() {
        Some(parent) if parent.join(ab_project::GAME_FILE).is_file() => parent.to_path_buf(),
        _ => assets.to_path_buf(),
    }
}

/// State color of a change.
pub fn state_color(state: FileState) -> egui::Color32 {
    match state {
        FileState::Modified | FileState::TypeChanged => theme::WARN,
        FileState::Added | FileState::Untracked | FileState::Copied => theme::PLAY,
        FileState::Deleted | FileState::Conflicted => theme::ERROR,
        FileState::Renamed => theme::ACCENT,
    }
}

enum Action {
    Refresh,
    Op(GitOp),
    Diff {
        path: String,
        staged: bool,
        untracked: bool,
    },
    Discard(Vec<String>, Vec<String>),
}

impl GitPanel {
    pub fn show(&mut self, ui: &mut egui::Ui, git: &mut GitClient, project: &Path) {
        git.refresh_every(REFRESH_EVERY);
        if let Some(made) = self.committing.and_then(|ticket| git.commit_made(ticket)) {
            self.committing = None;
            if made {
                self.message.clear();
                self.amend = false;
            }
        }
        let root = project_root(project);
        let mut action = None;
        let status = match &git.state {
            RepoState::Unknown => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Reading the repository…").color(theme::TEXT_DIM));
                });
                return;
            }
            RepoState::NoRepository => {
                if let Some(op) = no_repository(ui, git.is_busy(), &root) {
                    git.run(op);
                }
                return;
            }
            RepoState::Failed(err) => {
                ui.label(RichText::new(format!("{}  {err}", icon::X_CIRCLE)).color(theme::ERROR));
                if ui
                    .button(format!("{}  Retry", icon::ARROW_CLOCKWISE))
                    .clicked()
                {
                    git.refresh();
                }
                return;
            }
            RepoState::Ready(status) => status.as_ref(),
        };

        // Changes inside the project only, unless asked for the whole repository.
        let scope = root
            .strip_prefix(&status.root)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|p| !p.is_empty());
        self.toolbar(ui, git, status, &mut action);
        if let Some(scope) = &scope {
            ui.checkbox(&mut self.whole_repository, "Whole repository")
                .on_hover_text(format!(
                    "Show changes outside the project folder ({scope}) too"
                ));
        }
        ui.add_space(2.0);

        let whole = self.whole_repository;
        let visible = |c: &&FileChange| {
            whole
                || scope
                    .as_deref()
                    .is_none_or(|s| c.path.starts_with(s) && c.path[s.len()..].starts_with('/'))
        };

        let staged: Vec<&FileChange> = status.staged().filter(visible).collect();
        let unstaged: Vec<&FileChange> = status.unstaged().filter(visible).collect();
        self.commit_box(ui, git.is_busy(), &staged, &unstaged, &mut action);
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if staged.is_empty() && unstaged.is_empty() {
                    ui.label(
                        RichText::new(format!("{}  No changes", icon::CHECK))
                            .color(theme::TEXT_DIM),
                    );
                }
                if !staged.is_empty() {
                    let title = format!("Staged Changes ({})", staged.len());
                    section(ui, "git.staged", icon::GIT_COMMIT, &title, |ui| {
                        if ui
                            .small_button(format!("{}  Unstage all", icon::MINUS))
                            .clicked()
                        {
                            action = Some(Action::Op(GitOp::Unstage(paths(&staged))));
                        }
                        for change in &staged {
                            file_row(ui, git, change, true, &mut action);
                        }
                    });
                }
                if !unstaged.is_empty() {
                    let title = format!("Changes ({})", unstaged.len());
                    section(ui, "git.changes", icon::GIT_DIFF, &title, |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .small_button(format!("{}  Stage all", icon::PLUS))
                                .clicked()
                            {
                                action = Some(Action::Op(GitOp::Stage(paths(&unstaged))));
                            }
                            if ui
                                .small_button(format!(
                                    "{}  Discard all",
                                    icon::ARROW_COUNTER_CLOCKWISE
                                ))
                                .clicked()
                            {
                                let (tracked, untracked) = split_untracked(&unstaged);
                                action = Some(Action::Discard(tracked, untracked));
                            }
                        });
                        for change in &unstaged {
                            file_row(ui, git, change, false, &mut action);
                        }
                    });
                }
                if let Some(diff) = &git.diff {
                    let title = format!(
                        "{} ({})",
                        diff.path,
                        if diff.staged { "staged" } else { "changes" }
                    );
                    section(ui, "git.diff", icon::GIT_DIFF, &title, |ui| {
                        match &diff.text {
                            Ok(text) => diff_view(ui, text),
                            Err(err) => {
                                ui.label(RichText::new(err).color(theme::ERROR));
                            }
                        }
                    });
                }
                if !status.log.is_empty() {
                    let title = format!("History ({})", status.log.len());
                    section(
                        ui,
                        "git.history",
                        icon::CLOCK_COUNTER_CLOCKWISE,
                        &title,
                        |ui| {
                            for commit in &status.log {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(&commit.hash)
                                            .monospace()
                                            .color(theme::ACCENT),
                                    );
                                    ui.add(egui::Label::new(&commit.subject).truncate())
                                        .on_hover_text(&commit.subject);
                                });
                                ui.label(
                                    RichText::new(format!("{} · {}", commit.author, commit.when))
                                        .small()
                                        .color(theme::TEXT_DIM),
                                );
                                ui.add_space(2.0);
                            }
                        },
                    );
                }
            });

        self.confirm(ui, &mut action);
        match action {
            Some(Action::Refresh) => git.refresh(),
            Some(Action::Op(op)) => {
                if matches!(op, GitOp::Commit { .. }) {
                    self.committing = Some(git.commit(op));
                } else {
                    git.run(op);
                }
            }
            Some(Action::Diff {
                path,
                staged,
                untracked,
            }) => git.show_diff(&path, staged, untracked),
            Some(Action::Discard(tracked, untracked)) => {
                self.confirm_discard = Some((tracked, untracked));
            }
            None => {}
        }
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        git: &GitClient,
        status: &RepoStatus,
        action: &mut Option<Action>,
    ) {
        let busy = git.is_busy();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::GIT_BRANCH).color(theme::ACCENT));
            let current = status.branch.label();
            egui::ComboBox::from_id_salt("git.branch")
                .selected_text(&current)
                .width(150.0)
                .show_ui(ui, |ui| {
                    for branch in &status.branches {
                        let here = status.branch.head.as_deref() == Some(branch.as_str());
                        if ui.selectable_label(here, branch).clicked() && !here {
                            *action = Some(Action::Op(GitOp::Switch(branch.clone())));
                        }
                    }
                    ui.separator();
                    if ui.button(format!("{}  New branch…", icon::PLUS)).clicked() {
                        self.new_branch = Some(String::new());
                    }
                });
            let branch = &status.branch;
            if branch.upstream.is_some() {
                ui.label(
                    RichText::new(format!(
                        "{} {}  {} {}",
                        icon::ARROW_UP,
                        branch.ahead,
                        icon::ARROW_DOWN,
                        branch.behind
                    ))
                    .color(theme::TEXT_DIM),
                )
                .on_hover_text(format!(
                    "{} to push, {} to pull ({})",
                    branch.ahead,
                    branch.behind,
                    branch.upstream.as_deref().unwrap_or_default()
                ));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has_remote = !status.remotes.is_empty();
                let tip = |what: &str| {
                    if has_remote {
                        what.to_owned()
                    } else {
                        "No remote configured (git remote add origin <url>)".to_owned()
                    }
                };
                let enabled = !busy && has_remote;
                if ui
                    .add_enabled(enabled, egui::Button::new(icon::CLOUD_ARROW_UP))
                    .on_hover_text(tip("Push"))
                    .clicked()
                {
                    *action = Some(Action::Op(GitOp::Push));
                }
                if ui
                    .add_enabled(enabled, egui::Button::new(icon::CLOUD_ARROW_DOWN))
                    .on_hover_text(tip("Pull (fast-forward only)"))
                    .clicked()
                {
                    *action = Some(Action::Op(GitOp::Pull { rebase: false }));
                }
                if ui
                    .add_enabled(enabled, egui::Button::new(icon::ARROWS_CLOCKWISE))
                    .on_hover_text(tip("Fetch"))
                    .clicked()
                {
                    *action = Some(Action::Op(GitOp::Fetch));
                }
                if ui
                    .add_enabled(!busy, egui::Button::new(icon::ARROW_CLOCKWISE))
                    .on_hover_text("Refresh")
                    .clicked()
                {
                    *action = Some(Action::Refresh);
                }
                if let Some(op) = git.remote_op() {
                    ui.label(RichText::new(format!("{op}…")).color(theme::TEXT_DIM));
                    ui.spinner();
                } else if busy {
                    ui.spinner();
                }
            });
        });
        if let Some(name) = &mut self.new_branch {
            let mut close = false;
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text("new-branch-name")
                        .desired_width(180.0),
                );
                edit.request_focus();
                let valid = valid_branch_name(name);
                let create = ui.add_enabled(valid && !busy, egui::Button::new("Create"));
                if create.clicked()
                    || (valid && edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                {
                    *action = Some(Action::Op(GitOp::CreateBranch(name.trim().to_owned())));
                    close = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    close = true;
                }
            });
            if close {
                self.new_branch = None;
            }
        }
    }

    fn commit_box(
        &mut self,
        ui: &mut egui::Ui,
        busy: bool,
        staged: &[&FileChange],
        unstaged: &[&FileChange],
        action: &mut Option<Action>,
    ) {
        let edit = ui.add(
            egui::TextEdit::multiline(&mut self.message)
                .hint_text("Commit message (Ctrl+Enter to commit)")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        let has_message = !self.message.trim().is_empty();
        // With nothing staged, commit every visible change (as most git tools do).
        let stage_all = staged.is_empty();
        let something = !staged.is_empty() || !unstaged.is_empty() || self.amend;
        let can_commit = !busy && something && (has_message || self.amend);
        let mut commit = false;
        ui.horizontal(|ui| {
            let label = match (self.amend, stage_all) {
                (true, _) => "Amend Last Commit",
                (false, true) => "Commit All",
                (false, false) => "Commit",
            };
            let button =
                egui::Button::new(RichText::new(format!("{}  {label}", icon::CHECK)).strong())
                    .fill(theme::ACCENT_DIM);
            let hint = if stage_all && !self.amend {
                "Nothing is staged: stages and commits every change listed"
            } else {
                "Commit the staged changes"
            };
            commit |= ui
                .add_enabled(can_commit, button)
                .on_hover_text(hint)
                .clicked();
            ui.checkbox(&mut self.amend, "Amend")
                .on_hover_text("Replace the last commit (message optional)");
        });
        let shortcut =
            edit.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
        if (commit || shortcut) && can_commit {
            let stage = if stage_all && !self.amend {
                paths(unstaged)
            } else {
                Vec::new()
            };
            *action = Some(Action::Op(GitOp::Commit {
                message: self.message.clone(),
                amend: self.amend,
                stage,
            }));
        }
    }

    fn confirm(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        let Some((tracked, untracked)) = &self.confirm_discard else {
            return;
        };
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("git.discard")).show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.heading(format!("{}  Discard Changes", icon::WARNING));
            let total = tracked.len() + untracked.len();
            ui.label(format!(
                "Throw away the changes to {total} file{}? This cannot be undone.",
                if total == 1 { "" } else { "s" }
            ));
            if !untracked.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "{} new file{} will be deleted.",
                        untracked.len(),
                        if untracked.len() == 1 { "" } else { "s" }
                    ))
                    .color(theme::WARN),
                );
            }
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(RichText::new("Discard").color(theme::ERROR))
                    .clicked()
                {
                    *action = Some(Action::Op(GitOp::Discard {
                        tracked: tracked.clone(),
                        untracked: untracked.clone(),
                    }));
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.confirm_discard = None;
        }
    }
}

/// The empty state: offers `git init` in the project folder.
pub fn no_repository(ui: &mut egui::Ui, busy: bool, root: &Path) -> Option<GitOp> {
    super::empty_hint(
        ui,
        icon::GIT_BRANCH,
        "This project is not under version control",
    );
    let mut op = None;
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(format!("Create a git repository in {}", root.display()))
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add_space(4.0);
        let init = egui::Button::new(
            RichText::new(format!("{}  Initialize Repository", icon::GIT_BRANCH)).strong(),
        )
        .fill(theme::ACCENT_DIM);
        if ui.add_enabled(!busy, init).clicked() {
            op = Some(GitOp::Init(root.to_path_buf()));
        }
    });
    op
}

fn file_row(
    ui: &mut egui::Ui,
    git: &GitClient,
    change: &FileChange,
    staged: bool,
    action: &mut Option<Action>,
) {
    let state = if staged {
        change.staged
    } else {
        change.unstaged
    };
    let Some(state) = state else {
        return;
    };
    let untracked = state == FileState::Untracked;
    let shown = git
        .diff
        .as_ref()
        .is_some_and(|d| d.path == change.path && d.staged == staged);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(state.letter())
                .monospace()
                .strong()
                .color(state_color(state)),
        )
        .on_hover_text(state.describe());
        let (folder, name) = change
            .path
            .rsplit_once('/')
            .map_or(("", change.path.as_str()), |(d, n)| (d, n));
        let mut text = RichText::new(name);
        if state == FileState::Deleted {
            text = text.strikethrough();
        }
        let row = ui.selectable_label(shown, text);
        let mut tip = change.path.clone();
        if let Some(original) = &change.original {
            tip = format!("{original} → {tip}");
        }
        if row.on_hover_text(tip).clicked() {
            *action = Some(Action::Diff {
                path: change.path.clone(),
                staged,
                untracked,
            });
        }
        if !folder.is_empty() {
            ui.label(RichText::new(folder).small().color(theme::TEXT_DIM));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if staged {
                if ui
                    .small_button(icon::MINUS)
                    .on_hover_text("Unstage")
                    .clicked()
                {
                    *action = Some(Action::Op(GitOp::Unstage(vec![change.path.clone()])));
                }
            } else {
                if state != FileState::Conflicted
                    && ui.small_button(icon::PLUS).on_hover_text("Stage").clicked()
                {
                    *action = Some(Action::Op(GitOp::Stage(vec![change.path.clone()])));
                }
                if ui
                    .small_button(icon::ARROW_COUNTER_CLOCKWISE)
                    .on_hover_text("Discard changes")
                    .clicked()
                {
                    let path = vec![change.path.clone()];
                    *action = Some(if untracked {
                        Action::Discard(Vec::new(), path)
                    } else {
                        Action::Discard(path, Vec::new())
                    });
                }
            }
        });
    });
}

fn diff_view(ui: &mut egui::Ui, text: &str) {
    if text.trim().is_empty() {
        ui.label(RichText::new("No textual changes").color(theme::TEXT_DIM));
        return;
    }
    egui::ScrollArea::both()
        .id_salt("git.diff.scroll")
        .max_height(320.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for line in text.lines() {
                let color = if line.starts_with("+++") || line.starts_with("---") {
                    theme::TEXT_DIM
                } else if line.starts_with('+') {
                    theme::PLAY
                } else if line.starts_with('-') {
                    theme::ERROR
                } else if line.starts_with("@@") {
                    theme::ACCENT
                } else if line.starts_with("diff ") || line.starts_with("index ") {
                    theme::TEXT_DIM
                } else {
                    ui.visuals().text_color()
                };
                ui.label(RichText::new(line).monospace().color(color));
            }
        });
}

fn paths(changes: &[&FileChange]) -> Vec<String> {
    changes.iter().map(|c| c.path.clone()).collect()
}

/// (tracked, untracked) paths of the work-tree changes.
pub fn split_untracked(changes: &[&FileChange]) -> (Vec<String>, Vec<String>) {
    let (untracked, tracked): (Vec<&&FileChange>, Vec<&&FileChange>) = changes
        .iter()
        .partition(|c| c.unstaged == Some(FileState::Untracked));
    (
        tracked.iter().map(|c| c.path.clone()).collect(),
        untracked.iter().map(|c| c.path.clone()).collect(),
    )
}

/// A plausible branch name (git has the final word).
pub fn valid_branch_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('/')
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("//")
        && !name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names() {
        assert!(valid_branch_name("feature/lights"));
        assert!(valid_branch_name("fix-123"));
        assert!(!valid_branch_name(""));
        assert!(!valid_branch_name("has space"));
        assert!(!valid_branch_name("-x"));
        assert!(!valid_branch_name("a..b"));
        assert!(!valid_branch_name("x.lock"));
        assert!(!valid_branch_name("what?"));
    }

    #[test]
    fn project_root_is_the_game_folder() {
        let dir = std::env::temp_dir().join(format!("ab-git-root-{}", std::process::id()));
        let assets = dir.join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        assert_eq!(project_root(&assets), assets, "a bare asset folder");
        std::fs::write(dir.join("game.toml"), "").unwrap();
        assert_eq!(project_root(&assets), dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discards_split_new_files() {
        let change = |path: &str, state| FileChange {
            path: path.into(),
            original: None,
            staged: None,
            unstaged: Some(state),
        };
        let a = change("a", FileState::Modified);
        let b = change("b", FileState::Untracked);
        let (tracked, untracked) = split_untracked(&[&a, &b]);
        assert_eq!((tracked, untracked), (vec!["a".into()], vec!["b".into()]));
    }
}
