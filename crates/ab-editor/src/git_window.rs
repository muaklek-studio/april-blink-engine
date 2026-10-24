//! The Git window (a tool window): the project's repository as a git client shows it.
//!
//! * Tool bar: the branch, Fetch, Pull (merge or rebase), Push, Sync (pull then push, with
//!   the counts to bring in and send), new branch, merge, stash and pop, and a fetch every
//!   few minutes so the counts stay current.
//! * Left: local branches (with what they have to push and pull), remote branches by
//!   remote, tags and stashes; double-click checks out, right-click for the rest.
//! * Middle: the commit graph of every branch, one colored lane per line of history, the
//!   branches and tags on their commits, and the uncommitted changes on top; a filter finds
//!   commits by message, author, hash or branch.
//! * Right: the selected commit (message, author, parents, the files it changed with their
//!   line counts), or for the uncommitted changes: conflicts to resolve, changes to stage,
//!   and the commit message.
//! * Bottom: the diff of the selected file, highlighted (see [`crate::diff_view`]).
//! * A merge, rebase, cherry-pick or revert stopped on conflicts shows as a banner with
//!   Continue and Abort.
//!
//! Git runs on its worker thread ([`GitClient`]): the window only shows its answers.

use crate::diff_view::{self, Layout};
use crate::git::{
    relative_time, CommitDetail, Edge, FileChange, FileState, GitClient, GitOp, GraphCommit,
    GraphRow, InProgress, RefKind, RepoState, RepoStatus,
};
use crate::panels::git::{split_untracked, state_color, valid_branch_name};
use crate::theme;
use crate::tool_window;
use eframe::egui::{self, Color32, Key, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Commits read at first; "Load more" adds as many again.
const PAGE: usize = 400;
const ROW: f32 = 26.0;
const LANE: f32 = 14.0;
/// Most lanes drawn; wider histories are clipped (the rest of the row still shows).
const MAX_LANES: usize = 14;
const REFRESH_EVERY: Duration = Duration::from_secs(3);
const FETCH_EVERY: Duration = Duration::from_secs(300);

/// Lane colors, distinct on the dark theme.
const LANE_COLORS: [Color32; 8] = [
    Color32::from_rgb(86, 156, 255),
    Color32::from_rgb(232, 120, 70),
    Color32::from_rgb(110, 200, 120),
    Color32::from_rgb(200, 110, 220),
    Color32::from_rgb(240, 200, 80),
    Color32::from_rgb(80, 200, 200),
    Color32::from_rgb(240, 100, 140),
    Color32::from_rgb(160, 160, 240),
];

fn lane_color(lane: usize) -> Color32 {
    LANE_COLORS[lane % LANE_COLORS.len()]
}

/// What the right-hand panel shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Selection {
    /// The uncommitted changes.
    #[default]
    WorkTree,
    Commit(String),
}

/// A question waiting for an answer.
#[derive(Debug, Clone)]
enum Dialog {
    NewBranch {
        name: String,
        /// The commit to start from (`None`: HEAD).
        at: Option<String>,
    },
    Tag {
        name: String,
        at: String,
    },
    Merge {
        branch: String,
    },
    Stash {
        message: String,
        untracked: bool,
    },
    Confirm {
        title: String,
        text: String,
        op: GitOp,
    },
}

enum Action {
    Op(GitOp),
    /// Several operations, in order (commit then push).
    Ops(Vec<GitOp>),
    Select(Selection),
    WorkTreeDiff {
        path: String,
        staged: bool,
        untracked: bool,
    },
    CommitDiff {
        hash: String,
        path: String,
    },
    Ask(Dialog),
    Refresh,
    LoadMore,
}

pub struct GitWindow {
    selection: Selection,
    message: String,
    amend: bool,
    /// A commit is running: the message is cleared once it is made, kept if it fails.
    committing: Option<u64>,
    push_after_commit: bool,
    filter: String,
    limit: usize,
    layout: Layout,
    rebase: bool,
    auto_fetch: bool,
    whole_repository: bool,
    dialog: Option<Dialog>,
    /// The diff on show, parsed: (what it is, its parse).
    parsed: Option<(u64, diff_view::Diff)>,
}

impl Default for GitWindow {
    fn default() -> Self {
        Self {
            selection: Selection::WorkTree,
            message: String::new(),
            amend: false,
            committing: None,
            push_after_commit: false,
            filter: String::new(),
            limit: PAGE,
            layout: Layout::Unified,
            rebase: false,
            auto_fetch: true,
            whole_repository: false,
            dialog: None,
            parsed: None,
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl GitWindow {
    /// The window's body. `git` is the editor's git worker; `project` the asset folder.
    pub fn show(&mut self, ui: &mut egui::Ui, git: Option<&mut GitClient>, project: &Path) {
        let Some(git) = git else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        git.refresh_every(REFRESH_EVERY);
        if let Some(made) = self.committing.and_then(|ticket| git.commit_made(ticket)) {
            self.committing = None;
            if made {
                self.message.clear();
                self.amend = false;
            }
        }
        let root = crate::panels::git::project_root(project);
        let status = match &git.state {
            RepoState::Unknown => {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            }
            RepoState::NoRepository => {
                ui.add_space(60.0);
                if let Some(op) = crate::panels::git::no_repository(ui, git.is_busy(), &root) {
                    git.run(op);
                }
                return;
            }
            RepoState::Failed(err) => {
                crate::panels::empty_hint(ui, icon::X_CIRCLE, err);
                ui.vertical_centered(|ui| {
                    if ui
                        .button(format!("{}  Retry", icon::ARROW_CLOCKWISE))
                        .clicked()
                    {
                        git.refresh();
                    }
                });
                return;
            }
            RepoState::Ready(status) => status.as_ref().clone(),
        };
        git.want_graph(self.limit);
        if self.auto_fetch {
            git.fetch_every(FETCH_EVERY);
        }

        let mut actions = Vec::new();
        tool_window::toolbar(ui, "git-toolbar", |ui| {
            self.toolbar(ui, git, &status, &mut actions);
        });
        if let Some(what) = status.in_progress {
            self.banner(ui, &status, what, &mut actions);
        }
        tool_window::side(
            ui,
            "git-refs",
            true,
            240.0,
            icon::GIT_BRANCH,
            "Branches",
            |ui| {
                refs_tree(ui, git, &status, &mut actions);
            },
        );
        egui::Panel::right(egui::Id::new("git-detail"))
            .resizable(true)
            .default_size(400.0)
            .size_range(280.0..=900.0)
            .frame(panel_frame())
            .show(ui, |ui| match self.selection.clone() {
                Selection::WorkTree => self.work_tree(ui, git, &status, &root, &mut actions),
                Selection::Commit(hash) => commit_detail(ui, git, &hash, &mut actions),
            });
        egui::Panel::bottom(egui::Id::new("git-diff"))
            .resizable(true)
            .default_size(320.0)
            .size_range(120.0..=1200.0)
            .frame(panel_frame())
            .show(ui, |ui| self.diff(ui, git));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG_0))
            .show(ui, |ui| self.graph(ui, git, &status, &mut actions));
        self.dialogs(ui, &status, &mut actions);

        for action in actions {
            match action {
                Action::Op(op) => self.run(git, op),
                Action::Ops(ops) => {
                    // Each step only if the one before worked (no push of a failed commit).
                    for (i, op) in ops.into_iter().enumerate() {
                        if i == 0 {
                            self.run(git, op);
                        } else {
                            git.run_after(op);
                        }
                    }
                }
                Action::Select(selection) => {
                    if let Selection::Commit(hash) = &selection {
                        git.show_commit(hash);
                    }
                    if self.selection != selection {
                        git.diff = None;
                    }
                    self.selection = selection;
                }
                Action::WorkTreeDiff {
                    path,
                    staged,
                    untracked,
                } => git.show_diff(&path, staged, untracked),
                Action::CommitDiff { hash, path } => git.show_commit_diff(&hash, &path),
                Action::Ask(dialog) => self.dialog = Some(dialog),
                Action::Refresh => git.refresh(),
                Action::LoadMore => self.limit += PAGE,
            }
        }
    }

    fn run(&mut self, git: &mut GitClient, op: GitOp) {
        if matches!(op, GitOp::Commit { .. }) {
            self.committing = Some(git.commit(op));
        } else {
            git.run(op);
        }
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        git: &GitClient,
        status: &RepoStatus,
        actions: &mut Vec<Action>,
    ) {
        let busy = git.is_busy();
        let branch = &status.branch;
        ui.label(RichText::new(icon::GIT_BRANCH).color(theme::ACCENT));
        egui::ComboBox::from_id_salt("git-window-branch")
            .selected_text(RichText::new(branch.label()).strong())
            .width(170.0)
            .show_ui(ui, |ui| {
                for local in &status.local {
                    let here = branch.head.as_deref() == Some(local.name.as_str());
                    if ui.selectable_label(here, &local.name).clicked() && !here {
                        actions.push(Action::Op(GitOp::Switch(local.name.clone())));
                    }
                }
            });
        ui.separator();
        let remote = !status.remotes.is_empty();
        let no_remote = "No remote configured (git remote add origin <url>)";
        let tip = |text: &str| {
            if remote {
                text.to_owned()
            } else {
                no_remote.to_owned()
            }
        };
        let enabled = remote && !busy;
        if ui
            .add_enabled(
                enabled,
                egui::Button::new(format!("{}  Fetch", icon::ARROWS_CLOCKWISE)),
            )
            .on_hover_text(tip(
                "Download what the remote has, without changing your branch",
            ))
            .clicked()
        {
            actions.push(Action::Op(GitOp::Fetch));
        }
        let pull_text = if branch.behind > 0 {
            format!("{}  Pull  {}", icon::ARROW_DOWN, branch.behind)
        } else {
            format!("{}  Pull", icon::ARROW_DOWN)
        };
        if ui
            .add_enabled(enabled, egui::Button::new(pull_text))
            .on_hover_text(tip(if self.rebase {
                "Bring in the remote's commits, your own replayed on top (rebase)"
            } else {
                "Bring in the remote's commits (merged with yours when both changed)"
            }))
            .clicked()
        {
            actions.push(Action::Op(GitOp::Pull {
                rebase: self.rebase,
            }));
        }
        ui.menu_button(icon::CARET_DOWN, |ui| {
            ui.radio_value(&mut self.rebase, false, "Pull: merge");
            ui.radio_value(&mut self.rebase, true, "Pull: rebase");
        })
        .response
        .on_hover_text("How Pull and Sync bring in the remote's commits");
        let push_text = if branch.ahead > 0 {
            format!("{}  Push  {}", icon::ARROW_UP, branch.ahead)
        } else {
            format!("{}  Push", icon::ARROW_UP)
        };
        if ui
            .add_enabled(enabled, egui::Button::new(push_text))
            .on_hover_text(tip("Send your commits to the remote"))
            .clicked()
        {
            actions.push(Action::Op(GitOp::Push));
        }
        let sync = egui::Button::new(
            RichText::new(format!(
                "{}  Sync  ↓{} ↑{}",
                icon::ARROWS_DOWN_UP,
                branch.behind,
                branch.ahead
            ))
            .strong(),
        )
        .fill(theme::ACCENT_DIM);
        if ui
            .add_enabled(enabled, sync)
            .on_hover_text(tip("Pull, then push: your branch and the remote's in step"))
            .clicked()
        {
            actions.push(Action::Op(GitOp::Sync {
                rebase: self.rebase,
            }));
        }
        ui.separator();
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(format!("{}  Branch", icon::GIT_FORK)),
            )
            .on_hover_text("New branch from the current commit")
            .clicked()
        {
            actions.push(Action::Ask(Dialog::NewBranch {
                name: String::new(),
                at: None,
            }));
        }
        if ui
            .add_enabled(
                !busy && status.in_progress.is_none(),
                egui::Button::new(format!("{}  Merge", icon::GIT_MERGE)),
            )
            .on_hover_text("Merge another branch into this one")
            .clicked()
        {
            let other = status
                .local
                .iter()
                .map(|b| b.name.clone())
                .find(|b| Some(b.as_str()) != branch.head.as_deref())
                .unwrap_or_default();
            actions.push(Action::Ask(Dialog::Merge { branch: other }));
        }
        let dirty = !status.changes.is_empty();
        if ui
            .add_enabled(
                !busy && dirty,
                egui::Button::new(format!("{}  Stash", icon::ARCHIVE)),
            )
            .on_hover_text("Put your uncommitted changes aside")
            .clicked()
        {
            actions.push(Action::Ask(Dialog::Stash {
                message: String::new(),
                untracked: true,
            }));
        }
        if ui
            .add_enabled(
                !busy && !status.stashes.is_empty(),
                egui::Button::new(format!("{}  Pop", icon::STACK)),
            )
            .on_hover_text("Bring back the last stashed changes")
            .clicked()
        {
            actions.push(Action::Op(GitOp::StashPop(0)));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(!busy, egui::Button::new(icon::ARROW_CLOCKWISE))
                .on_hover_text("Read the repository again")
                .clicked()
            {
                actions.push(Action::Refresh);
            }
            ui.toggle_value(&mut self.auto_fetch, icon::CLOCK)
                .on_hover_text("Fetch every 5 minutes, so the counts to pull stay current");
            if let Some(op) = git.remote_op() {
                ui.label(RichText::new(format!("{op}…")).color(theme::TEXT_DIM));
                ui.spinner();
            } else if busy {
                ui.spinner();
            }
            if let Some(upstream) = &branch.upstream {
                ui.label(
                    RichText::new(format!("{}  {upstream}", icon::CLOUD))
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
        });
    }

    fn banner(
        &mut self,
        ui: &mut egui::Ui,
        status: &RepoStatus,
        what: InProgress,
        actions: &mut Vec<Action>,
    ) {
        let conflicts = status.conflicts().count();
        egui::Panel::top(egui::Id::new("git-in-progress"))
            .frame(
                egui::Frame::NONE
                    .fill(theme::WARN.gamma_multiply(0.25))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .stroke(Stroke::new(1.0, theme::WARN.gamma_multiply(0.6))),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::WARNING).color(theme::WARN).size(16.0));
                    let text = if conflicts > 0 {
                        format!(
                            "{} in progress: {conflicts} file{} in conflict. Resolve them on the right, then Continue.",
                            what.describe(),
                            if conflicts == 1 { "" } else { "s" }
                        )
                    } else {
                        format!("{} in progress: conflicts resolved, ready to Continue.", what.describe())
                    };
                    ui.label(RichText::new(text).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(RichText::new(format!("{}  Abort", icon::X)).color(theme::ERROR))
                            .on_hover_text(format!("Give up the {} and go back to before it", what.describe().to_lowercase()))
                            .clicked()
                        {
                            actions.push(Action::Ask(Dialog::Confirm {
                                title: format!("Abort the {}?", what.describe().to_lowercase()),
                                text: "Your branch goes back to how it was before; resolutions made so far are lost.".into(),
                                op: GitOp::Abort(what),
                            }));
                        }
                        let ready = conflicts == 0;
                        if ui
                            .add_enabled(
                                ready,
                                egui::Button::new(RichText::new(format!("{}  Continue", icon::CHECK)).strong())
                                    .fill(theme::ACCENT_DIM),
                            )
                            .on_disabled_hover_text("Resolve every conflict first")
                            .clicked()
                        {
                            actions.push(Action::Op(GitOp::Continue(what)));
                            actions.push(Action::Select(Selection::WorkTree));
                        }
                    });
                });
            });
        self.selection = if conflicts > 0 {
            Selection::WorkTree
        } else {
            self.selection.clone()
        };
    }

    // =========================================================================================
    // Graph
    // =========================================================================================

    fn graph(
        &mut self,
        ui: &mut egui::Ui,
        git: &GitClient,
        status: &RepoStatus,
        actions: &mut Vec<Action>,
    ) {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(format!(
                        "{}  Filter by message, author, hash or branch",
                        icon::FUNNEL
                    ))
                    .desired_width(320.0),
            );
            if !self.filter.is_empty() && ui.small_button(icon::X).clicked() {
                self.filter.clear();
            }
            if let Some(err) = &git.graph_error {
                ui.label(RichText::new(format!("{}  {err}", icon::WARNING)).color(theme::ERROR));
            }
        });
        let Some(graph) = &git.graph else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        let needle = self.filter.trim().to_lowercase();
        let filtering = !needle.is_empty();
        let shown: Vec<usize> = (0..graph.commits.len())
            .filter(|&i| !filtering || matches_filter(&graph.commits[i], &needle))
            .collect();
        let work_tree = !status.changes.is_empty() && !filtering;
        let lanes = graph
            .rows
            .iter()
            .map(|r| r.width)
            .max()
            .unwrap_or(1)
            .min(MAX_LANES);
        let graph_width = if filtering {
            18.0
        } else {
            lanes as f32 * LANE + 12.0
        };
        let more = graph.commits.len() >= self.limit;
        let total = shown.len() + usize::from(work_tree) + usize::from(more);
        let head = status.branch.oid.clone();
        let now = now();

        egui::ScrollArea::vertical()
            .id_salt("git-graph")
            .auto_shrink([false, false])
            .show_rows(ui, ROW, total, |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for index in range {
                    let width = ui.available_width();
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(width, ROW), Sense::click());
                    if work_tree && index == 0 {
                        self.work_tree_row(ui, rect, &response, status, graph_width, actions);
                        continue;
                    }
                    let i = index - usize::from(work_tree);
                    if i >= shown.len() {
                        let button = ui.put(
                            rect.shrink2(Vec2::new(width * 0.35, 3.0)),
                            egui::Button::new(format!(
                                "{}  Load {PAGE} more commits",
                                icon::CARET_DOWN
                            )),
                        );
                        if button.clicked() {
                            actions.push(Action::LoadMore);
                        }
                        continue;
                    }
                    let commit = &graph.commits[shown[i]];
                    let selected = self.selection == Selection::Commit(commit.hash.clone());
                    if selected {
                        ui.painter()
                            .rect_filled(rect, 0.0, crate::panels::ROW_SELECTED);
                    } else if response.hovered() {
                        ui.painter().rect_filled(rect, 0.0, theme::BG_2);
                    }
                    let graph_rect = Rect::from_min_size(rect.min, Vec2::new(graph_width, ROW));
                    if filtering {
                        ui.painter().circle_filled(
                            graph_rect.center(),
                            4.0,
                            lane_color(graph.rows[shown[i]].lane),
                        );
                    } else {
                        paint_row(
                            &ui.painter_at(graph_rect),
                            graph_rect,
                            &graph.rows[shown[i]],
                            commit,
                            head.as_deref() == Some(commit.hash.as_str()),
                        );
                    }
                    let text_rect = Rect::from_min_max(
                        Pos2::new(graph_rect.right() + 4.0, rect.top()),
                        rect.max,
                    );
                    commit_columns(ui, text_rect, commit, now, head.as_deref());
                    if response.clicked() {
                        actions.push(Action::Select(Selection::Commit(commit.hash.clone())));
                    }
                    if response.double_clicked() {
                        if let Some(branch) = commit.refs.iter().find(|r| r.kind == RefKind::Local)
                        {
                            actions.push(Action::Op(GitOp::Switch(branch.name.clone())));
                        }
                    }
                    response.context_menu(|ui| commit_menu(ui, commit, status, actions));
                }
            });
    }

    fn work_tree_row(
        &self,
        ui: &egui::Ui,
        rect: Rect,
        response: &egui::Response,
        status: &RepoStatus,
        graph_width: f32,
        actions: &mut Vec<Action>,
    ) {
        if self.selection == Selection::WorkTree {
            ui.painter()
                .rect_filled(rect, 0.0, crate::panels::ROW_SELECTED);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 0.0, theme::BG_2);
        }
        let dot = Pos2::new(rect.left() + LANE / 2.0 + 6.0, rect.center().y);
        let painter = ui.painter();
        // A dashed stub down to HEAD's commit.
        painter.add(egui::Shape::dashed_line(
            &[dot, Pos2::new(dot.x, rect.bottom())],
            Stroke::new(1.5, theme::TEXT_DIM),
            3.0,
            3.0,
        ));
        painter.circle(dot, 4.5, theme::BG_0, Stroke::new(1.5, theme::TEXT_DIM));
        let conflicts = status.conflicts().count();
        let (text, color) = if conflicts > 0 {
            (
                format!("{}  {conflicts} conflict(s) to resolve", icon::WARNING),
                theme::ERROR,
            )
        } else {
            (
                format!(
                    "{}  Uncommitted changes ({})",
                    icon::PENCIL_SIMPLE_LINE,
                    status.changes.len()
                ),
                theme::WARN,
            )
        };
        painter.text(
            Pos2::new(rect.left() + graph_width + 4.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(13.0),
            color,
        );
        if response.clicked() {
            actions.push(Action::Select(Selection::WorkTree));
        }
    }

    // =========================================================================================
    // Work tree: conflicts, staging and the commit
    // =========================================================================================

    fn work_tree(
        &mut self,
        ui: &mut egui::Ui,
        git: &GitClient,
        status: &RepoStatus,
        root: &Path,
        actions: &mut Vec<Action>,
    ) {
        let busy = git.is_busy();
        let scope = root
            .strip_prefix(&status.root)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|p| !p.is_empty());
        let whole = self.whole_repository;
        let visible = |c: &&FileChange| {
            whole
                || scope
                    .as_deref()
                    .is_none_or(|s| c.path.starts_with(s) && c.path[s.len()..].starts_with('/'))
        };
        let conflicts: Vec<&FileChange> = status.conflicts().collect();
        let staged: Vec<&FileChange> = status.staged().filter(visible).collect();
        let unstaged: Vec<&FileChange> = status
            .unstaged()
            .filter(|c| c.unstaged != Some(FileState::Conflicted))
            .filter(visible)
            .collect();

        tool_window::heading(ui, icon::PENCIL_SIMPLE_LINE, "Uncommitted changes");
        if let Some(scope) = &scope {
            ui.checkbox(&mut self.whole_repository, "Whole repository")
                .on_hover_text(format!(
                    "Show changes outside the project folder ({scope}) too"
                ));
        }
        self.commit_box(ui, busy, status, &staged, &unstaged, actions);
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("git-work-tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if !conflicts.is_empty() {
                    list_header(
                        ui,
                        icon::WARNING,
                        "Conflicts",
                        conflicts.len(),
                        theme::ERROR,
                    );
                    ui.label(
                        RichText::new("Keep one side, or edit the file and mark it resolved.")
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                    for change in &conflicts {
                        conflict_row(ui, git, change, actions);
                    }
                    ui.add_space(6.0);
                }
                list_header(ui, icon::GIT_COMMIT, "Staged", staged.len(), theme::PLAY);
                if !staged.is_empty()
                    && ui
                        .small_button(format!("{}  Unstage all", icon::MINUS))
                        .clicked()
                {
                    actions.push(Action::Op(GitOp::Unstage(paths(&staged))));
                }
                for change in &staged {
                    change_row(ui, git, change, true, actions);
                }
                ui.add_space(6.0);
                list_header(ui, icon::GIT_DIFF, "Changes", unstaged.len(), theme::WARN);
                if !unstaged.is_empty() {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(format!("{}  Stage all", icon::PLUS))
                            .clicked()
                        {
                            actions.push(Action::Op(GitOp::Stage(paths(&unstaged))));
                        }
                        if ui
                            .small_button(format!("{}  Discard all", icon::ARROW_COUNTER_CLOCKWISE))
                            .clicked()
                        {
                            let (tracked, untracked) = split_untracked(&unstaged);
                            actions.push(discard(tracked, untracked));
                        }
                    });
                }
                for change in &unstaged {
                    change_row(ui, git, change, false, actions);
                }
                if staged.is_empty() && unstaged.is_empty() && conflicts.is_empty() {
                    crate::panels::empty_hint(ui, icon::CHECK, "Nothing to commit");
                }
            });
    }

    fn commit_box(
        &mut self,
        ui: &mut egui::Ui,
        busy: bool,
        status: &RepoStatus,
        staged: &[&FileChange],
        unstaged: &[&FileChange],
        actions: &mut Vec<Action>,
    ) {
        let merging = status.in_progress == Some(InProgress::Merge);
        let hint = if merging {
            "Merge message (empty: git's \"Merge branch …\")"
        } else {
            "Commit message (Ctrl+Enter to commit)"
        };
        let edit = ui.add(
            egui::TextEdit::multiline(&mut self.message)
                .hint_text(hint)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        let conflicts = status.conflicts().count() > 0;
        let stage_all = staged.is_empty() && !merging;
        let something = !staged.is_empty() || !unstaged.is_empty() || self.amend || merging;
        let has_message = !self.message.trim().is_empty() || merging || self.amend;
        let can_commit = !busy && something && has_message && !conflicts;
        let mut commit = false;
        ui.horizontal(|ui| {
            let label = match (self.amend, stage_all, merging) {
                (_, _, true) => "Commit Merge",
                (true, ..) => "Amend",
                (false, true, _) => "Commit All",
                (false, false, _) => "Commit",
            };
            let button =
                egui::Button::new(RichText::new(format!("{}  {label}", icon::CHECK)).strong())
                    .fill(theme::ACCENT_DIM);
            commit |= ui
                .add_enabled(can_commit, button)
                .on_hover_text(if stage_all {
                    "Nothing is staged: stages and commits every change listed"
                } else {
                    "Commit the staged changes"
                })
                .on_disabled_hover_text(if conflicts {
                    "Resolve the conflicts first"
                } else {
                    "Write a message and have something to commit"
                })
                .clicked();
            ui.checkbox(&mut self.amend, "Amend")
                .on_hover_text("Replace the last commit (message optional)");
            ui.checkbox(&mut self.push_after_commit, "Push")
                .on_hover_text("Push right after committing");
        });
        let shortcut =
            edit.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
        if (commit || shortcut) && can_commit {
            let stage = if stage_all && !self.amend {
                paths(unstaged)
            } else {
                Vec::new()
            };
            let mut ops = vec![GitOp::Commit {
                message: self.message.clone(),
                amend: self.amend,
                stage,
            }];
            if self.push_after_commit && !status.remotes.is_empty() {
                ops.push(GitOp::Push);
            }
            actions.push(Action::Ops(ops));
        }
    }

    // =========================================================================================
    // Diff
    // =========================================================================================

    fn diff(&mut self, ui: &mut egui::Ui, git: &GitClient) {
        let Some(diff) = &git.diff else {
            crate::panels::empty_hint(ui, icon::GIT_DIFF, "Select a file to see its changes");
            return;
        };
        let key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (&diff.path, diff.staged, &diff.commit, &diff.text).hash(&mut hasher);
            hasher.finish()
        };
        if self.parsed.as_ref().is_none_or(|(k, _)| *k != key) {
            let parsed = diff_view::parse(diff.text.as_deref().unwrap_or_default());
            self.parsed = Some((key, parsed));
        }
        let Some((_, parsed)) = &self.parsed else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::FILE_TEXT).color(theme::ACCENT));
            ui.label(RichText::new(&diff.path).strong());
            let source = match &diff.commit {
                Some(hash) => format!("in {}", &hash[..hash.len().min(8)]),
                None if diff.staged => "staged".into(),
                None => "not staged".into(),
            };
            ui.label(RichText::new(source).small().color(theme::TEXT_DIM));
            diff_view::counts(ui, parsed.added, parsed.removed);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.selectable_value(
                    &mut self.layout,
                    Layout::SideBySide,
                    format!("{}  Split", icon::COLUMNS),
                )
                .on_hover_text("Old and new side by side");
                ui.selectable_value(
                    &mut self.layout,
                    Layout::Unified,
                    format!("{}  Unified", icon::ROWS),
                )
                .on_hover_text("Changes in one column");
            });
        });
        ui.separator();
        match &diff.text {
            Ok(_) => diff_view::show(ui, ("git-window", &diff.path), parsed, self.layout),
            Err(err) => {
                ui.label(RichText::new(err).color(theme::ERROR));
            }
        }
    }

    // =========================================================================================
    // Dialogs
    // =========================================================================================

    fn dialogs(&mut self, ui: &mut egui::Ui, status: &RepoStatus, actions: &mut Vec<Action>) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("git-window-dialog")).show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            let mut buttons = |ui: &mut egui::Ui, label: &str, enabled: bool| -> bool {
                let mut ok = false;
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ok = ui
                        .add_enabled(
                            enabled,
                            egui::Button::new(RichText::new(label).strong())
                                .fill(theme::ACCENT_DIM),
                        )
                        .clicked()
                        || (enabled && ui.input(|i| i.key_pressed(Key::Enter)));
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                ok
            };
            match dialog {
                Dialog::NewBranch { name, at } => {
                    ui.heading(format!("{}  New Branch", icon::GIT_FORK));
                    ui.label(
                        RichText::new(match at {
                            Some(at) => format!("From commit {}", &at[..at.len().min(8)]),
                            None => format!("From {}", status.branch.label()),
                        })
                        .color(theme::TEXT_DIM),
                    );
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("feature/new-level")
                            .desired_width(f32::INFINITY),
                    )
                    .request_focus();
                    let valid = valid_branch_name(name);
                    if buttons(ui, "Create and switch", valid) {
                        actions.push(Action::Op(match at {
                            Some(at) => GitOp::CreateBranchAt {
                                name: name.trim().to_owned(),
                                at: at.clone(),
                            },
                            None => GitOp::CreateBranch(name.trim().to_owned()),
                        }));
                        close = true;
                    }
                }
                Dialog::Tag { name, at } => {
                    ui.heading(format!("{}  New Tag", icon::TAG));
                    ui.label(
                        RichText::new(format!("On commit {}", &at[..at.len().min(8)]))
                            .color(theme::TEXT_DIM),
                    );
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("v1.0")
                            .desired_width(f32::INFINITY),
                    )
                    .request_focus();
                    let valid = valid_branch_name(name);
                    if buttons(ui, "Create tag", valid) {
                        actions.push(Action::Op(GitOp::CreateTag {
                            name: name.trim().to_owned(),
                            at: at.clone(),
                        }));
                        close = true;
                    }
                }
                Dialog::Merge { branch } => {
                    ui.heading(format!("{}  Merge", icon::GIT_MERGE));
                    ui.label(format!("Merge into {}:", status.branch.label()));
                    egui::ComboBox::from_id_salt("git-merge-branch")
                        .selected_text(branch.as_str())
                        .width(380.0)
                        .show_ui(ui, |ui| {
                            for local in &status.local {
                                if Some(local.name.as_str()) != status.branch.head.as_deref() {
                                    ui.selectable_value(branch, local.name.clone(), &local.name);
                                }
                            }
                            for remote in &status.remote_branches {
                                ui.selectable_value(
                                    branch,
                                    remote.clone(),
                                    format!("{}  {remote}", icon::CLOUD),
                                );
                            }
                        });
                    ui.label(
                        RichText::new("Conflicts, if any, stop the merge for you to resolve here.")
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                    if buttons(ui, "Merge", !branch.is_empty()) {
                        actions.push(Action::Op(GitOp::Merge(branch.clone())));
                        close = true;
                    }
                }
                Dialog::Stash { message, untracked } => {
                    ui.heading(format!("{}  Stash Changes", icon::ARCHIVE));
                    ui.add(
                        egui::TextEdit::singleline(message)
                            .hint_text("What these changes are (optional)")
                            .desired_width(f32::INFINITY),
                    )
                    .request_focus();
                    ui.checkbox(untracked, "Include new files");
                    if buttons(ui, "Stash", true) {
                        actions.push(Action::Op(GitOp::StashPush {
                            message: message.clone(),
                            untracked: *untracked,
                        }));
                        close = true;
                    }
                }
                Dialog::Confirm { title, text, op } => {
                    ui.heading(format!("{}  {title}", icon::WARNING));
                    ui.label(text.as_str());
                    if buttons(ui, "Yes, go ahead", true) {
                        actions.push(Action::Op(op.clone()));
                        close = true;
                    }
                }
            }
        });
        if close || modal.should_close() {
            self.dialog = None;
        }
    }
}

fn panel_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::BG_1)
        .inner_margin(egui::Margin::same(6))
        .stroke(Stroke::new(1.0, theme::BORDER))
}

fn paths(changes: &[&FileChange]) -> Vec<String> {
    changes.iter().map(|c| c.path.clone()).collect()
}

fn discard(tracked: Vec<String>, untracked: Vec<String>) -> Action {
    let total = tracked.len() + untracked.len();
    let mut text = format!(
        "Throw away the changes to {total} file{}? This cannot be undone.",
        if total == 1 { "" } else { "s" }
    );
    if !untracked.is_empty() {
        text.push_str(&format!(
            " {} new file(s) will be deleted.",
            untracked.len()
        ));
    }
    Action::Ask(Dialog::Confirm {
        title: "Discard changes?".into(),
        text,
        op: GitOp::Discard { tracked, untracked },
    })
}

fn matches_filter(commit: &GraphCommit, needle: &str) -> bool {
    commit.subject.to_lowercase().contains(needle)
        || commit.author.to_lowercase().contains(needle)
        || commit.hash.starts_with(needle)
        || commit
            .refs
            .iter()
            .any(|r| r.name.to_lowercase().contains(needle))
}

/// One row's lines and dot.
fn paint_row(
    painter: &egui::Painter,
    rect: Rect,
    row: &GraphRow,
    commit: &GraphCommit,
    head: bool,
) {
    let x = |lane: usize| rect.left() + 6.0 + lane as f32 * LANE + LANE / 2.0;
    let y = |edge: Edge| match edge {
        Edge::Top => rect.top(),
        Edge::Middle => rect.center().y,
        Edge::Bottom => rect.bottom(),
    };
    for segment in &row.segments {
        if segment.from.0 >= MAX_LANES || segment.to.0 >= MAX_LANES {
            continue;
        }
        let stroke = Stroke::new(2.0, lane_color(segment.color));
        let a = Pos2::new(x(segment.from.0), y(segment.from.1));
        let b = Pos2::new(x(segment.to.0), y(segment.to.1));
        if a.x == b.x {
            painter.line_segment([a, b], stroke);
        } else {
            // A smooth bend from one lane to the other.
            let middle = (a.y + b.y) / 2.0;
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [a, Pos2::new(a.x, middle), Pos2::new(b.x, middle), b],
                false,
                Color32::TRANSPARENT,
                stroke,
            ));
        }
    }
    if row.lane >= MAX_LANES {
        return;
    }
    let center = Pos2::new(x(row.lane), rect.center().y);
    let color = lane_color(row.lane);
    if commit.parents.len() > 1 {
        // A merge: a ring.
        painter.circle(center, 4.0, theme::BG_0, Stroke::new(2.0, color));
    } else {
        painter.circle_filled(center, 4.5, color);
    }
    if head {
        painter.circle_stroke(center, 7.5, Stroke::new(1.5, theme::TEXT));
    }
}

/// Branch and tag labels, the subject, author, date and hash of a commit row.
fn commit_columns(
    ui: &mut egui::Ui,
    rect: Rect,
    commit: &GraphCommit,
    now: i64,
    head: Option<&str>,
) {
    let hash_w = 70.0;
    let date_w = 110.0;
    let author_w = (rect.width() * 0.18).clamp(80.0, 160.0);
    let text_right = rect.right() - hash_w - date_w - author_w - 12.0;
    let painter = ui.painter_at(rect);
    let y = rect.center().y;
    let mut x = rect.left();
    for label in &commit.refs {
        let (glyph, fill, text) = match label.kind {
            RefKind::Head => (icon::CHECK, theme::ACCENT, theme::BG_0),
            RefKind::Local => (icon::GIT_BRANCH, theme::ACCENT_DIM, theme::TEXT),
            RefKind::Remote => (icon::CLOUD, Color32::from_rgb(70, 60, 110), theme::TEXT),
            RefKind::Tag => (icon::TAG, Color32::from_rgb(110, 90, 30), theme::TEXT),
        };
        let galley = painter.layout_no_wrap(
            format!("{glyph} {}", label.name),
            egui::FontId::proportional(11.5),
            text,
        );
        let pill = Rect::from_min_size(
            Pos2::new(x, y - 9.0),
            Vec2::new(galley.size().x + 10.0, 18.0),
        );
        if pill.right() > text_right - 60.0 {
            break;
        }
        painter.rect_filled(pill, 9.0, fill);
        painter.galley(Pos2::new(x + 5.0, y - galley.size().y / 2.0), galley, text);
        x = pill.right() + 4.0;
    }
    let strong = head == Some(commit.hash.as_str());
    let subject = painter.layout(
        commit.subject.clone(),
        egui::FontId::proportional(13.0),
        if strong { Color32::WHITE } else { theme::TEXT },
        f32::INFINITY,
    );
    let clip = Rect::from_min_max(
        Pos2::new(x, rect.top()),
        Pos2::new(text_right, rect.bottom()),
    );
    ui.painter_at(clip).galley(
        Pos2::new(x + 2.0, y - subject.size().y / 2.0),
        subject,
        theme::TEXT,
    );
    let column = |left: f32, width: f32, text: String, color: Color32, mono: bool| {
        let font = if mono {
            egui::FontId::monospace(11.5)
        } else {
            egui::FontId::proportional(12.0)
        };
        let clip = Rect::from_min_max(
            Pos2::new(left, rect.top()),
            Pos2::new(left + width - 6.0, rect.bottom()),
        );
        ui.painter_at(clip).text(
            Pos2::new(left, y),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            color,
        );
    };
    column(
        text_right + 8.0,
        author_w,
        commit.author.clone(),
        theme::TEXT_DIM,
        false,
    );
    column(
        text_right + 8.0 + author_w,
        date_w,
        relative_time(commit.time, now),
        theme::TEXT_DIM,
        false,
    );
    column(
        rect.right() - hash_w,
        hash_w,
        commit.short.clone(),
        theme::ACCENT,
        true,
    );
}

fn commit_menu(
    ui: &mut egui::Ui,
    commit: &GraphCommit,
    status: &RepoStatus,
    actions: &mut Vec<Action>,
) {
    let short = &commit.hash[..commit.hash.len().min(8)];
    ui.label(RichText::new(format!("{}  {short}", icon::GIT_COMMIT)).color(theme::TEXT_DIM));
    ui.separator();
    for branch in commit.refs.iter().filter(|r| r.kind == RefKind::Local) {
        if ui
            .button(format!("{}  Switch to {}", icon::GIT_BRANCH, branch.name))
            .clicked()
        {
            actions.push(Action::Op(GitOp::Switch(branch.name.clone())));
            ui.close();
        }
    }
    if ui
        .button(format!("{}  New branch here…", icon::GIT_FORK))
        .clicked()
    {
        actions.push(Action::Ask(Dialog::NewBranch {
            name: String::new(),
            at: Some(commit.hash.clone()),
        }));
        ui.close();
    }
    if ui
        .button(format!("{}  Tag this commit…", icon::TAG))
        .clicked()
    {
        actions.push(Action::Ask(Dialog::Tag {
            name: String::new(),
            at: commit.hash.clone(),
        }));
        ui.close();
    }
    let busy_elsewhere = status.in_progress.is_some();
    ui.separator();
    if ui
        .add_enabled(
            !busy_elsewhere,
            egui::Button::new(format!(
                "{}  Merge into {}",
                icon::GIT_MERGE,
                status.branch.label()
            )),
        )
        .clicked()
    {
        let target = commit
            .refs
            .iter()
            .find(|r| matches!(r.kind, RefKind::Local | RefKind::Remote))
            .map_or_else(|| commit.hash.clone(), |r| r.name.clone());
        actions.push(Action::Op(GitOp::Merge(target)));
        ui.close();
    }
    if ui
        .add_enabled(
            !busy_elsewhere,
            egui::Button::new(format!("{}  Cherry-pick", icon::GIT_PULL_REQUEST)),
        )
        .on_hover_text("Apply this commit's change on the current branch")
        .clicked()
    {
        actions.push(Action::Op(GitOp::CherryPick(commit.hash.clone())));
        ui.close();
    }
    if ui
        .add_enabled(
            !busy_elsewhere,
            egui::Button::new(format!("{}  Revert", icon::ARROW_U_DOWN_LEFT)),
        )
        .on_hover_text("A new commit undoing this one")
        .clicked()
    {
        actions.push(Action::Ask(Dialog::Confirm {
            title: format!("Revert {short}?"),
            text: format!("A new commit will undo “{}”.", commit.subject),
            op: GitOp::Revert(commit.hash.clone()),
        }));
        ui.close();
    }
    ui.separator();
    if ui
        .button(format!("{}  Check out this commit", icon::DESKTOP))
        .on_hover_text("Detached HEAD: look around, then switch back to a branch")
        .clicked()
    {
        actions.push(Action::Op(GitOp::CheckoutCommit(commit.hash.clone())));
        ui.close();
    }
    if ui
        .button(format!("{}  Copy hash", icon::COPY_SIMPLE))
        .clicked()
    {
        ui.ctx().copy_text(commit.hash.clone());
        ui.close();
    }
}

fn list_header(ui: &mut egui::Ui, glyph: &str, title: &str, count: usize, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyph).color(color));
        ui.label(RichText::new(format!("{title}  ({count})")).strong());
    });
}

/// The file's name, with its folder dimmed after it; selected when its diff is on show.
fn file_label(ui: &mut egui::Ui, path: &str, state: FileState, selected: bool) -> egui::Response {
    let (folder, name) = path.rsplit_once('/').map_or(("", path), |(d, n)| (d, n));
    ui.label(
        RichText::new(state.letter())
            .monospace()
            .strong()
            .color(state_color(state)),
    )
    .on_hover_text(state.describe());
    let mut text = RichText::new(name);
    if state == FileState::Deleted {
        text = text.strikethrough();
    }
    let response = ui.selectable_label(selected, text).on_hover_text(path);
    if !folder.is_empty() {
        ui.label(RichText::new(folder).small().color(theme::TEXT_DIM));
    }
    response
}

fn change_row(
    ui: &mut egui::Ui,
    git: &GitClient,
    change: &FileChange,
    staged: bool,
    actions: &mut Vec<Action>,
) {
    let Some(state) = (if staged {
        change.staged
    } else {
        change.unstaged
    }) else {
        return;
    };
    let untracked = state == FileState::Untracked;
    let selected = git
        .diff
        .as_ref()
        .is_some_and(|d| d.commit.is_none() && d.path == change.path && d.staged == staged);
    ui.horizontal(|ui| {
        if file_label(ui, &change.path, state, selected).clicked() {
            actions.push(Action::WorkTreeDiff {
                path: change.path.clone(),
                staged,
                untracked,
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if staged {
                if ui
                    .small_button(icon::MINUS)
                    .on_hover_text("Unstage")
                    .clicked()
                {
                    actions.push(Action::Op(GitOp::Unstage(vec![change.path.clone()])));
                }
            } else {
                if ui.small_button(icon::PLUS).on_hover_text("Stage").clicked() {
                    actions.push(Action::Op(GitOp::Stage(vec![change.path.clone()])));
                }
                if ui
                    .small_button(icon::ARROW_COUNTER_CLOCKWISE)
                    .on_hover_text("Discard changes")
                    .clicked()
                {
                    let path = vec![change.path.clone()];
                    actions.push(if untracked {
                        discard(Vec::new(), path)
                    } else {
                        discard(path, Vec::new())
                    });
                }
            }
        });
    });
}

fn conflict_row(
    ui: &mut egui::Ui,
    git: &GitClient,
    change: &FileChange,
    actions: &mut Vec<Action>,
) {
    let selected = git
        .diff
        .as_ref()
        .is_some_and(|d| d.commit.is_none() && d.path == change.path);
    ui.horizontal(|ui| {
        if file_label(ui, &change.path, FileState::Conflicted, selected).clicked() {
            actions.push(Action::WorkTreeDiff {
                path: change.path.clone(),
                staged: false,
                untracked: false,
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let path = vec![change.path.clone()];
            if ui
                .small_button(icon::CHECK)
                .on_hover_text("Mark resolved (the file as it is now)")
                .clicked()
            {
                actions.push(Action::Op(GitOp::MarkResolved(path.clone())));
            }
            if ui
                .small_button("Theirs")
                .on_hover_text("Keep the version being merged in")
                .clicked()
            {
                actions.push(Action::Op(GitOp::Resolve {
                    paths: path.clone(),
                    theirs: true,
                }));
            }
            if ui
                .small_button("Ours")
                .on_hover_text("Keep this branch's version")
                .clicked()
            {
                actions.push(Action::Op(GitOp::Resolve {
                    paths: path,
                    theirs: false,
                }));
            }
        });
    });
}

/// The selected commit in full.
fn commit_detail(ui: &mut egui::Ui, git: &GitClient, hash: &str, actions: &mut Vec<Action>) {
    let detail: &CommitDetail = match &git.detail {
        Some(Ok(detail)) if detail.hash == hash => detail,
        Some(Err(err)) => {
            ui.label(RichText::new(err).color(theme::ERROR));
            return;
        }
        _ => {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        }
    };
    let (subject, body) = detail
        .message
        .split_once('\n')
        .map_or((detail.message.as_str(), ""), |(s, b)| (s, b.trim()));
    ui.label(RichText::new(subject).strong().size(15.0));
    if !body.is_empty() {
        ui.add(egui::Label::new(RichText::new(body).color(theme::TEXT_DIM)).wrap());
    }
    ui.add_space(4.0);
    egui::Grid::new("git-commit-meta")
        .num_columns(2)
        .spacing([10.0, 3.0])
        .show(ui, |ui| {
            let dim = |ui: &mut egui::Ui, text: &str| {
                ui.label(RichText::new(text).color(theme::TEXT_DIM));
            };
            dim(ui, "Commit");
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&detail.hash[..detail.hash.len().min(12)])
                        .monospace()
                        .color(theme::ACCENT),
                );
                if ui
                    .small_button(icon::COPY_SIMPLE)
                    .on_hover_text("Copy the full hash")
                    .clicked()
                {
                    ui.ctx().copy_text(detail.hash.clone());
                }
            });
            ui.end_row();
            dim(ui, "Author");
            ui.label(format!("{} <{}>", detail.author, detail.email));
            ui.end_row();
            if detail.committer != detail.author {
                dim(ui, "Committer");
                ui.label(&detail.committer);
                ui.end_row();
            }
            dim(ui, "Date");
            ui.label(&detail.date);
            ui.end_row();
            dim(
                ui,
                if detail.parents.len() > 1 {
                    "Parents"
                } else {
                    "Parent"
                },
            );
            ui.horizontal(|ui| {
                for parent in &detail.parents {
                    if ui
                        .link(RichText::new(&parent[..parent.len().min(8)]).monospace())
                        .on_hover_text("Select this commit")
                        .clicked()
                    {
                        actions.push(Action::Select(Selection::Commit(parent.clone())));
                    }
                }
                if detail.parents.is_empty() {
                    ui.label(RichText::new("none (first commit)").color(theme::TEXT_DIM));
                }
            });
            ui.end_row();
        });
    ui.add_space(6.0);
    let (added, removed) = detail.files.iter().fold((0, 0), |(a, r), f| {
        (
            a + f.added.unwrap_or(0) as usize,
            r + f.removed.unwrap_or(0) as usize,
        )
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::FILES).color(theme::ACCENT));
        ui.label(RichText::new(format!("{} file(s) changed", detail.files.len())).strong());
        diff_view::counts(ui, added, removed);
    });
    if detail.parents.len() > 1 {
        ui.label(
            RichText::new("A merge: changes against its first parent.")
                .small()
                .color(theme::TEXT_DIM),
        );
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("git-commit-files")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for file in &detail.files {
                let selected = git
                    .diff
                    .as_ref()
                    .is_some_and(|d| d.commit.as_deref() == Some(hash) && d.path == file.path);
                ui.horizontal(|ui| {
                    let response = file_label(ui, &file.path, file.state, selected);
                    let response = match &file.original {
                        Some(from) => response.on_hover_text(format!("renamed from {from}")),
                        None => response,
                    };
                    if response.clicked() {
                        actions.push(Action::CommitDiff {
                            hash: hash.to_owned(),
                            path: file.path.clone(),
                        });
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| match (file.added, file.removed) {
                            (Some(a), Some(r)) => {
                                ui.label(
                                    RichText::new(format!("−{r}"))
                                        .monospace()
                                        .small()
                                        .color(theme::ERROR),
                                );
                                ui.label(
                                    RichText::new(format!("+{a}"))
                                        .monospace()
                                        .small()
                                        .color(theme::PLAY),
                                );
                            }
                            _ => {
                                ui.label(RichText::new("binary").small().color(theme::TEXT_DIM));
                            }
                        },
                    );
                });
            }
        });
}

/// Branches, remotes, tags and stashes.
fn refs_tree(ui: &mut egui::Ui, git: &GitClient, status: &RepoStatus, actions: &mut Vec<Action>) {
    // A ref's commit in the graph, to select it.
    let commit_of = |name: &str| -> Option<String> {
        git.graph
            .as_ref()?
            .commits
            .iter()
            .find(|c| c.refs.iter().any(|r| r.name == name))
            .map(|c| c.hash.clone())
    };
    let select = |actions: &mut Vec<Action>, name: &str| {
        if let Some(hash) = commit_of(name) {
            actions.push(Action::Select(Selection::Commit(hash)));
        }
    };
    egui::ScrollArea::vertical()
        .id_salt("git-refs")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new(format!("{}  Local", icon::DESKTOP)).strong())
                .default_open(true)
                .show(ui, |ui| {
                    for branch in &status.local {
                        let current = status.branch.head.as_deref() == Some(branch.name.as_str());
                        let response = ui
                            .horizontal(|ui| {
                                let glyph = if current { icon::CHECK } else { icon::GIT_BRANCH };
                                ui.label(RichText::new(glyph).color(if current { theme::ACCENT } else { theme::TEXT_DIM }));
                                let mut text = RichText::new(&branch.name);
                                if current {
                                    text = text.strong().color(Color32::WHITE);
                                }
                                let response = ui.selectable_label(false, text);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if branch.gone {
                                        ui.label(RichText::new(icon::PROHIBIT).color(theme::WARN))
                                            .on_hover_text("Its remote branch was deleted");
                                    }
                                    if branch.behind > 0 {
                                        ui.label(RichText::new(format!("↓{}", branch.behind)).small().color(theme::WARN));
                                    }
                                    if branch.ahead > 0 {
                                        ui.label(RichText::new(format!("↑{}", branch.ahead)).small().color(theme::PLAY));
                                    }
                                });
                                response
                            })
                            .inner;
                        let response = response.on_hover_text(match &branch.upstream {
                            Some(upstream) => format!("Tracks {upstream}. Double-click to switch."),
                            None => "No upstream (published on first push). Double-click to switch.".into(),
                        });
                        if response.clicked() {
                            select(actions, &branch.name);
                        }
                        if response.double_clicked() && !current {
                            actions.push(Action::Op(GitOp::Switch(branch.name.clone())));
                        }
                        response.context_menu(|ui| {
                            if ui.add_enabled(!current, egui::Button::new(format!("{}  Switch to it", icon::GIT_BRANCH))).clicked() {
                                actions.push(Action::Op(GitOp::Switch(branch.name.clone())));
                                ui.close();
                            }
                            if ui
                                .add_enabled(!current && status.in_progress.is_none(), egui::Button::new(format!("{}  Merge into {}", icon::GIT_MERGE, status.branch.label())))
                                .clicked()
                            {
                                actions.push(Action::Op(GitOp::Merge(branch.name.clone())));
                                ui.close();
                            }
                            if ui.button(format!("{}  New branch from it…", icon::GIT_FORK)).clicked() {
                                actions.push(Action::Ask(Dialog::NewBranch { name: String::new(), at: Some(branch.name.clone()) }));
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .add_enabled(!current, egui::Button::new(RichText::new(format!("{}  Delete", icon::TRASH)).color(theme::ERROR)))
                                .clicked()
                            {
                                actions.push(Action::Ask(Dialog::Confirm {
                                    title: format!("Delete {}?", branch.name),
                                    text: "Only if it is merged; its commits stay reachable from the branch it was merged into.".into(),
                                    op: GitOp::DeleteBranch { name: branch.name.clone(), force: false },
                                }));
                                ui.close();
                            }
                            if ui
                                .add_enabled(!current, egui::Button::new(RichText::new(format!("{}  Delete, even unmerged", icon::TRASH)).color(theme::ERROR)))
                                .clicked()
                            {
                                actions.push(Action::Ask(Dialog::Confirm {
                                    title: format!("Delete {} for good?", branch.name),
                                    text: "Commits only this branch has will be lost.".into(),
                                    op: GitOp::DeleteBranch { name: branch.name.clone(), force: true },
                                }));
                                ui.close();
                            }
                        });
                    }
                });
            for remote in &status.remotes {
                let branches: Vec<&String> = status
                    .remote_branches
                    .iter()
                    .filter(|b| b.strip_prefix(remote.as_str()).is_some_and(|r| r.starts_with('/')))
                    .collect();
                egui::CollapsingHeader::new(RichText::new(format!("{}  {remote}", icon::CLOUD)).strong())
                    .id_salt(("git-remote", remote))
                    .default_open(true)
                    .show(ui, |ui| {
                        for branch in branches {
                            let short = &branch[remote.len() + 1..];
                            let local = status.local.iter().any(|b| b.name == short);
                            let response = ui
                                .selectable_label(false, format!("{}  {short}", icon::GIT_BRANCH))
                                .on_hover_text(if local {
                                    format!("{branch}. Double-click to switch to your {short}.")
                                } else {
                                    format!("{branch}. Double-click to check it out as a local branch.")
                                });
                            if response.clicked() {
                                select(actions, branch);
                            }
                            let checkout = if local {
                                GitOp::Switch(short.to_owned())
                            } else {
                                GitOp::CheckoutRemote(branch.clone())
                            };
                            if response.double_clicked() {
                                actions.push(Action::Op(checkout.clone()));
                            }
                            response.context_menu(|ui| {
                                if ui.button(format!("{}  Check out", icon::GIT_BRANCH)).clicked() {
                                    actions.push(Action::Op(checkout.clone()));
                                    ui.close();
                                }
                                if ui
                                    .add_enabled(status.in_progress.is_none(), egui::Button::new(format!("{}  Merge into {}", icon::GIT_MERGE, status.branch.label())))
                                    .clicked()
                                {
                                    actions.push(Action::Op(GitOp::Merge(branch.clone())));
                                    ui.close();
                                }
                            });
                        }
                    });
            }
            if !status.tags.is_empty() {
                egui::CollapsingHeader::new(RichText::new(format!("{}  Tags", icon::TAG)).strong())
                    .default_open(false)
                    .show(ui, |ui| {
                        for tag in &status.tags {
                            let response = ui.selectable_label(false, format!("{}  {tag}", icon::TAG));
                            if response.clicked() {
                                select(actions, tag);
                            }
                            response.context_menu(|ui| {
                                if ui.button(format!("{}  New branch from it…", icon::GIT_FORK)).clicked() {
                                    actions.push(Action::Ask(Dialog::NewBranch { name: String::new(), at: Some(tag.clone()) }));
                                    ui.close();
                                }
                                if ui.button(format!("{}  Check out", icon::DESKTOP)).clicked() {
                                    actions.push(Action::Op(GitOp::CheckoutCommit(tag.clone())));
                                    ui.close();
                                }
                            });
                        }
                    });
            }
            if !status.stashes.is_empty() {
                egui::CollapsingHeader::new(RichText::new(format!("{}  Stashes", icon::ARCHIVE)).strong())
                    .default_open(true)
                    .show(ui, |ui| {
                        for stash in &status.stashes {
                            let response = ui
                                .selectable_label(false, format!("{}  {}", icon::STACK, stash.message))
                                .on_hover_text(format!("stash@{{{}}}: right-click to apply, pop or drop", stash.index));
                            response.context_menu(|ui| {
                                if ui.button(format!("{}  Pop (apply and drop)", icon::STACK)).clicked() {
                                    actions.push(Action::Op(GitOp::StashPop(stash.index)));
                                    ui.close();
                                }
                                if ui.button(format!("{}  Apply (keep it)", icon::COPY)).clicked() {
                                    actions.push(Action::Op(GitOp::StashApply(stash.index)));
                                    ui.close();
                                }
                                if ui.button(RichText::new(format!("{}  Drop", icon::TRASH)).color(theme::ERROR)).clicked() {
                                    actions.push(Action::Ask(Dialog::Confirm {
                                        title: "Drop this stash?".into(),
                                        text: format!("“{}” will be deleted.", stash.message),
                                        op: GitOp::StashDrop(stash.index),
                                    }));
                                    ui.close();
                                }
                            });
                        }
                    });
            }
        });
}
