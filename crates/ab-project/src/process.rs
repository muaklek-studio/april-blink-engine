//! Running other programs (cargo, rustup, git…) from the editor.

use std::ffi::OsStr;
use std::process::Command;

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
