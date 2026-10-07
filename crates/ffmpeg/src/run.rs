use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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

pub const DEFAULT_STALL_SECS: u64 = 120;

pub fn encode(
    ff: &Ffmpeg,
    source: &Path,
    output: PathBuf,
    preset: &ExportPreset,
    trim: Trim,
    media: Option<&MediaInfo>,
    stop: &EncodeStop,
    on_progress: impl FnMut(&EncodeProgress),
    on_log: impl FnMut(&str),
) -> Result<EncodeResult, FfmpegError> {
    encode_with_stall(ff, source, output, preset, trim, media, None, stop, DEFAULT_STALL_SECS, on_progress, on_log)
}

pub fn encode_with_stall(
    ff: &Ffmpeg,
    source: &Path,
    output: PathBuf,
    preset: &ExportPreset,
    trim: Trim,
    media: Option<&MediaInfo>,
    sequence_start: Option<u32>,
    stop: &EncodeStop,
    stall_secs: u64,
    mut on_progress: impl FnMut(&EncodeProgress),
    mut on_log: impl FnMut(&str),
) -> Result<EncodeResult, FfmpegError> {
    if !source.is_file() {
        return Err(FfmpegError::Message(format!("source is missing: {}", source.display())));
    }
    let final_out = output;
    let tmp_out = partial_output(&final_out);
    if let Some(parent) = tmp_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let plan = build_encode_plan(PlanInput { source, output: tmp_out.clone(), preset, trim, media, sequence_start });
    on_log(&plan.display_cmdline(&ff.ffmpeg));

    let duration = trim.duration_sec(media.and_then(|m| m.duration_sec)).or_else(|| media.and_then(|m| m.duration_sec));

    let mut child = ChildGuard {
        child: Some(Command::new(&ff.ffmpeg).args(&plan.args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FfmpegError::Missing
            } else {
                FfmpegError::Io(e)
            }
        })?),
    };

    let stdout = child.stdout();
    let stderr = child.stderr();
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
    let mut stalled = false;
    let stall_for = Duration::from_secs(stall_secs.max(15));
    let mut last_activity = Instant::now();
    loop {
        if stop.is_requested() {
            cancelled = true;
            child.kill();
            break;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Msg::Out(line)) => {
                last_activity = Instant::now();
                if parse_progress_line(&line, &mut progress, duration) {
                    on_progress(&progress);
                }
            }
            Ok(Msg::Err(line)) => {
                last_activity = Instant::now();
                if !line.is_empty() {
                    log.push_str(&line);
                    log.push('\n');
                    on_log(&line);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if last_activity.elapsed() > stall_for {
                    stalled = true;
                    child.kill();
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    if cancelled || stop.is_requested() {
        let _ = remove_partial(&tmp_out);
        return Err(FfmpegError::Message("stopped".into()));
    }
    if stalled {
        let _ = remove_partial(&tmp_out);
        return Err(FfmpegError::Message(format!("ffmpeg stalled (no output for {}s)", stall_for.as_secs())));
    }
    if !status.success() {
        let _ = remove_partial(&tmp_out);
        let code = status.code().unwrap_or(-1);
        let err = tail(&log, 4000);
        if looks_like_disk_full(&err) {
            return Err(FfmpegError::Message("disk full".into()));
        }
        return Err(FfmpegError::Encode { status: code, stderr: err });
    }
    if let Some(parent) = final_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    replace_file(&tmp_out, &final_out)?;
    progress.done = true;
    progress.percent = 100.0;
    on_progress(&progress);
    Ok(EncodeResult { output: final_out, log, progress })
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.as_mut().and_then(|c| c.stdout.take())
    }
    fn stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.as_mut().and_then(|c| c.stderr.take())
    }
    fn kill(&mut self) {
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
        }
    }
    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        match self.child.as_mut() {
            Some(c) => {
                let st = c.wait()?;
                self.child = None;
                Ok(st)
            }
            None => Err(std::io::Error::new(std::io::ErrorKind::Other, "no child")),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub fn partial_output(final_out: &Path) -> PathBuf {
    let name = final_out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    if name.contains('%') {
        let seq_dir = final_out.parent().unwrap_or(Path::new("."));
        let parent = seq_dir.parent().unwrap_or(Path::new("."));
        let stem = seq_dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "seq".into());
        return parent.join(format!(".{stem}.encodecraft-partial")).join(name);
    }
    // Keep a real media extension so FFmpeg can pick a muxer.
    let stem = final_out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let fname = match final_out.extension() {
        Some(ext) => format!(".{stem}.encodecraft-partial.{}", ext.to_string_lossy()),
        None => format!(".{stem}.encodecraft-partial"),
    };
    match final_out.parent() {
        Some(dir) => dir.join(fname),
        None => PathBuf::from(fname),
    }
}

fn remove_partial(tmp: &Path) -> std::io::Result<()> {
    if tmp.to_string_lossy().contains('%') {
        if let Some(dir) = tmp.parent() {
            return std::fs::remove_dir_all(dir);
        }
    }
    if tmp.is_dir() {
        std::fs::remove_dir_all(tmp)
    } else {
        std::fs::remove_file(tmp)
    }
}

fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = std::fs::remove_file(to);
            if from.is_dir() {
                let _ = std::fs::remove_dir_all(to);
            }
            std::fs::rename(from, to)
        }
    }
}

fn looks_like_disk_full(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("no space left") || s.contains("disk full") || s.contains("not enough space")
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
    let out = cmd
        .args(["-i"])
        .arg(super::command::argv_path(source))
        .args(["-frames:v", "1", "-vf", "scale=320:-2", "-y"])
        .arg(super::command::argv_path(dest))
        .stdin(Stdio::null())
        .output()?;
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

    #[test]
    fn partial_output_is_beside_final() {
        let p = PathBuf::from("/out/clip.mp4");
        let t = partial_output(&p);
        let name = t.file_name().unwrap().to_string_lossy();
        assert!(name.contains("encodecraft-partial"));
        assert!(name.ends_with(".mp4"), "{name}");
        assert_eq!(t.parent(), p.parent());
        assert_ne!(t, p);
    }

    #[test]
    fn disk_full_sniff() {
        assert!(looks_like_disk_full("os error 28: No space left on device"));
        assert!(!looks_like_disk_full("invalid argument"));
    }
}
