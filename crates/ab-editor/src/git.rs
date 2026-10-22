//! Git integration: status, staging, commits, branches, history and remotes, through the
//! `git` command line.
//!
//! Every git command runs on one background worker thread ([`GitClient`]), one at a time:
//! the UI never waits for git, and two commands never race for the repository's index lock.
//! The worker answers with [`RepoStatus`] snapshots after every change, and wakes the UI
//! through the `notify` callback it was started with.
//!
//! Commands never prompt: `GIT_TERMINAL_PROMPT=0`, `GIT_EDITOR=true` and SSH batch mode make
//! a missing credential or passphrase an error in the panel rather than a hung worker.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

/// How a file differs, on one side (index or work tree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl FileState {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            b'M' => Self::Modified,
            b'A' => Self::Added,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'T' => Self::TypeChanged,
            _ => return None,
        })
    }

    /// One-letter badge, as in `git status --short`.
    pub fn letter(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Copied => "C",
            Self::TypeChanged => "T",
            Self::Untracked => "U",
            Self::Conflicted => "!",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Modified => "Modified",
            Self::Added => "Added",
            Self::Deleted => "Deleted",
            Self::Renamed => "Renamed",
            Self::Copied => "Copied",
            Self::TypeChanged => "Type changed",
            Self::Untracked => "Untracked",
            Self::Conflicted => "Conflict",
        }
    }
}

/// One changed path. A file can have staged and unstaged changes at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the repository root, with `/` separators.
    pub path: String,
    /// Where a rename or copy came from.
    pub original: Option<String>,
    /// Change in the index (what the next commit records).
    pub staged: Option<FileState>,
    /// Change in the work tree not staged yet.
    pub unstaged: Option<FileState>,
}

impl FileChange {
    /// The state shown for the file as a whole: the work tree's if any, else the index's.
    pub fn state(&self) -> Option<FileState> {
        self.unstaged.or(self.staged)
    }
}

/// The checked-out branch and how it compares with its upstream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchInfo {
    /// Branch name; `None` when HEAD is detached.
    pub head: Option<String>,
    /// Commit hash of HEAD; `None` before the first commit.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    /// Commits not pushed yet.
    pub ahead: u32,
    /// Commits on the upstream not pulled yet.
    pub behind: u32,
}

impl BranchInfo {
    /// HEAD's name for display: the branch, or the short hash when detached.
    pub fn label(&self) -> String {
        match (&self.head, &self.oid) {
            (Some(head), _) => head.clone(),
            (None, Some(oid)) => format!("({})", &oid[..oid.len().min(8)]),
            (None, None) => "(no branch)".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub hash: String,
    pub author: String,
    /// Relative date ("3 hours ago").
    pub when: String,
    pub subject: String,
}

/// Everything the panel shows about a repository.
#[derive(Debug, Clone, Default)]
pub struct RepoStatus {
    /// Work tree root.
    pub root: PathBuf,
    pub branch: BranchInfo,
    pub changes: Vec<FileChange>,
    pub branches: Vec<String>,
    pub remotes: Vec<String>,
    /// Latest commits, newest first.
    pub log: Vec<CommitInfo>,
}

impl RepoStatus {
    /// Changes with something staged.
    pub fn staged(&self) -> impl Iterator<Item = &FileChange> {
        self.changes.iter().filter(|c| c.staged.is_some())
    }

    /// Changes with something not staged (including untracked files).
    pub fn unstaged(&self) -> impl Iterator<Item = &FileChange> {
        self.changes.iter().filter(|c| c.unstaged.is_some())
    }

    /// Per-path state for quick lookups (asset badges), keyed by repository-relative path.
    pub fn states(&self) -> HashMap<String, FileState> {
        self.changes
            .iter()
            .filter_map(|c| Some((c.path.clone(), c.state()?)))
            .collect()
    }
}

// =============================================================================================
// Parsing
// =============================================================================================

/// Parses `git status --porcelain=v2 --branch -z --untracked-files=all`.
pub fn parse_status(output: &str) -> (BranchInfo, Vec<FileChange>) {
    let mut branch = BranchInfo::default();
    let mut changes = Vec::new();
    let mut records = output.split('\0').filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => branch.oid = Some(value.to_owned()),
                "branch.head" if value != "(detached)" => branch.head = Some(value.to_owned()),
                "branch.upstream" => branch.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            branch.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            branch.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = record.as_bytes()[0];
        match kind {
            // 1 XY sub mH mI mW hH hI path
            b'1' => {
                let fields: Vec<&str> = record.splitn(9, ' ').collect();
                if let (Some(xy), Some(path)) = (fields.get(1), fields.get(8)) {
                    changes.push(change(xy, path, None));
                }
            }
            // 2 XY sub mH mI mW hH hI Xscore path, then the original path as its own record
            b'2' => {
                let fields: Vec<&str> = record.splitn(10, ' ').collect();
                let original = records.next().map(str::to_owned);
                if let (Some(xy), Some(path)) = (fields.get(1), fields.get(9)) {
                    changes.push(change(xy, path, original));
                }
            }
            // u XY sub m1 m2 m3 mW h1 h2 h3 path
            b'u' => {
                if let Some(path) = record.splitn(11, ' ').nth(10) {
                    changes.push(FileChange {
                        path: path.to_owned(),
                        original: None,
                        staged: None,
                        unstaged: Some(FileState::Conflicted),
                    });
                }
            }
            b'?' => {
                if let Some(path) = record.get(2..) {
                    changes.push(FileChange {
                        path: path.to_owned(),
                        original: None,
                        staged: None,
                        unstaged: Some(FileState::Untracked),
                    });
                }
            }
            _ => {}
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    (branch, changes)
}

fn change(xy: &str, path: &str, original: Option<String>) -> FileChange {
    let xy = xy.as_bytes();
    FileChange {
        path: path.to_owned(),
        original,
        staged: xy.first().copied().and_then(FileState::from_code),
        unstaged: xy.get(1).copied().and_then(FileState::from_code),
    }
}

const FIELD: char = '\u{1f}';
const RECORD: char = '\u{1e}';
const LOG_FORMAT: &str = "--pretty=format:%h%x1f%an%x1f%ar%x1f%s%x1e";

/// Parses `git log` output in [`LOG_FORMAT`].
pub fn parse_log(output: &str) -> Vec<CommitInfo> {
    output
        .split(RECORD)
        .map(|r| r.trim_matches(['\n', '\r']))
        .filter(|r| !r.is_empty())
        .filter_map(|record| {
            let mut fields = record.split(FIELD);
            Some(CommitInfo {
                hash: fields.next()?.to_owned(),
                author: fields.next()?.to_owned(),
                when: fields.next()?.to_owned(),
                subject: fields.next().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

// =============================================================================================
// Running git
// =============================================================================================

/// Output of a failed git command (its error message).
pub type GitError = String;

/// Runs `git -C dir args…` without any prompt and returns its standard output.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let mut command = ab_project::process::command("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        // Status and diffs must not take the index lock the user's own git might need.
        .env("GIT_OPTIONAL_LOCKS", "0")
        // Stable, parseable messages.
        .env("LC_ALL", "C");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let output = command.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            "git is not installed (or not on PATH)".to_owned()
        } else {
            format!("cannot run git: {err}")
        }
    })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        Err(if message.is_empty() {
            format!(
                "git {} failed ({})",
                args.first().unwrap_or(&""),
                output.status
            )
        } else {
            message.to_owned()
        })
    }
}

/// The work tree containing `dir`, or `None` outside any repository.
pub fn discover(dir: &Path) -> Result<Option<PathBuf>, GitError> {
    match git(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(root) => Ok(Some(PathBuf::from(root.trim()))),
        Err(err) if err.contains("not a git repository") => Ok(None),
        Err(err) => Err(err),
    }
}

/// Reads the full status of the repository at `root`.
pub fn read_status(root: &Path) -> Result<RepoStatus, GitError> {
    let status = git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )?;
    let (branch, changes) = parse_status(&status);
    let branches = git(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    )?
    .lines()
    .map(str::to_owned)
    .collect();
    let remotes = git(root, &["remote"])?.lines().map(str::to_owned).collect();
    // No log before the first commit.
    let log = if branch.oid.is_some() {
        parse_log(&git(root, &["log", "-n", "50", "--no-color", LOG_FORMAT])?)
    } else {
        Vec::new()
    };
    Ok(RepoStatus {
        root: root.to_path_buf(),
        branch,
        changes,
        branches,
        remotes,
        log,
    })
}

/// Longest diff shown, in lines.
const MAX_DIFF_LINES: usize = 4000;

/// The diff of one file: staged (index vs HEAD) or unstaged (work tree vs index). Untracked
/// files show as entirely added.
pub fn diff(root: &Path, path: &str, staged: bool, untracked: bool) -> Result<String, GitError> {
    let text = if untracked {
        let bytes = std::fs::read(root.join(path)).map_err(|e| format!("{path}: {e}"))?;
        if bytes.contains(&0) {
            return Ok(format!("Binary file {path} ({} bytes)", bytes.len()));
        }
        let content = String::from_utf8_lossy(&bytes);
        let mut out = format!(
            "new file {path}\n@@ -0,0 +1,{} @@\n",
            content.lines().count()
        );
        for line in content.lines() {
            out.push('+');
            out.push_str(line);
            out.push('\n');
        }
        out
    } else {
        let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
        if staged {
            args.push("--cached");
        }
        args.extend(["--", path]);
        git(root, &args)?
    };
    let mut lines = text.lines();
    let mut out: String = lines
        .by_ref()
        .take(MAX_DIFF_LINES)
        .flat_map(|l| [l, "\n"])
        .collect();
    let rest = lines.count();
    if rest > 0 {
        out.push_str(&format!("… {rest} more lines\n"));
    }
    Ok(out)
}

/// Something that changes the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitOp {
    /// `git init` in this folder (also writes a `.gitignore` for `target/` if missing).
    Init(PathBuf),
    Stage(Vec<String>),
    Unstage(Vec<String>),
    /// Throws away work-tree changes: restores tracked files, deletes untracked ones.
    Discard {
        tracked: Vec<String>,
        untracked: Vec<String>,
    },
    Commit {
        message: String,
        amend: bool,
        /// Staged first (commit everything visible when nothing was staged).
        stage: Vec<String>,
    },
    Switch(String),
    CreateBranch(String),
    Fetch,
    Pull,
    Push,
}

impl GitOp {
    /// What the operation is, for messages ("Commit failed: …").
    pub fn title(&self) -> &'static str {
        match self {
            Self::Init(_) => "Initialize repository",
            Self::Stage(_) => "Stage",
            Self::Unstage(_) => "Unstage",
            Self::Discard { .. } => "Discard changes",
            Self::Commit { .. } => "Commit",
            Self::Switch(_) => "Switch branch",
            Self::CreateBranch(_) => "Create branch",
            Self::Fetch => "Fetch",
            Self::Pull => "Pull",
            Self::Push => "Push",
        }
    }

    /// Talks to a remote (slow; worth a progress note).
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Fetch | Self::Pull | Self::Push)
    }
}

/// Runs `op` on the repository at `root` (`None`: not a repository yet, only
/// [`GitOp::Init`] works). Returns a short success message.
pub fn run(root: Option<&Path>, op: &GitOp) -> Result<String, GitError> {
    if let GitOp::Init(dir) = op {
        git(dir, &["init", "--quiet"])?;
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            std::fs::write(&ignore, "/target\n")
                .map_err(|e| format!("cannot write {}: {e}", ignore.display()))?;
        }
        return Ok(format!("Initialized a repository in {}", dir.display()));
    }
    let root = root.ok_or("not a git repository")?;
    let with_paths = |args: &[&str], paths: &[String]| -> Result<String, GitError> {
        let mut all: Vec<&str> = args.to_vec();
        all.push("--");
        all.extend(paths.iter().map(String::as_str));
        git(root, &all)
    };
    match op {
        GitOp::Init(_) => unreachable!("handled above"),
        GitOp::Stage(paths) => {
            with_paths(&["add", "--all"], paths)?;
            Ok(format!("Staged {}", count(paths.len(), "file")))
        }
        GitOp::Unstage(paths) => {
            // Before the first commit there is no HEAD to restore the index from.
            if git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok() {
                with_paths(&["restore", "--staged"], paths)?;
            } else {
                with_paths(&["rm", "--cached", "--quiet", "-r"], paths)?;
            }
            Ok(format!("Unstaged {}", count(paths.len(), "file")))
        }
        GitOp::Discard { tracked, untracked } => {
            if !tracked.is_empty() {
                with_paths(&["restore", "--worktree"], tracked)?;
            }
            if !untracked.is_empty() {
                with_paths(&["clean", "--force", "--quiet"], untracked)?;
            }
            Ok(format!(
                "Discarded changes to {}",
                count(tracked.len() + untracked.len(), "file")
            ))
        }
        GitOp::Commit {
            message,
            amend,
            stage,
        } => {
            if !stage.is_empty() {
                with_paths(&["add", "--all"], stage)?;
            }
            let message = message.trim();
            let mut args = vec!["commit", "--quiet"];
            if *amend {
                args.push("--amend");
            }
            if message.is_empty() {
                if !*amend {
                    return Err("Write a commit message first".into());
                }
                args.push("--no-edit");
            } else {
                args.extend(["-m", message]);
            }
            git(root, &args)?;
            let hash = git(root, &["rev-parse", "--short", "HEAD"])?;
            Ok(format!(
                "{} {}",
                if *amend { "Amended" } else { "Committed" },
                hash.trim()
            ))
        }
        GitOp::Switch(branch) => {
            git(root, &["switch", "--quiet", branch])?;
            Ok(format!("Switched to {branch}"))
        }
        GitOp::CreateBranch(branch) => {
            git(root, &["switch", "--quiet", "--create", branch.trim()])?;
            Ok(format!("Created and switched to {}", branch.trim()))
        }
        GitOp::Fetch => {
            git(root, &["fetch", "--all", "--prune", "--quiet"])?;
            Ok("Fetched".into())
        }
        GitOp::Pull => {
            git(root, &["pull", "--ff-only", "--quiet"])?;
            Ok("Pulled".into())
        }
        GitOp::Push => {
            let status = git(root, &["status", "--porcelain=v2", "--branch", "-z"])?;
            let (branch, _) = parse_status(&status);
            if branch.upstream.is_some() {
                git(root, &["push", "--quiet"])?;
            } else {
                // First push of a branch: publish it and track the remote branch.
                let remotes = git(root, &["remote"])?;
                let remote = remotes
                    .lines()
                    .find(|r| *r == "origin")
                    .or_else(|| remotes.lines().next())
                    .ok_or("No remote to push to (add one with `git remote add`)")?
                    .to_owned();
                git(
                    root,
                    &["push", "--quiet", "--set-upstream", &remote, "HEAD"],
                )?;
            }
            Ok("Pushed".into())
        }
    }
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

// =============================================================================================
// Background worker
// =============================================================================================

enum Request {
    Refresh,
    Diff {
        path: String,
        staged: bool,
        untracked: bool,
    },
    Run(GitOp),
}

/// The repository as last read by the worker.
#[derive(Debug, Clone)]
pub enum RepoState {
    /// Not read yet.
    Unknown,
    /// The folder is not inside a repository.
    NoRepository,
    Ready(Box<RepoStatus>),
    /// Git could not be run, or the status could not be read.
    Failed(GitError),
}

/// A diff shown in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    pub path: String,
    pub staged: bool,
    pub text: Result<String, GitError>,
}

/// Outcome of a [`GitOp`], for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpResult {
    pub title: &'static str,
    pub result: Result<String, GitError>,
}

enum Reply {
    State(RepoState),
    Diff(DiffView),
    Done(OpResult),
}

/// The editor's handle on its git worker thread. Dropping it stops the worker once the
/// command in progress finishes.
pub struct GitClient {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
    /// Requests sent and not answered yet.
    in_flight: usize,
    /// The remote operation in progress, if any.
    remote_op: Option<&'static str>,
    pub state: RepoState,
    pub diff: Option<DiffView>,
    /// Results of operations since the editor last took them.
    pub results: Vec<OpResult>,
    last_refresh: Option<Instant>,
    /// [`RepoStatus::states`] of the current status.
    states: HashMap<String, FileState>,
}

impl std::fmt::Debug for GitClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitClient")
            .field("in_flight", &self.in_flight)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl GitClient {
    /// Starts the worker for the repository containing `dir`. `notify` is called (on the
    /// worker thread) after every answer, e.g. to repaint the UI.
    pub fn start(dir: PathBuf, notify: impl Fn() + Send + 'static) -> Self {
        let (requests, inbox) = mpsc::channel::<Request>();
        let (outbox, replies) = mpsc::channel::<Reply>();
        let spawned = std::thread::Builder::new()
            .name("ab-git".into())
            .spawn(move || worker(dir, inbox, outbox, notify));
        let mut client = Self {
            requests,
            replies,
            in_flight: 0,
            remote_op: None,
            state: RepoState::Unknown,
            diff: None,
            results: Vec::new(),
            last_refresh: None,
            states: HashMap::new(),
        };
        match spawned {
            Ok(_) => client.refresh(),
            Err(err) => client.state = RepoState::Failed(format!("cannot start git worker: {err}")),
        }
        client
    }

    fn send(&mut self, request: Request) {
        // An operation is answered twice: its result, then the status after it.
        let replies = if matches!(request, Request::Run(_)) {
            2
        } else {
            1
        };
        if self.requests.send(request).is_ok() {
            self.in_flight += replies;
        }
    }

    /// Re-reads the status (branch, changes, history).
    pub fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        self.send(Request::Refresh);
    }

    /// Refreshes when idle and the last refresh is older than `every`.
    pub fn refresh_every(&mut self, every: Duration) {
        if self.in_flight == 0 && self.last_refresh.is_none_or(|t| t.elapsed() >= every) {
            self.refresh();
        }
    }

    pub fn show_diff(&mut self, path: &str, staged: bool, untracked: bool) {
        self.send(Request::Diff {
            path: path.to_owned(),
            staged,
            untracked,
        });
    }

    pub fn run(&mut self, op: GitOp) {
        if op.is_remote() {
            self.remote_op = Some(op.title());
        }
        self.last_refresh = Some(Instant::now());
        self.send(Request::Run(op));
    }

    /// Commands are queued or running.
    pub fn is_busy(&self) -> bool {
        self.in_flight > 0
    }

    /// The fetch/pull/push in progress.
    pub fn remote_op(&self) -> Option<&'static str> {
        self.remote_op
    }

    pub fn status(&self) -> Option<&RepoStatus> {
        match &self.state {
            RepoState::Ready(status) => Some(status),
            _ => None,
        }
    }

    /// The change state of the file at `path` (absolute), if it has changes.
    pub fn file_state(&self, path: &Path) -> Option<FileState> {
        if self.states.is_empty() {
            return None;
        }
        let relative = path.strip_prefix(&self.status()?.root).ok()?;
        let key = relative.to_string_lossy().replace('\\', "/");
        self.states.get(&key).copied()
    }

    /// Applies the worker's answers. Call once per frame.
    pub fn poll(&mut self) {
        loop {
            match self.replies.try_recv() {
                Ok(reply) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    match reply {
                        Reply::State(state) => {
                            self.state = state;
                            self.states = self.status().map(|s| s.states()).unwrap_or_default();
                            // The diff may be stale now: drop it if its file changed state.
                            if let (Some(diff), Some(status)) = (&self.diff, self.status()) {
                                let still = status.changes.iter().any(|c| {
                                    c.path == diff.path
                                        && if diff.staged {
                                            c.staged.is_some()
                                        } else {
                                            c.unstaged.is_some()
                                        }
                                });
                                if !still {
                                    self.diff = None;
                                }
                            }
                        }
                        Reply::Diff(diff) => self.diff = Some(diff),
                        Reply::Done(result) => {
                            if self.remote_op == Some(result.title) {
                                self.remote_op = None;
                            }
                            self.results.push(result);
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.in_flight = 0;
                    break;
                }
            }
        }
    }
}

fn worker(dir: PathBuf, inbox: Receiver<Request>, outbox: Sender<Reply>, notify: impl Fn()) {
    let read = |root: &mut Option<PathBuf>| -> RepoState {
        match discover(&dir) {
            Ok(Some(found)) => {
                let state = match read_status(&found) {
                    Ok(status) => RepoState::Ready(Box::new(status)),
                    Err(err) => RepoState::Failed(err),
                };
                *root = Some(found);
                state
            }
            Ok(None) => {
                *root = None;
                RepoState::NoRepository
            }
            Err(err) => RepoState::Failed(err),
        }
    };
    let mut root: Option<PathBuf> = None;
    while let Ok(first) = inbox.recv() {
        // Collapse a burst of refreshes into one read; every request is still answered so
        // the client's in-flight count stays right.
        let mut batch = vec![first];
        while let Ok(more) = inbox.try_recv() {
            batch.push(more);
        }
        let mut refreshed: Option<RepoState> = None;
        for request in batch {
            let reply = match request {
                Request::Refresh => Reply::State(match &refreshed {
                    Some(state) => state.clone(),
                    None => refreshed.insert(read(&mut root)).clone(),
                }),
                Request::Diff {
                    path,
                    staged,
                    untracked,
                } => {
                    if root.is_none() {
                        root = discover(&dir).ok().flatten();
                    }
                    let text = match &root {
                        Some(root) => diff(root, &path, staged, untracked),
                        None => Err("not a git repository".into()),
                    };
                    Reply::Diff(DiffView { path, staged, text })
                }
                Request::Run(op) => {
                    if root.is_none() {
                        root = discover(&dir).ok().flatten();
                    }
                    let result = run(root.as_deref(), &op);
                    if let Err(err) = &result {
                        tracing::warn!("git: {} failed: {err}", op.title());
                    }
                    let done = Reply::Done(OpResult {
                        title: op.title(),
                        result,
                    });
                    // The operation changed the repository: whatever was read is stale.
                    refreshed = None;
                    let state = read(&mut root);
                    if outbox.send(done).is_err() || outbox.send(Reply::State(state)).is_err() {
                        return;
                    }
                    notify();
                    continue;
                }
            };
            if outbox.send(reply).is_err() {
                return;
            }
            notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2_status() {
        let output = concat!(
            "# branch.oid 1234567890abcdef\0",
            "# branch.head main\0",
            "# branch.upstream origin/main\0",
            "# branch.ab +2 -1\0",
            "1 .M N... 100644 100644 100644 aaaa bbbb scenes/level 1.toml\0",
            "1 A. N... 000000 100644 100644 0000 cccc src/new.rs\0",
            "1 MM N... 100644 100644 100644 aaaa bbbb game.toml\0",
            "2 R. N... 100644 100644 100644 aaaa bbbb R100 models/new.glb\0models/old.glb\0",
            "u UU N... 100644 100644 100644 100644 aaaa bbbb cccc both.txt\0",
            "? notes/todo.md\0",
        );
        let (branch, changes) = parse_status(output);
        assert_eq!(branch.head.as_deref(), Some("main"));
        assert_eq!(branch.oid.as_deref(), Some("1234567890abcdef"));
        assert_eq!(branch.upstream.as_deref(), Some("origin/main"));
        assert_eq!((branch.ahead, branch.behind), (2, 1));
        let find = |path: &str| changes.iter().find(|c| c.path == path).unwrap().clone();
        let level = find("scenes/level 1.toml");
        assert_eq!(
            (level.staged, level.unstaged),
            (None, Some(FileState::Modified))
        );
        assert_eq!(find("src/new.rs").staged, Some(FileState::Added));
        let both = find("game.toml");
        assert_eq!(
            (both.staged, both.unstaged),
            (Some(FileState::Modified), Some(FileState::Modified))
        );
        let renamed = find("models/new.glb");
        assert_eq!(renamed.staged, Some(FileState::Renamed));
        assert_eq!(renamed.original.as_deref(), Some("models/old.glb"));
        assert_eq!(find("both.txt").unstaged, Some(FileState::Conflicted));
        assert_eq!(find("notes/todo.md").state(), Some(FileState::Untracked));
        assert_eq!(changes.len(), 6, "the rename's origin is not its own entry");
    }

    #[test]
    fn parses_fresh_and_detached_repositories() {
        let (branch, changes) = parse_status("# branch.oid (initial)\0# branch.head master\0");
        assert_eq!(branch.oid, None);
        assert_eq!(branch.label(), "master");
        assert!(changes.is_empty());
        let (detached, _) = parse_status("# branch.oid abcdef1234\0# branch.head (detached)\0");
        assert_eq!(detached.head, None);
        assert_eq!(detached.label(), "(abcdef12)");
    }

    #[test]
    fn parses_log() {
        let log = parse_log(
            "abc123\u{1f}Ann\u{1f}2 hours ago\u{1f}Add level\u{1e}\n\
             def456\u{1f}Bo\u{1f}3 days ago\u{1f}Fix: a|b\u{1e}",
        );
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].hash, "abc123");
        assert_eq!(log[1].subject, "Fix: a|b");
        assert_eq!(log[1].when, "3 days ago");
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn temp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ab-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn configure(root: &Path) {
        git(root, &["config", "user.name", "Editor Test"]).unwrap();
        git(root, &["config", "user.email", "editor@example.com"]).unwrap();
        git(root, &["config", "commit.gpgsign", "false"]).unwrap();
    }

    #[test]
    fn stage_commit_branch_and_discard() {
        if !git_available() {
            return;
        }
        let dir = temp_repo("ops");
        assert_eq!(discover(&dir).unwrap(), None);
        run(None, &GitOp::Init(dir.clone())).unwrap();
        let root = discover(&dir).unwrap().expect("a repository now");
        configure(&root);
        assert!(root.join(".gitignore").is_file());

        std::fs::write(root.join("level.toml"), "[[node]]\n").unwrap();
        let status = read_status(&root).unwrap();
        assert_eq!(status.branch.oid, None, "no commit yet");
        let untracked: Vec<String> = status.unstaged().map(|c| c.path.clone()).collect();
        assert_eq!(untracked, [".gitignore", "level.toml"]);

        // Unstaging works before the first commit too.
        run(Some(&root), &GitOp::Stage(untracked.clone())).unwrap();
        assert_eq!(read_status(&root).unwrap().staged().count(), 2);
        run(Some(&root), &GitOp::Unstage(vec!["level.toml".into()])).unwrap();
        assert_eq!(read_status(&root).unwrap().staged().count(), 1);

        let err = run(
            Some(&root),
            &GitOp::Commit {
                message: "  ".into(),
                amend: false,
                stage: vec![],
            },
        );
        assert!(err.is_err(), "a message is required");
        let done = run(
            Some(&root),
            &GitOp::Commit {
                message: "First scene".into(),
                amend: false,
                stage: untracked,
            },
        )
        .unwrap();
        assert!(done.starts_with("Committed"), "{done}");
        let status = read_status(&root).unwrap();
        assert!(status.changes.is_empty());
        assert_eq!(status.log.len(), 1);
        assert_eq!(status.log[0].subject, "First scene");

        // A change, its diff, then throwing it away.
        std::fs::write(root.join("level.toml"), "[[node]]\nname = \"Root\"\n").unwrap();
        std::fs::write(root.join("scratch.txt"), "temp\n").unwrap();
        let text = diff(&root, "level.toml", false, false).unwrap();
        assert!(text.contains("+name = \"Root\""), "{text}");
        let text = diff(&root, "scratch.txt", false, true).unwrap();
        assert!(text.contains("+temp"), "{text}");
        run(
            Some(&root),
            &GitOp::Discard {
                tracked: vec!["level.toml".into()],
                untracked: vec!["scratch.txt".into()],
            },
        )
        .unwrap();
        assert!(read_status(&root).unwrap().changes.is_empty());
        assert!(!root.join("scratch.txt").exists());

        // Branches.
        run(Some(&root), &GitOp::CreateBranch("feature/lights".into())).unwrap();
        let status = read_status(&root).unwrap();
        assert_eq!(status.branch.head.as_deref(), Some("feature/lights"));
        assert_eq!(status.branches.len(), 2);
        let other = status
            .branches
            .iter()
            .find(|b| *b != "feature/lights")
            .unwrap()
            .clone();
        run(Some(&root), &GitOp::Switch(other.clone())).unwrap();
        assert_eq!(read_status(&root).unwrap().branch.head, Some(other));

        // No remote: pushing explains itself instead of hanging.
        let err = run(Some(&root), &GitOp::Push).unwrap_err();
        assert!(err.contains("No remote"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn worker_answers_in_the_background() {
        if !git_available() {
            return;
        }
        let dir = temp_repo("worker");
        let mut client = GitClient::start(dir.clone(), || {});
        let wait = |client: &mut GitClient| {
            let start = Instant::now();
            while client.is_busy() {
                assert!(start.elapsed() < Duration::from_secs(20), "worker hung");
                std::thread::sleep(Duration::from_millis(5));
                client.poll();
            }
        };
        wait(&mut client);
        assert!(matches!(client.state, RepoState::NoRepository));

        client.run(GitOp::Init(dir.clone()));
        wait(&mut client);
        let result = client.results.pop().unwrap();
        assert!(result.result.is_ok(), "{result:?}");
        let status = client.status().expect("a repository now");
        assert_eq!(status.changes.len(), 1, ".gitignore is new");

        client.show_diff(".gitignore", false, true);
        client.refresh();
        client.refresh();
        wait(&mut client);
        let diff = client.diff.as_ref().unwrap();
        assert!(diff.text.as_ref().unwrap().contains("+/target"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
