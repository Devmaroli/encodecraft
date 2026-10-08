//! Spawn child processes without a console window on Windows.

use std::ffi::OsStr;
use std::process::Command;

/// `CREATE_NO_WINDOW` — hide the console that `ffmpeg.exe` / `effectcraft-cli.exe` would flash.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Build a [`Command`] that does not open a console on Windows.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    hide_window(&mut cmd);
    cmd
}

/// Apply the no-window flag to an existing command (tests / wrappers).
pub fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_points_at_program() {
        let c = command("ffmpeg");
        assert!(format!("{c:?}").to_ascii_lowercase().contains("ffmpeg"));
    }
}
