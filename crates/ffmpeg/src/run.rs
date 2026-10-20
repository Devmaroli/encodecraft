use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};

#[cfg(test)]
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::command::{build_encode_plan, build_encode_plans, h264_unencodable_message, inject_hrd_cbr, PlanInput};
use super::probe::{self, AudioStream, MediaInfo, VideoStream};
use super::progress::{parse_progress_line, EncodeProgress};
use super::stream::{RawVideoInput, StreamHeader};
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
    /// ffprobe average video bitrate of the finished file.
    pub measured_video_kbps: Option<f32>,
    /// True when a min bitrate was set and the finished file still sits under it.
    pub min_bitrate_warning: bool,
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
    let w = preset.video.width.or_else(|| media.map(|m| m.width())).unwrap_or(0);
    let h = preset.video.height.or_else(|| media.map(|m| m.height())).unwrap_or(0);
    if let Some(msg) = h264_unencodable_message(preset, w, h) {
        return Err(FfmpegError::Message(msg));
    }

    let plans = build_encode_plans(PlanInput { source, output: tmp_out.clone(), preset, trim, media, sequence_start, raw: None, audio: None });
    let n_passes = plans.len().max(1);
    let duration = trim.duration_sec(media.and_then(|m| m.duration_sec)).or_else(|| media.and_then(|m| m.duration_sec));
    let mut log = String::new();
    let mut progress = EncodeProgress::default();
    let stats = plans.iter().find_map(|p| p.passlogfile.clone());

    for (i, plan) in plans.iter().enumerate() {
        on_log(&plan.display_cmdline(&ff.ffmpeg));
        progress.percent = 0.0;
        progress.pass_percent = 0.0;
        progress.out_time_sec = 0.0;
        progress.done = false;
        progress.frame = 0;
        if n_passes > 1 {
            progress.stage = format!("Encoding pass {}/{}", i + 1, n_passes);
            on_log(&format!("pass {}/{}", i + 1, n_passes));
        } else if preset.video.use_nvenc {
            progress.stage = "Encoding GPU".into();
        } else {
            progress.stage = "Encoding".into();
        }
        on_progress(&scale_progress(&progress, i, n_passes));
        match run_one_plan(ff, plan, duration, stall_secs, stop, &mut progress, &mut log, &mut on_progress, &mut on_log, i, n_passes) {
            Ok(()) => {}
            Err(e) => {
                let _ = remove_partial(&tmp_out);
                cleanup_passlog(stats.as_deref());
                return Err(e);
            }
        }
    }

    if let Some(parent) = final_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    replace_file(&tmp_out, &final_out)?;
    cleanup_passlog(stats.as_deref());

    let (measured_video_kbps, min_bitrate_warning, extra) = enforce_min_bitrate(
        ff,
        source,
        &final_out,
        preset,
        trim,
        media,
        sequence_start,
        None,
        None,
        stall_secs,
        stop,
        &mut progress,
        &mut on_progress,
        &mut on_log,
    )?;
    log.push_str(&extra);

    progress.done = true;
    progress.percent = 100.0;
    on_progress(&progress);
    Ok(EncodeResult { output: final_out, log, progress, measured_video_kbps, min_bitrate_warning })
}

#[allow(clippy::too_many_arguments)]
fn enforce_min_bitrate(
    ff: &Ffmpeg,
    source: &Path,
    final_out: &Path,
    preset: &ExportPreset,
    trim: Trim,
    media: Option<&MediaInfo>,
    sequence_start: Option<u32>,
    raw: Option<RawVideoInput>,
    audio: Option<&Path>,
    stall_secs: u64,
    stop: &EncodeStop,
    progress: &mut EncodeProgress,
    on_progress: &mut impl FnMut(&EncodeProgress),
    on_log: &mut impl FnMut(&str),
) -> Result<(Option<f32>, bool, String), FfmpegError> {
    let mut extra = String::new();
    let info = probe::probe(ff, final_out).ok();
    let mut measured = info.as_ref().and_then(MediaInfo::video_average_kbps);
    let Some(min_k) = preset.video.quality.effective_min_kbps() else {
        return Ok((measured, false, extra));
    };
    let min_f = min_k as f32;
    if measured.unwrap_or(0.0) + 20.0 >= min_f {
        return Ok((measured, false, extra));
    }
    let msg =
        format!("average {:.2} Mbps is below min {:.2} Mbps — filler pass (nal-hrd cbr, picture unchanged)", measured.unwrap_or(0.0) / 1000.0, min_f / 1000.0);
    extra.push_str(&msg);
    extra.push('\n');
    on_log(&msg);

    let mut floor = preset.clone();
    if let encodecraft_presets::Quality::Vbr2Pass { max_mbps, .. } = floor.video.quality {
        let min_m = floor.video.quality.effective_min_mbps().unwrap_or(min_f as f64 / 1000.0);
        // x264 HRD on short clips often lands ~10% under -b:v. Aim a little over the
        // min (capped at max) so the finished-file average still clears the floor.
        let cbr = (min_m / 0.90).min(max_mbps.max(min_m));
        floor.video.quality = encodecraft_presets::Quality::Vbr2Pass { target_mbps: cbr, max_mbps: cbr, min_mbps: Some(cbr), two_pass: false };
    }
    let tmp = partial_output(final_out);
    let mut plan = build_encode_plan(PlanInput { source, output: tmp.clone(), preset: &floor, trim, media, sequence_start, raw, audio });
    if !floor.video.use_nvenc {
        inject_hrd_cbr(&mut plan.args);
    }
    on_log(&plan.display_cmdline(&ff.ffmpeg));
    let duration = trim.duration_sec(media.and_then(|m| m.duration_sec)).or_else(|| media.and_then(|m| m.duration_sec));
    match run_one_plan(ff, &plan, duration, stall_secs, stop, progress, &mut extra, on_progress, on_log, 0, 1) {
        Ok(()) => {
            replace_file(&tmp, final_out)?;
        }
        Err(e) => {
            let _ = remove_partial(&tmp);
            extra.push_str(&format!("filler pass failed ({e}); keeping VBR encode\n"));
            on_log("filler pass failed; keeping VBR encode");
            let warn = measured.map(|a| a + 20.0 < min_f).unwrap_or(true);
            return Ok((measured, warn, extra));
        }
    }
    let info2 = probe::probe(ff, final_out).ok();
    measured = info2.as_ref().and_then(MediaInfo::video_average_kbps).or(measured);
    let warn = measured.map(|a| a + 20.0 < min_f).unwrap_or(false);
    Ok((measured, warn, extra))
}

#[allow(clippy::too_many_arguments)]
fn run_one_plan(
    ff: &Ffmpeg,
    plan: &super::command::EncodePlan,
    duration: Option<f64>,
    stall_secs: u64,
    stop: &EncodeStop,
    progress: &mut EncodeProgress,
    log: &mut String,
    on_progress: &mut impl FnMut(&EncodeProgress),
    on_log: &mut impl FnMut(&str),
    pass_i: usize,
    n_passes: usize,
) -> Result<(), FfmpegError> {
    let mut child = ChildGuard {
        child: Some(super::proc::command(&ff.ffmpeg).args(&plan.args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(
            |e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    FfmpegError::Missing
                } else {
                    FfmpegError::Io(e)
                }
            },
        )?),
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
                if parse_progress_line(&line, progress, duration) {
                    on_progress(&scale_progress(progress, pass_i, n_passes));
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
        return Err(FfmpegError::Message("stopped".into()));
    }
    if stalled {
        return Err(FfmpegError::Message(format!("ffmpeg stalled (no output for {}s)", stall_for.as_secs())));
    }
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        let err = tail(log, 4000);
        if looks_like_disk_full(&err) {
            return Err(FfmpegError::Message("disk full".into()));
        }
        return Err(FfmpegError::Encode { status: code, stderr: humanize_h264_error(&err, plan) });
    }
    Ok(())
}

fn scale_progress(p: &EncodeProgress, pass_i: usize, n_passes: usize) -> EncodeProgress {
    let mut q = p.clone();
    q.pass_index = pass_i;
    q.pass_count = n_passes.max(1);
    q.pass_percent = p.percent;
    if n_passes <= 1 {
        return q;
    }
    let span = 100.0 / n_passes as f32;
    q.done = false;
    q.percent = (pass_i as f32).mul_add(span, (p.percent / 100.0) * span);
    q
}

fn cleanup_passlog(prefix: Option<&Path>) {
    let Some(prefix) = prefix else {
        return;
    };
    let name = prefix.to_string_lossy();
    for extra in ["-0.log", "-0.log.mbtree", ".log", ".log.mbtree"] {
        let _ = std::fs::remove_file(format!("{name}{extra}"));
    }
}

fn humanize_h264_error(stderr: &str, plan: &super::command::EncodePlan) -> String {
    let s = stderr.to_ascii_lowercase();
    let looks = s.contains("mb size")
        || (s.contains("frame") && s.contains("too large"))
        || s.contains("invalid level")
        || s.contains("picture is too large")
        || s.contains("width not divisible")
        || s.contains("level restriction");
    if looks {
        format!("H.264 cannot encode this frame size with libx264 (output {}). FFmpeg said: {}", plan.output.display(), tail(stderr, 1500))
    } else {
        stderr.to_string()
    }
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
            None => Err(std::io::Error::other("no child")),
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
    let mut cmd = super::proc::command(&ff.ffmpeg);
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

pub fn media_from_header(header: &StreamHeader, path: &Path) -> MediaInfo {
    MediaInfo {
        path: path.to_path_buf(),
        duration_sec: header.duration_sec(),
        format_name: "rawvideo".into(),
        bit_rate_bps: None,
        size_bytes: None,
        video: Some(VideoStream {
            width: header.width,
            height: header.height,
            fps: Some(header.ffmpeg_fps()),
            codec: "rawvideo".into(),
            pix_fmt: header.pix_fmt().into(),
            bit_rate_bps: None,
        }),
        audio: if header.audio {
            Some(AudioStream { codec: "pcm_s16le".into(), sample_rate: header.sample_rate, channels: header.audio_channels, bit_rate_bps: None })
        } else {
            None
        },
    }
}

/// FFmpeg process whose stdin receives raw frames (x264 pass 1, or a 1-pass encode).
pub struct SpawnedPlan {
    child: ChildGuard,
    rx: std::sync::mpsc::Receiver<Msg>,
    pub log: String,
    pub progress: EncodeProgress,
    pass_i: usize,
    n_passes: usize,
    duration: Option<f64>,
    stall_secs: u64,
}

impl SpawnedPlan {
    pub fn stdin(&mut self) -> Option<ChildStdin> {
        self.child.child.as_mut().and_then(|c| c.stdin.take())
    }
}

pub fn spawn_plan(
    ff: &Ffmpeg,
    plan: &super::command::EncodePlan,
    duration: Option<f64>,
    stall_secs: u64,
    pass_i: usize,
    n_passes: usize,
) -> Result<SpawnedPlan, FfmpegError> {
    let child = super::proc::command(&ff.ffmpeg).args(&plan.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FfmpegError::Missing
        } else {
            FfmpegError::Io(e)
        }
    })?;
    let mut guard = ChildGuard { child: Some(child) };
    let stdout = guard.stdout();
    let stderr = guard.stderr();
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    if let Some(out) = stdout {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(out).lines() {
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
            for line in BufReader::new(err).lines() {
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
    Ok(SpawnedPlan {
        child: guard,
        rx,
        log: String::new(),
        progress: EncodeProgress {
            stage: if n_passes > 1 { format!("Encoding pass {}/{}", pass_i + 1, n_passes) } else { "Encoding".into() },
            ..EncodeProgress::default()
        },
        pass_i,
        n_passes,
        duration,
        stall_secs,
    })
}

pub fn feed_pass_stdin(stdin: &mut ChildStdin, frame: &[u8]) -> Result<(), FfmpegError> {
    stdin.write_all(frame).map_err(FfmpegError::Io)?;
    Ok(())
}

pub fn wait_spawned_plan(
    spawned: &mut SpawnedPlan,
    stop: &EncodeStop,
    mut on_progress: impl FnMut(&EncodeProgress),
    mut on_log: impl FnMut(&str),
) -> Result<(), FfmpegError> {
    let mut cancelled = false;
    let mut stalled = false;
    let stall_for = Duration::from_secs(spawned.stall_secs.max(15));
    let mut last_activity = Instant::now();
    loop {
        if stop.is_requested() {
            cancelled = true;
            spawned.child.kill();
            break;
        }
        match spawned.rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Msg::Out(line)) => {
                last_activity = Instant::now();
                if parse_progress_line(&line, &mut spawned.progress, spawned.duration) {
                    on_progress(&scale_progress(&spawned.progress, spawned.pass_i, spawned.n_passes));
                }
            }
            Ok(Msg::Err(line)) => {
                last_activity = Instant::now();
                if !line.is_empty() {
                    spawned.log.push_str(&line);
                    spawned.log.push('\n');
                    on_log(&line);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if last_activity.elapsed() > stall_for {
                    stalled = true;
                    spawned.child.kill();
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = spawned.child.wait()?;
    if cancelled || stop.is_requested() {
        return Err(FfmpegError::Message("stopped".into()));
    }
    if stalled {
        return Err(FfmpegError::Message(format!("ffmpeg stalled (no output for {}s)", stall_for.as_secs())));
    }
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(FfmpegError::Encode { status: code, stderr: tail(&spawned.log, 4000) });
    }
    Ok(())
}

/// Pass 2 (and 1-pass file encodes) from a finished raw yuv file + optional WAV.
pub fn encode_raw_from_file(
    ff: &Ffmpeg,
    yuv: &Path,
    output: PathBuf,
    preset: &ExportPreset,
    header: &StreamHeader,
    audio: Option<&Path>,
    stdin_pass: bool,
    stop: &EncodeStop,
    stall_secs: u64,
    mut on_progress: impl FnMut(&EncodeProgress),
    mut on_log: impl FnMut(&str),
) -> Result<EncodeResult, FfmpegError> {
    if !yuv.is_file() {
        return Err(FfmpegError::Message(format!("raw source is missing: {}", yuv.display())));
    }
    let final_out = output;
    let tmp_out = partial_output(&final_out);
    if let Some(parent) = tmp_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let media = media_from_header(header, yuv);
    let raw = RawVideoInput::from_header(header, stdin_pass);
    let w = preset.video.width.filter(|n| *n > 0).unwrap_or(header.width);
    let h = preset.video.height.filter(|n| *n > 0).unwrap_or(header.height);
    if let Some(msg) = h264_unencodable_message(preset, w, h) {
        return Err(FfmpegError::Message(msg));
    }
    let plans = build_encode_plans(PlanInput {
        source: yuv,
        output: tmp_out.clone(),
        preset,
        trim: Trim::default(),
        media: Some(&media),
        sequence_start: None,
        raw: Some(raw),
        audio,
    });
    let n_passes = plans.len().max(1);
    let duration = header.duration_sec();
    let mut log = String::new();
    let mut progress = EncodeProgress::default();
    let stats = plans.iter().find_map(|p| p.passlogfile.clone());
    for (i, plan) in plans.iter().enumerate() {
        if stdin_pass && plan.stdin && i == 0 {
            continue;
        }
        on_log(&plan.display_cmdline(&ff.ffmpeg));
        progress.stage = if n_passes > 1 { format!("Encoding pass {}/{}", i + 1, n_passes) } else { "Encoding".into() };
        match run_one_plan(ff, plan, duration, stall_secs, stop, &mut progress, &mut log, &mut on_progress, &mut on_log, i, n_passes) {
            Ok(()) => {}
            Err(e) => {
                let _ = remove_partial(&tmp_out);
                cleanup_passlog(stats.as_deref());
                return Err(e);
            }
        }
    }
    if let Some(parent) = final_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    replace_file(&tmp_out, &final_out)?;
    cleanup_passlog(stats.as_deref());
    let filler_raw = Some(RawVideoInput::from_header(header, false));
    let (measured_video_kbps, min_bitrate_warning, extra) = enforce_min_bitrate(
        ff,
        yuv,
        &final_out,
        preset,
        Trim::default(),
        Some(&media),
        None,
        filler_raw,
        audio,
        stall_secs,
        stop,
        &mut progress,
        &mut on_progress,
        &mut on_log,
    )?;
    log.push_str(&extra);
    progress.done = true;
    progress.percent = 100.0;
    on_progress(&progress);
    Ok(EncodeResult { output: final_out, log, progress, measured_video_kbps, min_bitrate_warning })
}

/// First 2-pass plan with stdin rawvideo, or the only plan for 1-pass.
pub fn first_stdin_plan(plans: &[super::command::EncodePlan]) -> Option<(usize, &super::command::EncodePlan)> {
    plans.iter().enumerate().find(|(_, p)| p.stdin)
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

    #[test]
    fn scale_progress_keeps_pass_local_percent() {
        let mut p = EncodeProgress { percent: 50.0, stage: "Encoding pass 1/2".into(), ..EncodeProgress::default() };
        let q = scale_progress(&p, 0, 2);
        assert_eq!(q.pass_index, 0);
        assert_eq!(q.pass_count, 2);
        assert!((q.pass_percent - 50.0).abs() < 0.01);
        assert!((q.percent - 25.0).abs() < 0.01);
        p.percent = 0.0;
        p.stage = "Encoding pass 2/2".into();
        let q2 = scale_progress(&p, 1, 2);
        assert_eq!(q2.pass_index, 1);
        assert!(q2.percent >= 49.0 && q2.percent < 51.0, "{}", q2.percent);
    }

    #[test]
    fn ffmpeg_encodes_tall_h264_level62() {
        let Ok(ff) = crate::find_ffmpeg() else {
            eprintln!("skip: ffmpeg not on PATH");
            return;
        };
        let dir = std::env::temp_dir().join(format!("ec-h264-{}-{}", std::process::id(), unix_nanos()));
        let _ = std::fs::create_dir_all(&dir);
        for (w, h, name) in [(1008u32, 5184u32, "ring"), (960u32, 5328u32, "nassar")] {
            let src = dir.join(format!("{name}-src.mp4"));
            let out = dir.join(format!("{name}-out.mp4"));
            let status = Command::new(&ff.ffmpeg)
                .args([
                    "-hide_banner",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("testsrc2=size={w}x{h}:rate=25:duration=0.2"),
                    "-frames:v",
                    "2",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                ])
                .arg(&src)
                .status()
                .expect("spawn lavfi");
            assert!(status.success(), "lavfi {w}x{h} failed");
            let lib = encodecraft_presets::PresetLibrary::new();
            let mut preset = lib.get("system.better-res").unwrap();
            preset.video.width = Some(w);
            preset.video.height = Some(h);
            preset.video.quality = encodecraft_presets::Quality::vbr_2pass(2.0, 3.0);
            encode(&ff, &src, out.clone(), &preset, encodecraft_job::Trim::default(), None, &EncodeStop::new(), |_| {}, |_| {}).unwrap();
            assert!(out.is_file() && out.metadata().map(|m| m.len() > 0).unwrap_or(false), "{name} empty");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn unix_nanos() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    }

    #[test]
    fn first_stdin_plan_is_pass_one() {
        let lib = encodecraft_presets::PresetLibrary::new();
        let mut p = lib.get("system.better-res").unwrap();
        p.video.width = Some(6880);
        p.video.height = Some(1032);
        let raw = RawVideoInput { width: 6880, height: 1032, fps: 25.0, pix_fmt: "yuv420p".into(), from_stdin: true };
        let plans = build_encode_plans(PlanInput {
            source: Path::new("clip.yuv"),
            output: PathBuf::from("out.mp4"),
            preset: &p,
            trim: Trim::default(),
            media: None,
            sequence_start: None,
            raw: Some(raw),
            audio: None,
        });
        let (i, plan) = first_stdin_plan(&plans).expect("stdin pass");
        assert_eq!(i, 0);
        assert!(plan.stdin);
        assert_eq!(plan.pass, 1);
    }

    #[test]
    fn encode_raw_yuv_two_pass_better_res() {
        let Ok(ff) = crate::find_ffmpeg() else {
            eprintln!("skip: ffmpeg not on PATH");
            return;
        };
        let dir = std::env::temp_dir().join(format!("ec-rawyuv-{}-{}", std::process::id(), unix_nanos()));
        let _ = std::fs::create_dir_all(&dir);
        let w = 16u32;
        let h = 16u32;
        let frames = 8u64;
        let bpf = crate::yuv420p_bytes_per_frame(w, h);
        let mut raw = vec![0u8; (bpf * frames) as usize];
        for f in 0..frames {
            let off = (f * bpf) as usize;
            raw[off..off + (w * h) as usize].fill((40 + f * 20) as u8);
        }
        let yuv = dir.join("clip.yuv");
        std::fs::write(&yuv, &raw).unwrap();
        let header = StreamHeader { version: 1, width: w, height: h, fps: 25.0, frames, pix_fmt: "yuv420p".into(), bytes_per_frame: bpf, ..Default::default() };
        let lib = encodecraft_presets::PresetLibrary::new();
        let mut preset = lib.get("system.better-res").unwrap();
        preset.video.width = Some(w);
        preset.video.height = Some(h);
        let out = dir.join("out.mp4");
        let started = Instant::now();
        let result = encode_raw_from_file(&ff, &yuv, out.clone(), &preset, &header, None, false, &EncodeStop::new(), 60, |_| {}, |_| {}).unwrap();
        let secs = started.elapsed().as_secs_f64();
        eprintln!("tiny 16×16 8f Better Res 2-pass (no ProRes): {secs:.3}s");
        assert!(result.output.is_file() && result.output.metadata().map(|m| m.len() > 0).unwrap_or(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// lavfi → each studio preset on a near-static slide and a busy testsrc; ffprobe average ≥ min.
    #[test]
    fn studio_presets_hit_min_bitrate_static_and_busy() {
        let Ok(ff) = crate::find_ffmpeg() else {
            eprintln!("skip: ffmpeg not on PATH");
            return;
        };
        let dir = std::env::temp_dir().join(format!("ec-studio-br-{}-{}", std::process::id(), unix_nanos()));
        let _ = std::fs::create_dir_all(&dir);
        let static_src = dir.join("static.mp4");
        let busy_src = dir.join("busy.mp4");
        let mk = |filter: &str, dest: &Path| {
            let st = Command::new(&ff.ffmpeg)
                .args([
                    "-hide_banner",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    filter,
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000",
                    "-t",
                    "2",
                    "-shortest",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "128k",
                ])
                .arg(dest)
                .status()
                .expect("lavfi");
            assert!(st.success(), "lavfi failed for {}", dest.display());
        };
        mk("color=c=0xCC3300:s=1920x1080:r=25,drawbox=x=80:y=80:w=640:h=120:color=white:t=fill", &static_src);
        mk("testsrc2=size=1920x1080:rate=25", &busy_src);

        let lib = encodecraft_presets::PresetLibrary::new();
        let ids = ["system.better-res", "system.installation", "system.approval-res", "system.sultan"];
        let mut rows = Vec::new();
        for id in ids {
            let preset = lib.get(id).unwrap();
            let min = preset.video.quality.effective_min_mbps().unwrap_or(0.0);
            let max = preset.video.quality.max_kbps().map(|k| k as f64 / 1000.0).unwrap_or(99.0);
            for (clip, src) in [("static", &static_src), ("busy", &busy_src)] {
                let out = dir.join(format!("{id}-{clip}.mp4"));
                let result = encode(&ff, src, out.clone(), &preset, encodecraft_job::Trim::default(), None, &EncodeStop::new(), |_| {}, |_| {}).unwrap();
                let info = crate::probe(&ff, &out).unwrap();
                let mbps = info.video_average_kbps().or(result.measured_video_kbps).unwrap_or(0.0) / 1000.0;
                let has_audio = info.audio.is_some();
                let want_audio = id == "system.approval-res";
                let audio_ok = has_audio == want_audio;
                let min_ok = mbps + 0.08 >= min as f32;
                let max_ok = mbps <= max as f32 * 1.25 + 0.05;
                let pass = min_ok && max_ok && audio_ok;
                rows.push((id, clip, mbps, min, max, has_audio, pass));
                assert!(
                    pass,
                    "{id} {clip}: {mbps:.3} Mbps (min {min} max {max}) audio={has_audio} want_audio={want_audio} filler_warn={} log_tail={}",
                    result.min_bitrate_warning,
                    result.log.chars().rev().take(800).collect::<String>().chars().rev().collect::<String>()
                );
            }
        }
        eprintln!("preset            clip     Mbps   min    max    audio  pass");
        for (id, clip, mbps, min, max, audio, pass) in &rows {
            eprintln!("{id:<18} {clip:<7} {mbps:>5.2}  {min:>4.1}  {max:>4.1}  {audio:<5}  {}", if *pass { "ok" } else { "FAIL" });
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
