//! Keeps a project's packed assets in step with its files while the editor is open (see
//! [`ab_project::asset_store`]):
//!
//! * when the project opens (a fresh clone gets its images, models and sounds before the
//!   first scene loads),
//! * every few seconds in the background (edits and new files are packed),
//! * before a commit (the packs are written and staged with it) and after a pull or a
//!   branch switch (teammates' changes are unpacked).
//!
//! Files unpacked or removed are reloaded: the asset index rescans, and the engine forgets
//! what it had loaded from them.

use crate::git::{GitOp, Hook, Phase};
use crate::state::Editor;
use ab_engine::core::jobs::{self, Task};
use ab_project::asset_store::{self, SyncReport};
use ab_project::Project;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the background sync runs.
const EVERY: Duration = Duration::from_secs(15);

type Outcome = Result<SyncReport, String>;

pub struct AssetSync {
    project: Project,
    task: Option<Task<Outcome>>,
    last: Instant,
    /// Results of syncs run by git operations, on the git worker.
    from_git: Arc<Mutex<Vec<Outcome>>>,
}

fn run(project: &Project) -> Outcome {
    asset_store::sync(project).map_err(|e| format!("{e:#}"))
}

/// The first sync of a project being opened, run while the project manager waits for its
/// scripts to build (a fresh clone unpacks its assets there, not with the window frozen).
static PRESYNC: Mutex<Option<(std::path::PathBuf, Task<Outcome>)>> = Mutex::new(None);

/// Starts the first sync of `project` in the background (see [`AssetSync::start`]).
pub fn presync(project: &Project) {
    let copy = project.clone();
    let task = jobs::spawn_background("ab-asset-presync", move || run(&copy));
    *PRESYNC.lock().unwrap_or_else(|e| e.into_inner()) = Some((project.root.clone(), task));
}

/// Whether the first sync started for the project at `root` is done (true without one).
pub fn presync_finished(root: &std::path::Path) -> bool {
    match &mut *PRESYNC.lock().unwrap_or_else(|e| e.into_inner()) {
        Some((at, task)) if at == root => task.is_finished(),
        _ => true,
    }
}

/// The outcome of the first sync started for `root`, waiting for it if needed.
fn take_presync(root: &std::path::Path) -> Option<Outcome> {
    let mut slot = PRESYNC.lock().unwrap_or_else(|e| e.into_inner());
    match slot.take() {
        Some((at, task)) if at == root => {
            drop(slot);
            Some(task.wait())
        }
        other => {
            *slot = other;
            None
        }
    }
}

impl AssetSync {
    /// Brings `project` in step now (before its scenes load), and keeps it so.
    pub fn start(project: Project, editor: &mut Editor) -> Self {
        if let Err(err) = asset_store::ensure_git_files(&project) {
            tracing::warn!("git files not updated: {err:#}");
        }
        if project.lfs {
            if let Err(err) = asset_store::lfs_setup(&project.root) {
                tracing::warn!("Git LFS not set up: {err:#}");
            }
        }
        let started = Instant::now();
        // Started by the project manager already, most of the time.
        let outcome = take_presync(&project.root).unwrap_or_else(|| run(&project));
        tracing::info!(elapsed = ?started.elapsed(), "assets synced");
        let mut sync = Self {
            project,
            task: None,
            last: Instant::now(),
            from_git: Arc::default(),
        };
        sync.apply(outcome, editor);
        if sync.project.lfs && !asset_store::lfs_available() {
            let warning = format!(
                "Git LFS is not installed: packed assets cannot be committed. To fix: {}",
                asset_store::LFS_INSTALL
            );
            tracing::warn!("{warning}");
            editor.fail(warning);
        }
        sync
    }

    /// The git worker's hook: before a commit, pack the assets and stage the packs (and
    /// refuse when they should go to Git LFS and it is missing); after a pull or a switch,
    /// unpack; after `git init`, set the new repository up for Git LFS.
    pub fn git_hook(&self) -> Hook {
        let project = self.project.clone();
        let results = self.from_git.clone();
        Box::new(move |phase, op| {
            match (phase, op) {
                (Phase::Before, GitOp::Commit { .. }) => {}
                (Phase::After, op) if op.changes_work_tree() => {}
                (Phase::After, GitOp::Init(_)) => {
                    if let Err(err) = asset_store::lfs_setup(&project.root) {
                        tracing::warn!("Git LFS not set up: {err:#}");
                    }
                    return Ok(Vec::new());
                }
                _ => return Ok(Vec::new()),
            }
            let outcome = run(&project);
            let synced = outcome.as_ref().map(|_| ()).map_err(Clone::clone);
            results
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(outcome);
            if phase == Phase::After {
                return Ok(Vec::new());
            }
            synced.map_err(|err| format!("the packed assets are not in step: {err}"))?;
            if !project.pack_dir().is_dir() {
                return Ok(Vec::new());
            }
            asset_store::prepare_commit(&project).map_err(|err| format!("{err:#}"))?;
            // Committed with the scenes that use them; the git files with the packs.
            Ok([
                project.pack_dir(),
                project.root.join(".gitignore"),
                project.root.join(".gitattributes"),
            ]
            .into_iter()
            .filter(|path| path.exists())
            .collect())
        })
    }

    /// Starts the background sync when due, and applies finished ones.
    pub fn poll(&mut self, editor: &mut Editor) {
        let finished: Vec<Outcome> =
            std::mem::take(&mut *self.from_git.lock().unwrap_or_else(|e| e.into_inner()));
        for outcome in finished {
            self.apply(outcome, editor);
        }
        if let Some(task) = &mut self.task {
            if let Some(outcome) = task.try_take() {
                self.task = None;
                self.last = Instant::now();
                self.apply(outcome, editor);
            }
            return;
        }
        if self.last.elapsed() >= EVERY {
            let project = self.project.clone();
            self.task = Some(jobs::spawn_background("ab-asset-sync", move || {
                run(&project)
            }));
        }
    }

    fn apply(&mut self, outcome: Outcome, editor: &mut Editor) {
        let report = match outcome {
            Ok(report) => report,
            Err(err) => {
                tracing::error!("packed assets: {err}");
                editor.fail(format!("Packed assets not synced: {err}"));
                return;
            }
        };
        if !report.unpacked.is_empty() || !report.removed.is_empty() {
            if let Some(mut server) = editor
                .tree()
                .world()
                .resource_mut::<ab_engine::asset::AssetServer>()
            {
                for path in report.unpacked.iter().chain(&report.removed) {
                    server.evict(path);
                }
            }
            editor.assets.request_rescan();
        }
        if !report.conflicts.is_empty() {
            let conflicts = report.conflicts.join(", ");
            tracing::warn!(
                "packed assets changed here and in git: kept yours, theirs are in {}: {conflicts}",
                self.project.data_dir().join("conflicts").display()
            );
            editor.fail(format!(
                "Asset conflict (kept yours; theirs in .ablink/conflicts): {conflicts}"
            ));
        } else if report.changed() {
            tracing::info!("{}", report.summary());
            editor.info(report.summary());
        }
    }
}
