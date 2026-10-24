//! Building a project's script library in the background.

use crate::{process, Profile, Project};
use anyhow::{Context as _, Result};
use std::io::{BufRead as _, BufReader};
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// Error lines kept from a failed build.
const MAX_ERROR_LINES: usize = 40;

/// State of a [`ScriptBuild`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildState {
    Running,
    Succeeded,
    /// The compiler's errors.
    Failed(String),
}

/// `cargo build --lib` of a project's scripts, running in the background.
///
/// The library uses the script ABI, so any Rust toolchain and any profile work: the editor
/// builds the debug profile for fast rebuilds.
#[derive(Debug)]
pub struct ScriptBuild {
    child: Child,
    lines: Receiver<String>,
    /// Latest progress line ("Compiling serde").
    pub progress: String,
    /// Crates compiled so far.
    pub compiled: usize,
    errors: Vec<String>,
    pub state: BuildState,
}

impl ScriptBuild {
    pub fn start(project: &Project, profile: Profile) -> Result<Self> {
        if project.package.is_none() {
            anyhow::bail!("the project has no Cargo.toml");
        }
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut command = process::command(cargo);
        command
            .args(["build", "--lib", "--color", "never"])
            .current_dir(&project.root)
            // A cargo running us (`cargo run -p ab-editor`) must not lend its package to
            // the project's own build.
            .env_remove("CARGO_MANIFEST_DIR")
            .env_remove("CARGO_PRIMARY_PACKAGE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if profile == Profile::Release {
            command.arg("--release");
        }
        // Cancel stops rustc and the build scripts too, not cargo alone.
        process::own_group(&mut command);
        let mut child = command
            .spawn()
            .context("cannot run cargo: is Rust installed (https://rustup.rs)?")?;
        let stderr = child.stderr.take().context("no cargo output")?;
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            lines,
            progress: "Starting cargo".into(),
            compiled: 0,
            errors: Vec::new(),
            state: BuildState::Running,
        })
    }

    fn read_line(&mut self, line: String) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Compiling ") {
            self.compiled += 1;
            let krate = rest.split_whitespace().next().unwrap_or(rest);
            self.progress = format!("Compiling {krate}");
        } else if (trimmed.starts_with("error") || !self.errors.is_empty())
            && self.errors.len() < MAX_ERROR_LINES
        {
            self.errors.push(line);
        }
    }

    /// Reads new output; returns the state.
    pub fn poll(&mut self) -> &BuildState {
        if self.state != BuildState::Running {
            return &self.state;
        }
        while let Ok(line) = self.lines.try_recv() {
            self.read_line(line);
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            // Output written just before exiting.
            while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(50)) {
                self.read_line(line);
            }
            self.state = if status.success() {
                BuildState::Succeeded
            } else if self.errors.is_empty() {
                BuildState::Failed(format!("cargo failed ({status})"))
            } else {
                BuildState::Failed(self.errors.join("\n"))
            };
        }
        &self.state
    }

    /// Waits for the build to end (tools, tests).
    pub fn wait(&mut self) -> &BuildState {
        while *self.poll() == BuildState::Running {
            std::thread::sleep(Duration::from_millis(100));
        }
        &self.state
    }

    pub fn cancel(&mut self) {
        process::kill_tree(&mut self.child);
    }
}

impl Drop for ScriptBuild {
    fn drop(&mut self) {
        if self.state == BuildState::Running {
            self.cancel();
        }
    }
}
