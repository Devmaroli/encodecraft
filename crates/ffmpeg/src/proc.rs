//! Spawn child processes without a console window on Windows.
//!
//! Production code must not call [`Command::new`] except here. EncodeCraft, FFmpeg, ffprobe,
//! `effectcraft-cli`, `reg`, and the “open in Explorer” helpers all go through [`command`].

use std::ffi::OsStr;
use std::process::Command;

/// `CREATE_NO_WINDOW` (0x08000000) — hide the console that `ffmpeg.exe` / `effectcraft-cli.exe` would flash.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

    #[test]
    fn create_no_window_flag_is_documented() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
        let src = include_str!("proc.rs");
        assert!(src.contains("creation_flags"));
        assert!(src.contains("CommandExt") || !cfg!(windows));
        assert!(src.contains("0x0800_0000"));
    }

    #[cfg(windows)]
    #[test]
    fn hidden_command_captures_output_with_create_no_window() {
        let out = command("cmd.exe").args(["/C", "echo hidden-ok"]).output().expect("spawn cmd");
        assert!(out.status.success(), "status {:?}", out.status);
        let stdout = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        assert!(stdout.contains("hidden-ok"), "{stdout:?}");
    }
}
