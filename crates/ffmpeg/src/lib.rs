//! FFmpeg as an **external binary** (never linked). EffectCraft encodes in pure Rust; EncodeCraft
//! shells out to FFmpeg because that is the right tool for a Media Encoder-style delivery queue.
//!
//! Discovery looks at `FFMPEG_PATH`, `PATH`, and common Windows/macOS/Linux install locations.
//! If nothing is found, callers get a [`FfmpegError::Missing`] with install hints — the UI shows
//! that instead of crashing.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod command;
mod detect;
mod probe;
mod progress;
mod run;

pub use command::{build_encode_plan, even, EncodePlan, PlanInput};
pub use detect::{find_effectcraft_cli, find_ffmpeg, find_ffmpeg_at, Ffmpeg};
pub use probe::{format_duration, probe, AudioStream, MediaInfo, VideoStream};
pub use progress::{parse_progress_line, EncodeProgress};
pub use run::{encode, encode_path, thumbnail_png, EncodeResult, EncodeStop};

use encodecraft_job::JobSource;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum FfmpegError {
    #[error("FFmpeg was not found. Install it and make sure `ffmpeg` is on PATH, or set the FFMPEG_PATH environment variable.\n\nWindows: winget install Gyan.FFmpeg  (or download a build from https://www.gyan.dev/ffmpeg/builds/ and add the bin folder to PATH)\nmacOS: brew install ffmpeg\nLinux: sudo apt install ffmpeg   or   sudo dnf install ffmpeg")]
    Missing,
    #[error("effectcraft-cli was not found (needed to render a composition before encoding). Set EFFECTCRAFT_CLI or put effectcraft-cli on PATH.")]
    EffectCraftMissing,
    #[error("ffprobe failed: {0}")]
    Probe(String),
    #[error("ffmpeg failed ({status}): {stderr}")]
    Encode { status: i32, stderr: String },
    #[error("{0}")]
    Message(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// How EncodeCraft turns an EffectCraft composition into pixels: render a ProRes (or PNG) mezzanine
/// with `effectcraft-cli`, then encode that file. Frame streaming was considered and rejected —
/// EffectCraft already has a renderer, and a raw-frame socket would couple the two apps.
pub fn effectcraft_render_args(cli: &std::path::Path, source: &JobSource, mezzanine_out: &std::path::Path) -> Result<Vec<String>, FfmpegError> {
    match source {
        JobSource::EffectCraft { project, comp, mezzanine, work_area } => {
            let mut args = vec![
                cli.to_string_lossy().into_owned(),
                "render".into(),
                "--project".into(),
                project.to_string_lossy().into_owned(),
                "--out".into(),
                mezzanine_out.to_string_lossy().into_owned(),
                "--format".into(),
                mezzanine.clone(),
            ];
            if let Some(c) = comp {
                args.push("--comp".into());
                args.push(c.clone());
            }
            if *work_area {
                args.push("--work-area".into());
            }
            Ok(args)
        }
        JobSource::File { .. } => Err(FfmpegError::Message("source is already a file".into())),
    }
}

pub fn mezzanine_path(work_dir: &std::path::Path, job_id: &str, mezzanine: &str) -> PathBuf {
    let ext = match mezzanine {
        "png" | "jpeg" | "tiff" | "exr" => "png",
        "webm" => "webm",
        "gif" => "gif",
        "wav" | "aiff" => "wav",
        _ => "mov",
    };
    work_dir.join(format!("{job_id}-mezzanine.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn effectcraft_argv_includes_project_and_comp() {
        let src = JobSource::EffectCraft { project: PathBuf::from("/p/main.ecproj"), comp: Some("Main".into()), mezzanine: "prores".into(), work_area: true };
        let args = effectcraft_render_args(std::path::Path::new("/bin/effectcraft-cli"), &src, std::path::Path::new("/tmp/m.mov")).unwrap();
        assert!(args.windows(2).any(|w| w[0] == "--project" && w[1].ends_with("main.ecproj")));
        assert!(args.windows(2).any(|w| w[0] == "--comp" && w[1] == "Main"));
        assert!(args.iter().any(|a| a == "--work-area"));
        assert!(args.windows(2).any(|w| w[0] == "--format" && w[1] == "prores"));
    }
}
