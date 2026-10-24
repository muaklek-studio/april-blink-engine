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

/// A local branch and how it compares with its upstream.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct LocalBranch {
    pub name: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Its upstream was deleted on the remote.
    pub gone: bool,
}

/// A stash entry (`stash@{index}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    pub index: usize,
    pub message: String,
}

/// An operation git stopped in the middle of, waiting for conflicts to be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InProgress {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl InProgress {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Merge => "Merge",
            Self::Rebase => "Rebase",
            Self::CherryPick => "Cherry-pick",
            Self::Revert => "Revert",
        }
    }
}

/// Everything the panel shows about a repository.
#[derive(Debug, Clone, Default)]
pub struct RepoStatus {
    /// Work tree root.
    pub root: PathBuf,
    pub branch: BranchInfo,
    pub changes: Vec<FileChange>,
    pub branches: Vec<String>,
    /// The local branches with their upstreams, sorted by name.
    pub local: Vec<LocalBranch>,
    /// Remote-tracking branches (`origin/main`), without the remotes' `HEAD`.
    pub remote_branches: Vec<String>,
    /// Tags, newest first.
    pub tags: Vec<String>,
    pub stashes: Vec<Stash>,
    pub remotes: Vec<String>,
    /// A merge, rebase, cherry-pick or revert stopped on conflicts.
    pub in_progress: Option<InProgress>,
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

    /// Files with unresolved conflicts.
    pub fn conflicts(&self) -> impl Iterator<Item = &FileChange> {
        self.changes
            .iter()
            .filter(|c| c.unstaged == Some(FileState::Conflicted))
    }

    /// What decides the commit graph: HEAD and every ref. The graph is read again when it
    /// changes.
    pub fn graph_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (
            &self.branch.oid,
            &self.branch.head,
            &self.local,
            &self.remote_branches,
            &self.tags,
        )
            .hash(&mut hasher);
        hasher.finish()
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

/// Parses `git for-each-ref --format=%(refname)%1f%(upstream:short)%1f%(upstream:track)`
/// over heads, remotes and tags: (local branches, remote branches, tags).
pub fn parse_refs(output: &str) -> (Vec<LocalBranch>, Vec<String>, Vec<String>) {
    let (mut local, mut remote, mut tags) = (Vec::new(), Vec::new(), Vec::new());
    for line in output.lines() {
        let mut fields = line.split(FIELD);
        let Some(name) = fields.next() else { continue };
        let upstream = fields.next().filter(|u| !u.is_empty()).map(str::to_owned);
        let track = fields.next().unwrap_or_default();
        if let Some(branch) = name.strip_prefix("refs/heads/") {
            let number = |key: &str| {
                track
                    .split_once(key)
                    .and_then(|(_, rest)| {
                        rest.trim_start()
                            .split(|c: char| !c.is_ascii_digit())
                            .next()?
                            .parse()
                            .ok()
                    })
                    .unwrap_or(0)
            };
            local.push(LocalBranch {
                name: branch.to_owned(),
                upstream,
                ahead: number("ahead"),
                behind: number("behind"),
                gone: track.contains("gone"),
            });
        } else if let Some(branch) = name.strip_prefix("refs/remotes/") {
            // `origin/HEAD` points at the default branch: not a branch of its own.
            if branch.contains('/') && !branch.ends_with("/HEAD") {
                remote.push(branch.to_owned());
            }
        } else if let Some(tag) = name.strip_prefix("refs/tags/") {
            tags.push(tag.to_owned());
        }
    }
    local.sort_by(|a, b| a.name.cmp(&b.name));
    remote.sort();
    (local, remote, tags)
}

/// Parses `git stash list --format=%gd%x1f%s`.
pub fn parse_stashes(output: &str) -> Vec<Stash> {
    output
        .lines()
        .filter_map(|line| {
            let (name, message) = line.split_once(FIELD)?;
            let index = name
                .strip_prefix("stash@{")?
                .strip_suffix('}')?
                .parse()
                .ok()?;
            Some(Stash {
                index,
                message: message.to_owned(),
            })
        })
        .collect()
}

/// What a label on a commit names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    /// HEAD itself (detached), or the branch HEAD is on.
    Head,
    Local,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

/// One commit of the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCommit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Seconds since 1970 (author date).
    pub time: i64,
    /// Local date and time, `2026-10-24 14:05`.
    pub date: String,
    pub refs: Vec<RefLabel>,
    pub subject: String,
}

/// `git log` format of [`parse_graph`].
pub const GRAPH_FORMAT: &str =
    "--format=%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%ad%x1f%D%x1f%s%x1e";
/// Dates as [`GraphCommit::date`] shows them.
pub const DATE_FORMAT: &str = "--date=format-local:%Y-%m-%d %H:%M";

/// Parses `git log` output in [`GRAPH_FORMAT`]; `remotes` tell remote branches from local
/// ones with a slash in their name.
pub fn parse_graph(output: &str, remotes: &[String]) -> Vec<GraphCommit> {
    output
        .split(RECORD)
        .map(|r| r.trim_matches(['\n', '\r']))
        .filter(|r| !r.is_empty())
        .filter_map(|record| {
            let f: Vec<&str> = record.splitn(9, FIELD).collect();
            if f.len() < 9 {
                return None;
            }
            Some(GraphCommit {
                hash: f[0].to_owned(),
                short: f[1].to_owned(),
                parents: f[2].split_whitespace().map(str::to_owned).collect(),
                author: f[3].to_owned(),
                email: f[4].to_owned(),
                time: f[5].parse().unwrap_or(0),
                date: f[6].to_owned(),
                refs: parse_decorations(f[7], remotes),
                subject: f[8].to_owned(),
            })
        })
        .collect()
}

/// `HEAD -> main, origin/main, tag: v1.0` → labels, HEAD's branch first.
fn parse_decorations(text: &str, remotes: &[String]) -> Vec<RefLabel> {
    let mut labels: Vec<RefLabel> = text
        .split(", ")
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .filter_map(|d| {
            let (name, kind) = if let Some(branch) = d.strip_prefix("HEAD -> ") {
                (branch, RefKind::Head)
            } else if d == "HEAD" {
                ("HEAD", RefKind::Head)
            } else if let Some(tag) = d.strip_prefix("tag: ") {
                (tag, RefKind::Tag)
            } else if d.ends_with("/HEAD") {
                return None;
            } else if remotes.iter().any(|r| {
                d.strip_prefix(r.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            }) {
                (d, RefKind::Remote)
            } else {
                (d, RefKind::Local)
            };
            Some(RefLabel {
                name: name.to_owned(),
                kind,
            })
        })
        .collect();
    labels.sort_by_key(|l| l.kind);
    labels
}

/// Where a line of the graph starts or ends within its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Middle,
    Bottom,
}

/// A line of the graph within one row: from (lane, edge) to (lane, edge), colored after
/// `color` (a lane index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub from: (usize, Edge),
    pub to: (usize, Edge),
    pub color: usize,
}

/// How one commit's row of the graph is drawn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane of the commit's dot.
    pub lane: usize,
    pub segments: Vec<Segment>,
    /// Lanes in use in this row (the graph column's width).
    pub width: usize,
}

/// Lays out the commit graph (commits newest first, in topological order): one lane per
/// line of history, a commit on the lane that reached it, its first parent continuing its
/// lane and other parents (merges) on their own.
pub fn layout_graph(commits: &[GraphCommit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let hash = commit.hash.as_str();
        let free = |lanes: &mut Vec<Option<&str>>, after: usize| -> usize {
            match (after..lanes.len()).find(|&i| lanes[i].is_none()) {
                Some(i) => i,
                None => {
                    lanes.push(None);
                    lanes.len() - 1
                }
            }
        };
        let lane = match lanes.iter().position(|l| *l == Some(hash)) {
            Some(lane) => lane,
            None => free(&mut lanes, 0),
        };
        let mut row = GraphRow {
            lane,
            ..Default::default()
        };
        // Lines coming in from above: every lane waiting for this commit joins its dot, the
        // others pass by.
        let mut passing = Vec::new();
        for (i, waiting) in lanes.iter().enumerate() {
            match waiting {
                Some(h) if *h == hash => row.segments.push(Segment {
                    from: (i, Edge::Top),
                    to: (lane, Edge::Middle),
                    color: i,
                }),
                Some(_) => passing.push(i),
                None => {}
            }
        }
        for lane_slot in lanes.iter_mut() {
            if *lane_slot == Some(hash) {
                *lane_slot = None;
            }
        }
        // Lines going down to the parents.
        for (n, parent) in commit.parents.iter().enumerate() {
            let parent = parent.as_str();
            // The first parent goes on down this lane (even when another lane already waits
            // for it: the two meet at the parent); other parents join a waiting lane.
            let waiting = if n == 0 && lanes[lane].is_none() {
                None
            } else {
                lanes.iter().position(|l| *l == Some(parent))
            };
            let target = match waiting {
                Some(existing) => existing,
                None => {
                    let slot = if n == 0 && lanes[lane].is_none() {
                        lane
                    } else {
                        free(&mut lanes, lane + 1)
                    };
                    lanes[slot] = Some(parent);
                    slot
                }
            };
            row.segments.push(Segment {
                from: (lane, Edge::Middle),
                to: (target, Edge::Bottom),
                color: if n == 0 { lane } else { target },
            });
        }
        for i in passing {
            row.segments.push(Segment {
                from: (i, Edge::Top),
                to: (i, Edge::Bottom),
                color: i,
            });
        }
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
        row.width = row
            .segments
            .iter()
            .flat_map(|s| [s.from.0, s.to.0])
            .chain([lane])
            .max()
            .unwrap_or(0)
            + 1;
        rows.push(row);
    }
    rows
}

/// A file a commit changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: String,
    pub original: Option<String>,
    pub state: FileState,
    /// Lines added and removed (`None` for binary files).
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

/// One commit in full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub date: String,
    pub committer: String,
    pub message: String,
    pub files: Vec<CommitFile>,
}

/// Git's empty tree: what a root commit is compared with.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// What `hash` is compared with to show its changes: its first parent, or nothing.
fn base_of(root: &Path, hash: &str) -> String {
    match git(
        root,
        &["rev-parse", "--verify", "--quiet", &format!("{hash}^1")],
    ) {
        Ok(parent) => parent.trim().to_owned(),
        Err(_) => EMPTY_TREE.to_owned(),
    }
}

/// Parses `git diff --name-status -z -M` into (path, original, state).
fn parse_name_status(output: &str) -> Vec<(String, Option<String>, FileState)> {
    let mut out = Vec::new();
    let mut fields = output.split('\0').filter(|f| !f.is_empty());
    while let Some(code) = fields.next() {
        let state = match code.as_bytes().first() {
            Some(b'U') => FileState::Conflicted,
            Some(&c) => FileState::from_code(c).unwrap_or(FileState::Modified),
            None => continue,
        };
        if matches!(state, FileState::Renamed | FileState::Copied) {
            let (Some(from), Some(to)) = (fields.next(), fields.next()) else {
                break;
            };
            out.push((to.to_owned(), Some(from.to_owned()), state));
        } else if let Some(path) = fields.next() {
            out.push((path.to_owned(), None, state));
        }
    }
    out
}

/// Parses `git diff --numstat -z -M` into path → (added, removed); `-` (binary) is `None`.
fn parse_numstat(output: &str) -> HashMap<String, (Option<u32>, Option<u32>)> {
    let mut out = HashMap::new();
    let mut fields = output.split('\0');
    while let Some(record) = fields.next() {
        let mut parts = record.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // A rename has an empty path, then its old and new paths as records of their own.
        let path = if path.is_empty() {
            let _from = fields.next();
            match fields.next() {
                Some(to) => to,
                None => break,
            }
        } else {
            path
        };
        out.insert(path.to_owned(), (added.parse().ok(), removed.parse().ok()));
    }
    out
}

/// Reads one commit: its message, author and the files it changed (against its first
/// parent).
pub fn read_commit(root: &Path, hash: &str) -> Result<CommitDetail, GitError> {
    let show = git(
        root,
        &[
            "show",
            "-s",
            "--no-color",
            DATE_FORMAT,
            "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%ad%x1f%cn%x1f%B",
            hash,
        ],
    )?;
    let f: Vec<&str> = show.splitn(7, FIELD).collect();
    if f.len() < 7 {
        return Err(format!("cannot read commit {hash}"));
    }
    let base = base_of(root, f[0]);
    let range = [base.as_str(), f[0]];
    let mut args = vec!["diff", "--no-color", "-M", "--name-status", "-z"];
    args.extend(range);
    let names = parse_name_status(&git(root, &args)?);
    let mut args = vec!["diff", "--no-color", "-M", "--numstat", "-z"];
    args.extend(range);
    let counts = parse_numstat(&git(root, &args)?);
    let files = names
        .into_iter()
        .map(|(path, original, state)| {
            let (added, removed) = counts.get(&path).copied().unwrap_or((None, None));
            CommitFile {
                path,
                original,
                state,
                added,
                removed,
            }
        })
        .collect();
    Ok(CommitDetail {
        hash: f[0].to_owned(),
        parents: f[1].split_whitespace().map(str::to_owned).collect(),
        author: f[2].to_owned(),
        email: f[3].to_owned(),
        date: f[4].to_owned(),
        committer: f[5].to_owned(),
        message: f[6].trim_end().to_owned(),
        files,
    })
}

/// The commit graph: up to `limit` commits of every branch and tag, newest first.
pub fn read_graph(
    root: &Path,
    limit: usize,
    remotes: &[String],
) -> Result<Vec<GraphCommit>, GitError> {
    if git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err() {
        return Ok(Vec::new());
    }
    let limit = format!("--max-count={limit}");
    let output = git(
        root,
        &[
            "log",
            "--no-color",
            "--topo-order",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            &limit,
            DATE_FORMAT,
            GRAPH_FORMAT,
        ],
    )?;
    Ok(parse_graph(&output, remotes))
}

/// Seconds since 1970 → "3 hours ago" (relative to `now`).
pub fn relative_time(time: i64, now: i64) -> String {
    let seconds = (now - time).max(0);
    let (n, unit) = match seconds {
        s if s < 60 => return "just now".into(),
        s if s < 3600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 86_400 * 30 => (s / 86_400, "day"),
        s if s < 86_400 * 365 => (s / (86_400 * 30), "month"),
        s => (s / (86_400 * 365), "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

// =============================================================================================
// Running git
// =============================================================================================

/// Output of a failed git command (its error message).
pub type GitError = String;

/// Whether git's config sets `core.sshCommand` for `dir` (asked once per folder).
fn has_ssh_command(dir: &Path) -> bool {
    use std::sync::{Mutex, OnceLock};
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(&set) = known.lock().unwrap_or_else(|e| e.into_inner()).get(dir) {
        return set;
    }
    let set = ab_project::process::command("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "--get", "core.sshCommand"])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.trim_ascii().is_empty());
    known
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(dir.to_path_buf(), set);
    set
}

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
        // Paths are file names, not patterns: discarding `level[1].abscene` must not touch
        // `level1.abscene`.
        .env("GIT_LITERAL_PATHSPECS", "1")
        // Stable, parseable messages.
        .env("LC_ALL", "C");
    // Never wait on a passphrase prompt nobody sees; but the user's own SSH setup
    // (`GIT_SSH`, `core.sshCommand`) wins, as GIT_SSH_COMMAND would override it.
    if std::env::var_os("GIT_SSH_COMMAND").is_none()
        && std::env::var_os("GIT_SSH").is_none()
        && !has_ssh_command(dir)
    {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    // The git worker runs one command at a time: one that hangs (an unreachable remote)
    // must not hold up every refresh and commit behind it.
    let timeout = match args.first().copied() {
        Some("fetch") => Duration::from_secs(120),
        Some("push" | "pull" | "lfs" | "clone") => Duration::from_secs(30 * 60),
        _ => Duration::from_secs(5 * 60),
    };
    let output =
        ab_project::process::output_with_timeout(&mut command, timeout).map_err(|err| match err
            .kind()
        {
            std::io::ErrorKind::NotFound => "git is not installed (or not on PATH)".to_owned(),
            std::io::ErrorKind::TimedOut => format!(
                "git {} gave no answer in {} minutes and was stopped (is the remote reachable?)",
                args.first().unwrap_or(&""),
                timeout.as_secs().div_ceil(60)
            ),
            _ => format!("cannot run git: {err}"),
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
    let refs = git(
        root,
        &[
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname)%1f%(upstream:short)%1f%(upstream:track)",
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
        ],
    )?;
    let (local, remote_branches, tags) = parse_refs(&refs);
    let branches = local.iter().map(|b| b.name.clone()).collect();
    let remotes = git(root, &["remote"])?.lines().map(str::to_owned).collect();
    let stashes = parse_stashes(&git(root, &["stash", "list", "--format=%gd%x1f%s"])?);
    let git_dir = PathBuf::from(git(root, &["rev-parse", "--absolute-git-dir"])?.trim());
    let in_progress =
        if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
            Some(InProgress::Rebase)
        } else if git_dir.join("MERGE_HEAD").is_file() {
            Some(InProgress::Merge)
        } else if git_dir.join("CHERRY_PICK_HEAD").is_file() {
            Some(InProgress::CherryPick)
        } else if git_dir.join("REVERT_HEAD").is_file() {
            Some(InProgress::Revert)
        } else {
            None
        };
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
        local,
        remote_branches,
        tags,
        stashes,
        remotes,
        in_progress,
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
    Ok(truncated(&text))
}

/// `text` cut to [`MAX_DIFF_LINES`] lines.
fn truncated(text: &str) -> String {
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
    out
}

/// The diff of one file in a commit, against the commit's first parent.
pub fn commit_diff(root: &Path, hash: &str, path: &str) -> Result<String, GitError> {
    let base = base_of(root, hash);
    let text = git(
        root,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "-M",
            &base,
            hash,
            "--",
            path,
        ],
    )?;
    Ok(truncated(&text))
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
    /// Brings in the upstream's commits: merged (or rebased onto) when both sides moved.
    Pull {
        rebase: bool,
    },
    Push,
    /// Pull, then push: the branch and its upstream in step.
    Sync {
        rebase: bool,
    },
    /// Merges a branch (or commit) into the current one.
    Merge(String),
    /// Abandons the merge, rebase, cherry-pick or revert in progress.
    Abort(InProgress),
    /// Goes on with a rebase, cherry-pick or revert once its conflicts are resolved (a
    /// merge goes on with a commit).
    Continue(InProgress),
    /// Resolves conflicts by taking one side: `theirs` (the branch merged in) or ours.
    Resolve {
        paths: Vec<String>,
        theirs: bool,
    },
    /// Marks conflicts resolved as the files are now.
    MarkResolved(Vec<String>),
    StashPush {
        message: String,
        untracked: bool,
    },
    StashPop(usize),
    StashApply(usize),
    StashDrop(usize),
    DeleteBranch {
        name: String,
        force: bool,
    },
    /// Checks out a remote branch as a local branch tracking it.
    CheckoutRemote(String),
    /// Checks out a commit, HEAD detached.
    CheckoutCommit(String),
    /// A new branch at a commit, checked out.
    CreateBranchAt {
        name: String,
        at: String,
    },
    CreateTag {
        name: String,
        at: String,
    },
    CherryPick(String),
    Revert(String),
}

impl GitOp {
    /// What the operation is, for messages ("Commit failed: …").
    pub fn title(&self) -> &'static str {
        match self {
            Self::Init(_) => "Initialize repository",
            Self::Stage(_) => "Stage",
            Self::Unstage(_) => "Unstage",
            Self::Discard { .. } => "Discard changes",
            Self::Commit { .. } => COMMIT_TITLE,
            Self::Switch(_) => "Switch branch",
            Self::CreateBranch(_) => "Create branch",
            Self::Fetch => "Fetch",
            Self::Pull { .. } => "Pull",
            Self::Push => "Push",
            Self::Sync { .. } => "Sync",
            Self::Merge(_) => "Merge",
            Self::Abort(_) => "Abort",
            Self::Continue(_) => "Continue",
            Self::Resolve { .. } => "Resolve conflicts",
            Self::MarkResolved(_) => "Mark resolved",
            Self::StashPush { .. } => "Stash",
            Self::StashPop(_) => "Pop stash",
            Self::StashApply(_) => "Apply stash",
            Self::StashDrop(_) => "Drop stash",
            Self::DeleteBranch { .. } => "Delete branch",
            Self::CheckoutRemote(_) => "Check out",
            Self::CheckoutCommit(_) => "Check out commit",
            Self::CreateBranchAt { .. } => "Create branch",
            Self::CreateTag { .. } => "Create tag",
            Self::CherryPick(_) => "Cherry-pick",
            Self::Revert(_) => "Revert",
        }
    }

    /// Talks to a remote (slow; worth a progress note).
    pub fn is_remote(&self) -> bool {
        matches!(
            self,
            Self::Fetch | Self::Pull { .. } | Self::Push | Self::Sync { .. }
        )
    }

    /// May change the files of the work tree (not only the index or the refs).
    pub fn changes_work_tree(&self) -> bool {
        !matches!(
            self,
            Self::Init(_)
                | Self::Stage(_)
                | Self::Unstage(_)
                | Self::Commit { .. }
                | Self::Fetch
                | Self::Push
                | Self::MarkResolved(_)
                | Self::StashDrop(_)
                | Self::DeleteBranch { .. }
                | Self::CreateTag { .. }
        )
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
                // Concluding a merge: git's prepared message ("Merge branch …").
                let merging =
                    git(root, &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"]).is_ok();
                if !*amend && !merging {
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
        GitOp::Pull { rebase } => {
            pull(root, *rebase)?;
            Ok("Pulled".into())
        }
        GitOp::Sync { rebase } => {
            let status = git(root, &["status", "--porcelain=v2", "--branch", "-z"])?;
            let (branch, _) = parse_status(&status);
            if branch.upstream.is_some() {
                pull(root, *rebase)?;
            }
            run(Some(root), &GitOp::Push)?;
            Ok("Synced with the remote".into())
        }
        GitOp::Merge(branch) => match git(root, &["merge", "--no-edit", branch]) {
            Ok(_) => Ok(format!("Merged {branch}")),
            Err(err) => Err(stopped_on_conflicts(root, "Merge", err)),
        },
        GitOp::Abort(what) => {
            let command = match what {
                InProgress::Merge => "merge",
                InProgress::Rebase => "rebase",
                InProgress::CherryPick => "cherry-pick",
                InProgress::Revert => "revert",
            };
            git(root, &[command, "--abort"])?;
            Ok(format!("{} aborted", what.describe()))
        }
        GitOp::Continue(what) => {
            let command = match what {
                InProgress::Merge => {
                    return run(
                        Some(root),
                        &GitOp::Commit {
                            message: String::new(),
                            amend: false,
                            stage: Vec::new(),
                        },
                    )
                }
                InProgress::Rebase => "rebase",
                InProgress::CherryPick => "cherry-pick",
                InProgress::Revert => "revert",
            };
            match git(root, &[command, "--continue"]) {
                Ok(_) => Ok(format!("{} finished", what.describe())),
                Err(err) => Err(stopped_on_conflicts(root, what.describe(), err)),
            }
        }
        GitOp::Resolve { paths, theirs } => {
            let side = if *theirs { "--theirs" } else { "--ours" };
            with_paths(&["checkout", side], paths)?;
            with_paths(&["add"], paths)?;
            Ok(format!(
                "Resolved {} with {} version",
                count(paths.len(), "file"),
                if *theirs { "their" } else { "our" }
            ))
        }
        GitOp::MarkResolved(paths) => {
            with_paths(&["add"], paths)?;
            Ok(format!("Marked {} resolved", count(paths.len(), "file")))
        }
        GitOp::StashPush { message, untracked } => {
            let mut args = vec!["stash", "push", "--quiet"];
            if *untracked {
                args.push("--include-untracked");
            }
            if !message.trim().is_empty() {
                args.extend(["--message", message.trim()]);
            }
            git(root, &args)?;
            Ok("Changes stashed".into())
        }
        GitOp::StashPop(index) => {
            let name = format!("stash@{{{index}}}");
            match git(root, &["stash", "pop", "--quiet", &name]) {
                Ok(_) => Ok("Stash applied and dropped".into()),
                Err(err) => Err(stopped_on_conflicts(root, "Stash pop", err)),
            }
        }
        GitOp::StashApply(index) => {
            let name = format!("stash@{{{index}}}");
            match git(root, &["stash", "apply", "--quiet", &name]) {
                Ok(_) => Ok("Stash applied".into()),
                Err(err) => Err(stopped_on_conflicts(root, "Stash apply", err)),
            }
        }
        GitOp::StashDrop(index) => {
            git(
                root,
                &["stash", "drop", "--quiet", &format!("stash@{{{index}}}")],
            )?;
            Ok("Stash dropped".into())
        }
        GitOp::DeleteBranch { name, force } => {
            git(root, &["branch", if *force { "-D" } else { "-d" }, name])?;
            Ok(format!("Deleted branch {name}"))
        }
        GitOp::CheckoutRemote(branch) => {
            git(root, &["switch", "--quiet", "--track", branch])?;
            Ok(format!("Checked out {branch}"))
        }
        GitOp::CheckoutCommit(hash) => {
            git(root, &["switch", "--quiet", "--detach", hash])?;
            Ok(format!("Checked out {} (detached)", short(hash)))
        }
        GitOp::CreateBranchAt { name, at } => {
            git(root, &["switch", "--quiet", "--create", name.trim(), at])?;
            Ok(format!("Created and switched to {}", name.trim()))
        }
        GitOp::CreateTag { name, at } => {
            git(root, &["tag", name.trim(), at])?;
            Ok(format!("Tagged {} as {}", short(at), name.trim()))
        }
        GitOp::CherryPick(hash) => match git(root, &["cherry-pick", hash]) {
            Ok(_) => Ok(format!("Cherry-picked {}", short(hash))),
            Err(err) => Err(stopped_on_conflicts(root, "Cherry-pick", err)),
        },
        GitOp::Revert(hash) => match git(root, &["revert", "--no-edit", hash]) {
            Ok(_) => Ok(format!("Reverted {}", short(hash))),
            Err(err) => Err(stopped_on_conflicts(root, "Revert", err)),
        },
        GitOp::Push => {
            let status = git(root, &["status", "--porcelain=v2", "--branch", "-z"])?;
            let (branch, _) = parse_status(&status);
            if branch.upstream.is_some() {
                git(root, &["push", "--quiet", "--follow-tags"])?;
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

/// `git pull`, merging (or rebasing) when both sides moved.
fn pull(root: &Path, rebase: bool) -> Result<String, GitError> {
    let mode = if rebase { "--rebase" } else { "--no-rebase" };
    git(root, &["pull", "--quiet", mode]).map_err(|err| stopped_on_conflicts(root, "Pull", err))
}

/// An operation's error, said plainly when it stopped on conflicts.
fn stopped_on_conflicts(root: &Path, what: &str, err: GitError) -> GitError {
    let conflicts = git(root, &["diff", "--name-only", "--diff-filter=U"])
        .map(|out| out.lines().count())
        .unwrap_or(0);
    if conflicts > 0 {
        format!(
            "{what} stopped: {} in conflict. Resolve them, then Continue (or Abort)",
            count(conflicts, "file")
        )
    } else {
        err
    }
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(8)]
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
    /// An operation; `chained`: skipped when the operation before it failed (Commit, then
    /// Push only if the commit was made).
    Run {
        op: GitOp,
        chained: bool,
    },
    Graph {
        limit: usize,
        key: u64,
    },
    Detail(String),
    CommitDiff {
        hash: String,
        path: String,
    },
}

/// The commit graph as last read, laid out.
#[derive(Debug, Clone)]
pub struct Graph {
    /// [`RepoStatus::graph_key`] it was read for.
    pub key: u64,
    /// Most commits asked for.
    pub limit: usize,
    pub commits: Vec<GraphCommit>,
    pub rows: Vec<GraphRow>,
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
    /// The commit whose change it is (`None`: the work tree's or the index's).
    pub commit: Option<String>,
    pub text: Result<String, GitError>,
}

const COMMIT_TITLE: &str = "Commit";

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
    Graph(Result<Graph, GitError>),
    Detail(Result<CommitDetail, GitError>),
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
    pub graph: Option<Graph>,
    pub graph_error: Option<GitError>,
    /// (key, limit) of the graph asked for and not received yet.
    graph_pending: Option<(u64, usize)>,
    /// The commit shown in full.
    pub detail: Option<Result<CommitDetail, GitError>>,
    last_fetch: Option<Instant>,
    /// A background fetch is running: its success is not reported.
    quiet_fetch: bool,
    /// Commits answered so far, and whether the last one was made.
    commits_answered: u64,
    last_commit_ok: bool,
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
        Self::start_with(dir, notify, Box::new(|_, _| Ok(Vec::new())))
    }

    /// [`Self::start`], with `hook` called on the worker around every operation.
    pub fn start_with(dir: PathBuf, notify: impl Fn() + Send + 'static, hook: Hook) -> Self {
        let (requests, inbox) = mpsc::channel::<Request>();
        let (outbox, replies) = mpsc::channel::<Reply>();
        let spawned = std::thread::Builder::new()
            .name("ab-git".into())
            .spawn(move || worker(dir, inbox, outbox, notify, hook));
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
            graph: None,
            graph_error: None,
            graph_pending: None,
            detail: None,
            last_fetch: None,
            quiet_fetch: false,
            commits_answered: 0,
            last_commit_ok: false,
        };
        match spawned {
            Ok(_) => client.refresh(),
            Err(err) => client.state = RepoState::Failed(format!("cannot start git worker: {err}")),
        }
        client
    }

    fn send(&mut self, request: Request) {
        // An operation is answered twice: its result, then the status after it.
        let replies = if matches!(request, Request::Run { .. }) {
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

    /// Reads the commit graph (up to `limit` commits) when the refs changed since it was
    /// last read, or more commits are wanted.
    pub fn want_graph(&mut self, limit: usize) {
        let Some(key) = self.status().map(RepoStatus::graph_key) else {
            return;
        };
        let fresh = self
            .graph
            .as_ref()
            .is_some_and(|g| g.key == key && g.limit >= limit);
        if fresh || self.graph_pending == Some((key, limit)) {
            return;
        }
        self.graph_pending = Some((key, limit));
        self.send(Request::Graph { limit, key });
    }

    /// Reads `hash` in full ([`Self::detail`]).
    pub fn show_commit(&mut self, hash: &str) {
        if matches!(&self.detail, Some(Ok(d)) if d.hash == hash) {
            return;
        }
        self.send(Request::Detail(hash.to_owned()));
    }

    /// The diff of `path` in commit `hash` ([`Self::diff`]).
    pub fn show_commit_diff(&mut self, hash: &str, path: &str) {
        self.send(Request::CommitDiff {
            hash: hash.to_owned(),
            path: path.to_owned(),
        });
    }

    /// Fetches from the remotes when idle and the last fetch is older than `every` (so the
    /// counts of commits to pull stay current).
    pub fn fetch_every(&mut self, every: Duration) {
        let has_remote = self.status().is_some_and(|s| !s.remotes.is_empty());
        if !has_remote || self.in_flight > 0 {
            return;
        }
        if self.last_fetch.is_none_or(|t| t.elapsed() >= every) {
            self.last_fetch = Some(Instant::now());
            self.quiet_fetch = true;
            self.run(GitOp::Fetch);
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
        self.send_op(op, false);
    }

    /// Runs `op` after the operation queued before it, only if that one worked.
    pub fn run_after(&mut self, op: GitOp) {
        self.send_op(op, true);
    }

    fn send_op(&mut self, op: GitOp, chained: bool) {
        if op.is_remote() {
            self.remote_op = Some(op.title());
        }
        self.last_refresh = Some(Instant::now());
        self.send(Request::Run { op, chained });
    }

    /// Starts a commit; the ticket tells [`GitClient::commit_made`] which answer to wait for.
    pub fn commit(&mut self, op: GitOp) -> u64 {
        self.run(op);
        self.commits_answered
    }

    /// For a commit started with `ticket`: `None` while it runs, then whether it was made.
    pub fn commit_made(&self, ticket: u64) -> Option<bool> {
        (self.commits_answered > ticket).then_some(self.last_commit_ok)
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
                            let work_tree_diff =
                                self.diff.as_ref().is_some_and(|d| d.commit.is_none());
                            if let (true, Some(diff), Some(status)) =
                                (work_tree_diff, &self.diff, self.status())
                            {
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
                        Reply::Graph(result) => {
                            self.graph_pending = None;
                            match result {
                                Ok(graph) => {
                                    self.graph = Some(graph);
                                    self.graph_error = None;
                                }
                                Err(err) => self.graph_error = Some(err),
                            }
                        }
                        Reply::Detail(detail) => self.detail = Some(detail),
                        Reply::Done(result) => {
                            if self.remote_op == Some(result.title) {
                                self.remote_op = None;
                            }
                            if result.title == COMMIT_TITLE {
                                self.commits_answered += 1;
                                self.last_commit_ok = result.result.is_ok();
                            }
                            let quiet = std::mem::take(&mut self.quiet_fetch)
                                && result.title == GitOp::Fetch.title()
                                && result.result.is_ok();
                            if !quiet {
                                self.results.push(result);
                            }
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

/// When a [`Hook`] is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Before,
    After,
}

/// Called on the git worker before and after each operation. Before a commit, the paths it
/// returns are staged too (files the editor keeps for git, such as packed assets); an error
/// before an operation stops it (and is its result).
pub type Hook = Box<dyn Fn(Phase, &GitOp) -> Result<Vec<PathBuf>, GitError> + Send>;

fn worker(
    dir: PathBuf,
    inbox: Receiver<Request>,
    outbox: Sender<Reply>,
    notify: impl Fn(),
    hook: Hook,
) {
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
    // Whether the last operation failed: a chained one after it is skipped.
    let mut last_failed = false;
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
                    Reply::Diff(DiffView {
                        path,
                        staged,
                        commit: None,
                        text,
                    })
                }
                Request::Graph { limit, key } => {
                    if root.is_none() {
                        root = discover(&dir).ok().flatten();
                    }
                    Reply::Graph(match &root {
                        Some(root) => git(root, &["remote"]).and_then(|remotes| {
                            let remotes: Vec<String> = remotes.lines().map(str::to_owned).collect();
                            let commits = read_graph(root, limit, &remotes)?;
                            let rows = layout_graph(&commits);
                            Ok(Graph {
                                key,
                                limit,
                                commits,
                                rows,
                            })
                        }),
                        None => Err("not a git repository".into()),
                    })
                }
                Request::Detail(hash) => Reply::Detail(match &root {
                    Some(root) => read_commit(root, &hash),
                    None => Err("not a git repository".into()),
                }),
                Request::CommitDiff { hash, path } => {
                    let text = match &root {
                        Some(root) => commit_diff(root, &hash, &path),
                        None => Err("not a git repository".into()),
                    };
                    Reply::Diff(DiffView {
                        path,
                        staged: false,
                        commit: Some(hash),
                        text,
                    })
                }
                Request::Run { op, chained } => {
                    if root.is_none() {
                        root = discover(&dir).ok().flatten();
                    }
                    if chained && last_failed {
                        let skipped = Reply::Done(OpResult {
                            title: op.title(),
                            result: Err("not run: the step before it failed".into()),
                        });
                        let state = read(&mut root);
                        if outbox.send(skipped).is_err()
                            || outbox.send(Reply::State(state)).is_err()
                        {
                            return;
                        }
                        notify();
                        continue;
                    }
                    let staged =
                        hook(Phase::Before, &op).and_then(|extra| match (&op, root.as_deref()) {
                            (GitOp::Commit { .. }, Some(root)) if !extra.is_empty() => {
                                let paths: Vec<String> = extra
                                    .iter()
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .collect();
                                let mut args = vec!["add", "--all", "--"];
                                args.extend(paths.iter().map(String::as_str));
                                git(root, &args)
                            }
                            _ => Ok(String::new()),
                        });
                    let result = staged.and_then(|_| run(root.as_deref(), &op));
                    last_failed = result.is_err();
                    if result.is_ok() {
                        // Only an error before an operation matters: after, it is reported
                        // by the hook's owner.
                        let _ = hook(Phase::After, &op);
                    }
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

    fn commit(hash: &str, parents: &[&str]) -> GraphCommit {
        GraphCommit {
            hash: hash.into(),
            short: hash.into(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            author: String::new(),
            email: String::new(),
            time: 0,
            date: String::new(),
            refs: Vec::new(),
            subject: String::new(),
        }
    }

    #[test]
    fn a_straight_history_stays_on_one_lane() {
        let rows = layout_graph(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert!(rows.iter().all(|r| r.lane == 0 && r.width == 1));
        assert_eq!(
            rows[2].segments.len(),
            1,
            "the root only receives its child's line"
        );
    }

    #[test]
    fn a_merge_opens_a_lane_and_closes_it_at_the_fork() {
        // m merges f (feature) into b (main); both started from a.
        let rows = layout_graph(&[
            commit("m", &["b", "f"]),
            commit("f", &["a"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(
            rows[0].width, 2,
            "the merge reaches out to its second parent"
        );
        assert_eq!(rows[1].lane, 1, "the feature commit on its own lane");
        assert_eq!(rows[2].lane, 0);
        // The feature's line passes main's commit, then both meet at the fork.
        assert!(rows[2]
            .segments
            .iter()
            .any(|s| s.from == (1, Edge::Top) && s.to == (1, Edge::Bottom)));
        assert_eq!(rows[3].lane, 0);
        assert_eq!(
            rows[3]
                .segments
                .iter()
                .filter(|s| s.to == (0, Edge::Middle))
                .count(),
            2,
            "both lines join at the fork"
        );
    }

    #[test]
    fn parses_refs_stashes_and_decorations() {
        let refs = concat!(
            "refs/heads/main\u{1f}origin/main\u{1f}[ahead 2, behind 1]\n",
            "refs/heads/old\u{1f}origin/old\u{1f}[gone]\n",
            "refs/heads/feature/x\u{1f}\u{1f}\n",
            "refs/remotes/origin/HEAD\u{1f}\u{1f}\n",
            "refs/remotes/origin\u{1f}\u{1f}\n",
            "refs/remotes/origin/main\u{1f}\u{1f}\n",
            "refs/tags/v1.0\u{1f}\u{1f}\n",
        );
        let (local, remote, tags) = parse_refs(refs);
        assert_eq!(local[0].name, "feature/x");
        let main = local.iter().find(|b| b.name == "main").unwrap();
        assert_eq!((main.ahead, main.behind, main.gone), (2, 1, false));
        assert!(local.iter().find(|b| b.name == "old").unwrap().gone);
        assert_eq!(remote, ["origin/main"]);
        assert_eq!(tags, ["v1.0"]);

        let stashes = parse_stashes("stash@{0}\u{1f}WIP on main\nstash@{1}\u{1f}lights\n");
        assert_eq!(stashes[1].index, 1);
        assert_eq!(stashes[1].message, "lights");

        let remotes = vec!["origin".to_owned()];
        let labels = parse_decorations(
            "tag: v1.0, origin/main, HEAD -> main, origin/HEAD, feature/x",
            &remotes,
        );
        let kinds: Vec<(&str, RefKind)> =
            labels.iter().map(|l| (l.name.as_str(), l.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("main", RefKind::Head),
                ("feature/x", RefKind::Local),
                ("origin/main", RefKind::Remote),
                ("v1.0", RefKind::Tag),
            ]
        );
    }

    #[test]
    fn parses_commit_file_lists() {
        let names = parse_name_status("M\0a.txt\0R087\0old.png\0new.png\0A\0b.rs\0");
        assert_eq!(
            names[1],
            ("new.png".into(), Some("old.png".into()), FileState::Renamed)
        );
        assert_eq!(names[2].2, FileState::Added);
        let counts = parse_numstat("3\t1\ta.txt\0-\t-\t\0old.png\0new.png\0");
        assert_eq!(counts["a.txt"], (Some(3), Some(1)));
        assert_eq!(counts["new.png"], (None, None), "binary");
        assert_eq!(relative_time(1000, 1000 + 7200), "2 hours ago");
        assert_eq!(relative_time(0, 30), "just now");
        assert_eq!(relative_time(0, 86_400), "1 day ago");
    }

    #[test]
    fn merge_conflicts_resolve_and_continue() {
        if !git_available() {
            return;
        }
        let dir = temp_repo("merge");
        run(None, &GitOp::Init(dir.clone())).unwrap();
        configure(&dir);
        git(&dir, &["checkout", "-q", "-b", "main"]).unwrap();
        let write = |text: &str| std::fs::write(dir.join("level.txt"), text).unwrap();
        let commit_all = |message: &str| {
            run(
                Some(&dir),
                &GitOp::Commit {
                    message: message.into(),
                    amend: false,
                    stage: vec![".".into()],
                },
            )
            .unwrap();
        };
        write("start\n");
        commit_all("start");
        run(Some(&dir), &GitOp::CreateBranch("feature".into())).unwrap();
        write("feature\n");
        commit_all("feature edit");
        run(Some(&dir), &GitOp::Switch("main".into())).unwrap();
        write("main\n");
        commit_all("main edit");

        let err = run(Some(&dir), &GitOp::Merge("feature".into())).unwrap_err();
        assert!(err.contains("1 file in conflict"), "{err}");
        let status = read_status(&dir).unwrap();
        assert_eq!(status.in_progress, Some(InProgress::Merge));
        assert_eq!(status.conflicts().count(), 1);

        run(
            Some(&dir),
            &GitOp::Resolve {
                paths: vec!["level.txt".into()],
                theirs: true,
            },
        )
        .unwrap();
        run(Some(&dir), &GitOp::Continue(InProgress::Merge)).unwrap();
        let status = read_status(&dir).unwrap();
        assert_eq!(status.in_progress, None);
        assert_eq!(
            std::fs::read_to_string(dir.join("level.txt")).unwrap(),
            "feature\n"
        );

        // The graph: the merge on top with two parents, the branches labelled.
        let graph = read_graph(&dir, 50, &[]).unwrap();
        assert_eq!(graph.len(), 4);
        assert_eq!(graph[0].parents.len(), 2);
        assert_eq!(graph[0].refs[0].kind, RefKind::Head);
        assert!(graph
            .iter()
            .any(|c| c.refs.iter().any(|r| r.name == "feature")));
        let rows = layout_graph(&graph);
        assert_eq!(rows[0].width, 2);
        let detail = read_commit(&dir, &graph[0].hash).unwrap();
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].state, FileState::Modified);
        assert_eq!(
            (detail.files[0].added, detail.files[0].removed),
            (Some(1), Some(1))
        );
        let diff = commit_diff(&dir, &graph[0].hash, "level.txt").unwrap();
        assert!(
            diff.contains("-main") && diff.contains("+feature"),
            "{diff}"
        );

        // Stash round trip and a tag.
        write("work in progress\n");
        run(
            Some(&dir),
            &GitOp::StashPush {
                message: "wip".into(),
                untracked: false,
            },
        )
        .unwrap();
        assert_eq!(read_status(&dir).unwrap().stashes.len(), 1);
        run(Some(&dir), &GitOp::StashPop(0)).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("level.txt")).unwrap(),
            "work in progress\n"
        );
        run(
            Some(&dir),
            &GitOp::CreateTag {
                name: "v1".into(),
                at: graph[0].hash.clone(),
            },
        )
        .unwrap();
        assert_eq!(read_status(&dir).unwrap().tags, ["v1"]);
        let _ = std::fs::remove_dir_all(&dir);
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
    fn paths_with_pattern_characters_are_file_names() {
        if !git_available() {
            return;
        }
        let dir = temp_repo("literal");
        run(None, &GitOp::Init(dir.clone())).unwrap();
        let root = discover(&dir).unwrap().expect("a repository");
        configure(&root);
        std::fs::write(root.join("level[1].txt"), "a\n").unwrap();
        std::fs::write(root.join("level1.txt"), "b\n").unwrap();
        run(
            Some(&root),
            &GitOp::Commit {
                message: "Levels".into(),
                amend: false,
                stage: vec![
                    ".gitignore".into(),
                    "level[1].txt".into(),
                    "level1.txt".into(),
                ],
            },
        )
        .unwrap();
        std::fs::write(root.join("level[1].txt"), "changed\n").unwrap();
        std::fs::write(root.join("level1.txt"), "my work\n").unwrap();
        std::fs::write(root.join("new[1].txt"), "x\n").unwrap();
        std::fs::write(root.join("new1.txt"), "keep\n").unwrap();
        run(
            Some(&root),
            &GitOp::Discard {
                tracked: vec!["level[1].txt".into()],
                untracked: vec!["new[1].txt".into()],
            },
        )
        .unwrap();
        // As a pattern, `level[1].txt` means `level1.txt`: the wrong file was restored.
        assert_eq!(
            std::fs::read_to_string(root.join("level[1].txt")).unwrap(),
            "a\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("level1.txt")).unwrap(),
            "my work\n"
        );
        assert!(!root.join("new[1].txt").exists());
        assert!(root.join("new1.txt").exists(), "untracked work kept");
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

        // A commit that fails (nothing to commit, no message) keeps what follows from running.
        let ticket = client.commit(GitOp::Commit {
            message: String::new(),
            amend: false,
            stage: Vec::new(),
        });
        client.run_after(GitOp::Fetch);
        wait(&mut client);
        assert_eq!(client.commit_made(ticket), Some(false));
        let skipped = client.results.pop().unwrap();
        assert_eq!(skipped.title, "Fetch");
        assert!(skipped.result.unwrap_err().contains("not run"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
