//! Running other programs (cargo, rustup, git…) from the editor.

use std::ffi::OsStr;
use std::io::{self, BufRead as _, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

/// `Command::new(program)` that opens no console window on Windows.
///
/// The editor is a windowed program there, so every console program it starts would otherwise
/// get a console window of its own, flashing open for each build, git status or check. The
/// program's output still reaches the pipes it is given.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Starts the program in a process group of its own (Unix), so [`kill_tree`] stops what it
/// starts too (cargo's rustc and build scripts).
pub fn own_group(command: &mut Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(not(unix))]
    let _ = command;
}

/// Stops `child` and every process it started, and reaps it.
pub fn kill_tree(child: &mut Child) {
    let pid = child.id().to_string();
    // The group made by `own_group` (its id is the child's).
    #[cfg(unix)]
    let _ = Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(windows)]
    let _ = command("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs `command`, handing each line of its standard error to `on_line` as it comes (cargo
/// writes its progress there), and returns how it ended. Its standard output is dropped.
pub fn run_streaming(
    command: &mut Command,
    mut on_line: impl FnMut(&str),
) -> io::Result<ExitStatus> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            on_line(&line);
        }
    }
    child.wait()
}

/// Like `Command::output`, but gives up after `timeout`: the program (and what it started)
/// is stopped and a `TimedOut` error returned.
pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    own_group(command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Both pipes drained at once: a full one would block the program.
    let drain = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    // Quick programs (git status) answer in milliseconds: look often at first.
    let mut pause = Duration::from_millis(1);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            kill_tree(&mut child);
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("no answer after {} s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(50));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_hangs_is_stopped() {
        let started = Instant::now();
        let result =
            output_with_timeout(Command::new("sleep").arg("30"), Duration::from_millis(300));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
        let done =
            output_with_timeout(Command::new("echo").arg("hi"), Duration::from_secs(10)).unwrap();
        assert_eq!(done.stdout, b"hi\n");
    }

    #[test]
    fn stderr_lines_come_as_they_are_written() {
        let mut lines = Vec::new();
        let status = run_streaming(
            Command::new("sh").args(["-c", "echo one >&2; echo two >&2"]),
            |line| lines.push(line.to_owned()),
        )
        .unwrap();
        assert!(status.success());
        assert_eq!(lines, ["one", "two"]);
    }
}
