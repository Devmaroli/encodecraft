use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use super::command::{build_encode_plan, PlanInput};
use super::probe::{self, MediaInfo};
use super::progress::{parse_progress_line, EncodeProgress};
use super::{Ffmpeg, FfmpegError};
use encodecraft_job::Trim;
use encodecraft_presets::ExportPreset;

/// Cooperative cancel flag shared with the UI / queue.
#[derive(Clone, Debug, Default)]
pub struct EncodeStop {
    inner: Arc<AtomicBool>,
}

impl EncodeStop {
    pub fn new() -> Self {
        Self { inner: Arc::new(AtomicBool::new(false)) }
    }
    pub fn request(&self) {
        self.inner.store(true, Ordering::SeqCst);
    }
    pub fn is_requested(&self) -> bool {
        self.inner.load(Ordering::SeqCst)
    }
}

pub struct EncodeResult {
    pub output: PathBuf,
    pub log: String,
    pub progress: EncodeProgress,
}

pub fn encode(
    ff: &Ffmpeg,
    source: &Path,
    output: PathBuf,
    preset: &ExportPreset,
    trim: Trim,
    media: Option<&MediaInfo>,
    stop: &EncodeStop,
    mut on_progress: impl FnMut(&EncodeProgress),
    mut on_log: impl FnMut(&str),
) -> Result<EncodeResult, FfmpegError> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let plan = build_encode_plan(PlanInput { source, output: output.clone(), preset, trim, media });
    on_log(&plan.display_cmdline(&ff.ffmpeg));

    let duration = trim.duration_sec(media.and_then(|m| m.duration_sec)).or_else(|| media.and_then(|m| m.duration_sec));

    let mut child = Command::new(&ff.ffmpeg).args(&plan.args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FfmpegError::Missing
        } else {
            FfmpegError::Io(e)
        }
    })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();

    if let Some(out) = stdout {
        let tx = tx.clone();
        thread::spawn(move || {
            let reader = BufReader::new(out);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(Msg::Out(l)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
    if let Some(err) = stderr {
        let tx = tx.clone();
        thread::spawn(move || {
            let reader = BufReader::new(err);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(Msg::Err(l)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
    drop(tx);

    let mut progress = EncodeProgress::default();
    let mut log = String::new();
    let mut cancelled = false;
    loop {
        if stop.is_requested() {
            cancelled = true;
            let _ = child.kill();
            break;
        }
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(Msg::Out(line)) => {
                if parse_progress_line(&line, &mut progress, duration) {
                    on_progress(&progress);
                }
            }
            Ok(Msg::Err(line)) => {
                if !line.is_empty() {
                    log.push_str(&line);
                    log.push('\n');
                    on_log(&line);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    if cancelled || stop.is_requested() {
        return Err(FfmpegError::Message("stopped".into()));
    }
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(FfmpegError::Encode { status: code, stderr: tail(&log, 4000) });
    }
    progress.done = true;
    progress.percent = 100.0;
    on_progress(&progress);
    Ok(EncodeResult { output, log, progress })
}

enum Msg {
    Out(String),
    Err(String),
}

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    s[s.len() - max..].to_string()
}

/// Grab a JPEG/PNG thumbnail of a source for the preview pane.
pub fn thumbnail_png(ff: &Ffmpeg, source: &Path, dest: &Path, at_sec: f64) -> Result<(), FfmpegError> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut cmd = Command::new(&ff.ffmpeg);
    if at_sec > 0.05 {
        cmd.args(["-ss", &format!("{at_sec:.3}")]);
    }
    let out = cmd.args(["-i"]).arg(source).args(["-frames:v", "1", "-vf", "scale=320:-2", "-y"]).arg(dest).output()?;
    if !out.status.success() {
        return Err(FfmpegError::Probe(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    Ok(())
}

/// Convenience: probe then encode.
pub fn encode_path(
    ff: &Ffmpeg,
    source: &Path,
    output: PathBuf,
    preset: &ExportPreset,
    trim: Trim,
    stop: &EncodeStop,
    on_progress: impl FnMut(&EncodeProgress),
    on_log: impl FnMut(&str),
) -> Result<EncodeResult, FfmpegError> {
    let media = probe::probe(ff, source).ok();
    encode(ff, source, output, preset, trim, media.as_ref(), stop, on_progress, on_log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_flag_is_shared() {
        let s = EncodeStop::new();
        let c = s.clone();
        assert!(!s.is_requested());
        c.request();
        assert!(s.is_requested());
    }
}
