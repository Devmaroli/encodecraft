use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use encodecraft_ffmpeg::{effectcraft_render_args, encode_with_stall, find_effectcraft_cli, find_ffmpeg_at, hidden_command, mezzanine_path, probe, EncodeStop};
use encodecraft_job::JobSource;
use encodecraft_presets::OutputFormat;
use encodecraft_queue::{unix_ms, JobStatus};

use crate::fsutil;
use crate::paths;
use crate::stage::{self, JobProgress, StageClock};
use crate::Handle;

pub fn run(h: Handle) {
    let mut ticks = 0u64;
    while h.is_alive() {
        thread::sleep(Duration::from_millis(250));
        ticks = ticks.wrapping_add(1);
        if ticks % 4 == 0 && h.take_dirty() {
            h.persist();
        }
        dispatch(&h);
    }
}

fn dispatch(h: &Handle) {
    let (running, max, waiting) = h.lock(|s| {
        s.queue.max_parallel = s.settings.max_parallel.max(1);
        (s.queue.running, s.queue.max_parallel, s.queue.next_waiting())
    });
    if !running {
        return;
    }
    let active = h.lock(|s| s.queue.active_count());
    if active >= max {
        return;
    }
    let Some(id) = waiting else {
        return;
    };
    h.lock(|s| {
        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
            j.status = JobStatus::Encoding;
            j.started_ms = Some(unix_ms());
            j.error = None;
        }
    });
    h.note_change();
    h.emit_ui(crate::UiEvent::Changed);
    let handle = h.clone();
    let job_id = id.clone();
    let _ = thread::Builder::new().name(format!("encode-{id}")).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encode_job(&handle, &job_id)))
            .unwrap_or_else(|_| Err(crate::EngineError::Message("encode worker panicked".into())));
        handle.lock(|s| {
            if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == job_id) {
                match result {
                    Ok(()) => {
                        if j.status != JobStatus::Stopped {
                            j.status = JobStatus::Done;
                            j.progress.percent = 100.0;
                        }
                    }
                    Err(e) => {
                        if j.status != JobStatus::Stopped {
                            let msg = e.to_string();
                            if msg == "stopped" {
                                j.status = JobStatus::Stopped;
                            } else {
                                j.status = JobStatus::Failed;
                                j.error = Some(msg.clone());
                                j.log.push_str(&msg);
                                j.log.push('\n');
                            }
                        }
                    }
                }
                j.finished_ms = Some(unix_ms());
            }
        });
        handle.unregister_stop(&job_id);
        handle.note_change();
        handle.mark_dirty();
    });
}

fn encode_job(h: &Handle, id: &str) -> Result<(), crate::EngineError> {
    let stop = EncodeStop::new();
    h.register_stop(id.to_string(), stop.clone());

    let job = h.lock(|s| s.queue.jobs.iter().find(|j| j.id == id).cloned()).ok_or_else(|| crate::EngineError::Message("job disappeared".into()))?;
    if let Some(msg) = job.encode_warning.clone() {
        return Err(crate::EngineError::Message(msg));
    }

    let override_path = h.lock(|s| s.settings.ffmpeg_path.clone());
    let ff = find_ffmpeg_at(override_path.as_deref())?;

    let gpu_on = h.lock(|s| s.settings.gpu_encode);
    let use_nvenc = encodecraft_ffmpeg::use_gpu_encoder(gpu_on, ff.nvenc, ff.nvidia, job.preset.video.allow_gpu, job.preset.format == OutputFormat::H264Mp4);
    let mut preset = job.preset.clone();
    preset.video.use_nvenc = use_nvenc;
    let encoder_label = if use_nvenc { "GPU" } else { "CPU" };
    h.lock(|s| {
        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
            j.encoder_used = Some(encoder_label.into());
        }
    });
    let has_render = job.source.is_effectcraft();
    let n_passes = if preset.format == OutputFormat::H264Mp4 && preset.video.quality.is_two_pass() && !preset.video.use_nvenc { 2 } else { 1 };
    let mapper = Arc::new(JobProgress::for_job(has_render, n_passes, &stage::load()));
    let clock = Arc::new(Mutex::new(StageClock::new(mapper.labels.len())));
    let started_ms = job.started_ms.unwrap_or_else(unix_ms);

    let source_file = resolve_source(h, &job, &stop, Arc::clone(&mapper), Arc::clone(&clock), started_ms)?;
    let media = probe(&ff, &source_file).ok();
    let mut job = job;
    if let Some(ref m) = media {
        let allow = h.lock(|s| s.settings.allow_overwrite);
        let renamed = job.apply_resolved_size(m.width(), m.height(), allow);
        h.lock(|s| {
            if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
                j.output = job.output.clone();
                j.preset.video.width = job.preset.video.width;
                j.preset.video.height = job.preset.video.height;
                j.size_name_pending = job.size_name_pending;
                j.encode_warning = job.encode_warning.clone();
            }
            s.push_log(format!("{}: {}", job.id, m.summary()));
            if renamed {
                s.push_log(format!("{}: output {}", job.id, job.output.display()));
            }
        });
    }
    if let Some(msg) = job.encode_warning.clone() {
        return Err(crate::EngineError::Message(msg));
    }

    let hid = id.to_string();
    let h_prog = h.clone();
    let h_log = h.clone();
    let last_prog = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1)));
    let stall = h.lock(|s| s.settings.stall_timeout_sec).max(15);
    let sequence_start = match &job.source {
        JobSource::File { sequence_start, .. } => *sequence_start,
        JobSource::EffectCraft { .. } => None,
    };
    let encode_stage0 = if has_render { 1 } else { 0 };
    let mapper_cb = Arc::clone(&mapper);
    let clock_cb = Arc::clone(&clock);
    let result = encode_with_stall(
        &ff,
        &source_file,
        job.output.clone(),
        &preset,
        job.trim,
        media.as_ref(),
        sequence_start,
        &stop,
        stall,
        |p| {
            let mut last = last_prog.lock().unwrap_or_else(|p| p.into_inner());
            if last.elapsed() < Duration::from_millis(120) && !p.done {
                return;
            }
            *last = Instant::now();
            let pass_i = if p.pass_count > 0 { p.pass_index } else { 0 };
            let stage = encode_stage0 + pass_i;
            if let Ok(mut c) = clock_cb.lock() {
                c.enter(stage);
            }
            let stage_pct = if p.pass_count > 1 { p.pass_percent } else { p.percent };
            let label = if p.stage.is_empty() { mapper_cb.label(stage).to_string() } else { p.stage.clone() };
            let mut snap = encodecraft_queue::ProgressSnap::from(p);
            snap.stage = label;
            snap.percent = mapper_cb.overall(stage, stage_pct);
            let elapsed = (unix_ms().saturating_sub(started_ms) as f64) / 1000.0;
            if snap.percent > 1.0 && elapsed > 0.4 {
                let total = elapsed / (snap.percent as f64 / 100.0);
                snap.eta_sec = Some((total - elapsed).max(0.0));
            }
            h_prog.lock(|s| {
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == hid) {
                    j.progress = snap;
                }
            });
            h_prog.note_change();
            h_prog.emit_ui(crate::UiEvent::Changed);
        },
        |line| {
            let line = line.to_string();
            h_log.lock(|s| {
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == hid) {
                    const MAX: usize = 24_576;
                    if j.log.len() > MAX {
                        j.log = j.log[j.log.len() - 12_288..].to_string();
                    }
                    j.log.push_str(&line);
                    j.log.push('\n');
                }
            });
        },
    )?;
    if let Ok(mut c) = clock.lock() {
        c.finish();
        stage::record(has_render, n_passes, c.samples());
    }
    h.lock(|s| {
        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
            j.measured_bitrate_kbps = result.measured_video_kbps;
            if let Some(kbps) = result.measured_video_kbps {
                s.push_log(format!("{}: average {:.2} Mbps", id, kbps / 1000.0));
            }
            if result.min_bitrate_warning {
                s.push_log(format!("{}: finished under the preset minimum bitrate", id));
            }
        }
    });
    Ok(())
}

fn resolve_source(
    h: &Handle,
    job: &encodecraft_queue::Job,
    stop: &EncodeStop,
    mapper: Arc<JobProgress>,
    clock: Arc<Mutex<StageClock>>,
    started_ms: u64,
) -> Result<PathBuf, crate::EngineError> {
    match &job.source {
        JobSource::File { path, .. } => {
            if fsutil::is_symlink(path) {
                return Err(crate::EngineError::Message("refusing to encode a symlink source".into()));
            }
            if !path.is_file() {
                return Err(crate::EngineError::Message(format!("source is missing: {}", path.display())));
            }
            Ok(path.clone())
        }
        JobSource::EffectCraft { mezzanine, .. } => {
            let cli = h
                .lock(|s| s.settings.effectcraft_cli.clone())
                .and_then(|p| if p.is_file() { Some(p) } else { None })
                .or_else(|| find_effectcraft_cli().ok())
                .ok_or(encodecraft_ffmpeg::FfmpegError::EffectCraftMissing)?;
            let out = mezzanine_path(&paths::work_dir(), &job.id, mezzanine);
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let args = effectcraft_render_args(&cli, &job.source, &out)?;
            h.lock(|s| {
                s.push_log(format!("{}: rendering EffectCraft mezzanine → {}", job.id, out.display()));
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == job.id) {
                    j.progress.stage = "Rendering comp".into();
                    j.progress.percent = 0.0;
                }
            });
            let hid = job.id.clone();
            let handle = h.clone();
            let last_parsed = Arc::new(Mutex::new(None::<RenderProgress>));
            let last_ui = Arc::new(Mutex::new(Instant::now() - Duration::from_millis(200)));
            let apply = Arc::new({
                let handle = handle.clone();
                let hid = hid.clone();
                let mapper = Arc::clone(&mapper);
                let last_parsed = Arc::clone(&last_parsed);
                let last_ui = Arc::clone(&last_ui);
                move |force: bool| {
                    let mut last = last_ui.lock().unwrap_or_else(|p| p.into_inner());
                    if !force && last.elapsed() < Duration::from_millis(80) {
                        return;
                    }
                    *last = Instant::now();
                    let parsed = last_parsed.lock().unwrap_or_else(|p| p.into_inner()).clone();
                    let elapsed = (unix_ms().saturating_sub(started_ms) as f64) / 1000.0;
                    let stage_pct = match &parsed {
                        Some(p) => p.percent,
                        None => {
                            let expected = mapper.expected_sec(0);
                            ((elapsed / expected) * 90.0).clamp(0.0, 90.0) as f32
                        }
                    };
                    let percent = mapper.overall(0, stage_pct);
                    let eta = if percent > 1.0 && elapsed > 0.4 {
                        let total = elapsed / (percent as f64 / 100.0);
                        Some((total - elapsed).max(0.0))
                    } else {
                        None
                    };
                    handle.lock(|s| {
                        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == hid) {
                            j.progress.stage = "Rendering comp".into();
                            j.progress.percent = percent;
                            j.progress.eta_sec = eta;
                            if let Some(p) = &parsed {
                                if let Some(n) = p.frame {
                                    j.progress.frame = n;
                                }
                                if let Some(t) = p.total {
                                    j.progress.total_frames = t;
                                }
                            }
                        }
                    });
                    handle.note_change();
                    handle.emit_ui(crate::UiEvent::Changed);
                }
            });
            run_command(
                &args,
                stop,
                {
                    let apply = Arc::clone(&apply);
                    let last_parsed = Arc::clone(&last_parsed);
                    let handle = handle.clone();
                    let hid = hid.clone();
                    move |line| {
                        if let Some(p) = parse_render_progress(line) {
                            *last_parsed.lock().unwrap_or_else(|p| p.into_inner()) = Some(p);
                            apply(true);
                        }
                        handle.lock(|s| {
                            if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == hid) {
                                const MAX: usize = 24_576;
                                if j.log.len() > MAX {
                                    j.log = j.log[j.log.len() - 12_288..].to_string();
                                }
                                j.log.push_str(line);
                                j.log.push('\n');
                            }
                        });
                    }
                },
                {
                    let apply = Arc::clone(&apply);
                    move || apply(false)
                },
            )?;
            if !out.exists() {
                return Err(crate::EngineError::Message(format!("effectcraft-cli did not write {}", out.display())));
            }
            if let Ok(mut c) = clock.lock() {
                c.enter(1.min(mapper.labels.len().saturating_sub(1)));
            }
            h.lock(|s| {
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == job.id) {
                    j.progress.stage = mapper.label(1.min(mapper.labels.len().saturating_sub(1))).into();
                    j.progress.percent = mapper.overall(0, 100.0);
                }
            });
            Ok(out)
        }
    }
}

fn run_command(args: &[String], stop: &EncodeStop, mut on_line: impl FnMut(&str), mut on_tick: impl FnMut()) -> Result<(), crate::EngineError> {
    let (prog, rest) = args.split_first().ok_or_else(|| crate::EngineError::Message("empty command".into()))?;
    let mut child = hidden_command(prog).args(rest).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if let Some(out) = stdout {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(err) = stderr {
        thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    loop {
        if stop.is_requested() {
            let _ = child.kill();
            return Err(crate::EngineError::Message("stopped".into()));
        }
        while let Ok(line) = rx.try_recv() {
            on_line(&line);
        }
        match child.try_wait()? {
            Some(st) if st.success() => {
                while let Ok(line) = rx.try_recv() {
                    on_line(&line);
                }
                return Ok(());
            }
            Some(st) => return Err(crate::EngineError::Message(format!("effectcraft-cli exited {}", st.code().unwrap_or(-1)))),
            None => {
                on_tick();
                thread::sleep(Duration::from_millis(80));
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderProgress {
    pub percent: f32,
    pub frame: Option<u64>,
    pub total: Option<u64>,
}

fn json_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|i| u64::try_from(i).ok()))
        .or_else(|| v.as_f64().and_then(|f| if f.is_finite() && f >= 0.0 { Some(f as u64) } else { None }))
}

fn json_f32(v: &serde_json::Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32).or_else(|| v.as_u64().map(|n| n as f32)).or_else(|| v.as_i64().map(|n| n as f32))
}

fn from_frame_total(n: u64, m: u64) -> Option<RenderProgress> {
    if m == 0 {
        return None;
    }
    Some(RenderProgress { percent: (n as f32 / m as f32 * 100.0).clamp(0.0, 100.0), frame: Some(n), total: Some(m) })
}

/// Parse EffectCraft / FFmpeg-style render progress.
pub(crate) fn parse_render_progress(line: &str) -> Option<RenderProgress> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if line.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let total = v.get("total").or_else(|| v.get("frames")).and_then(json_u64);
            let n = v.get("n").or_else(|| v.get("frame")).or_else(|| v.get("current")).and_then(json_u64);
            if let (Some(n), Some(m)) = (n, total) {
                return from_frame_total(n, m);
            }
            if let Some(p) = v.get("percent").or_else(|| v.get("progress")).and_then(json_f32) {
                let percent = if p <= 1.0 && p >= 0.0 && v.get("percent").is_none() { p * 100.0 } else { p };
                return Some(RenderProgress { percent: percent.clamp(0.0, 100.0), frame: n, total });
            }
        }
    }
    if let Some(rest) = line.strip_prefix("progress=") {
        let v: f32 = rest.trim().parse().ok()?;
        return Some(RenderProgress { percent: if v <= 1.0 { v * 100.0 } else { v.clamp(0.0, 100.0) }, frame: None, total: None });
    }
    if let Some(rest) = line.strip_prefix("percent=") {
        return rest.trim().parse::<f32>().ok().map(|v| RenderProgress { percent: v.clamp(0.0, 100.0), frame: None, total: None });
    }
    let lower = line.to_ascii_lowercase();
    if let Some(idx) = lower.find(" of ") {
        let before = line[..idx].rsplit(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty())?;
        let after = line[idx + 4..].split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty())?;
        let n: u64 = before.parse().ok()?;
        let m: u64 = after.parse().ok()?;
        return from_frame_total(n, m);
    }
    if let Some(i) = line.find("frame=") {
        let rest = line[i + 6..].trim();
        let n: u64 = rest.split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty())?.parse().ok()?;
        if let Some(t) = lower.find("total=") {
            let m: u64 = line[t + 6..].split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty())?.parse().ok()?;
            return from_frame_total(n, m);
        }
    }
    parse_slash_fraction(line)
}

/// `12/250` or `frame 12/250`. Skip date-like `2024/10`.
fn parse_slash_fraction(line: &str) -> Option<RenderProgress> {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'/' {
                let slash = i;
                i += 1;
                let t0 = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > t0 {
                    if let (Ok(n), Ok(m)) = (line[start..slash].parse::<u64>(), line[t0..i].parse::<u64>()) {
                        if m >= 2 && n <= m && m <= 1_000_000 {
                            // 2024/10 looks like a date, not a frame count.
                            if n >= 1000 && m <= 31 {
                                continue;
                            }
                            return from_frame_total(n, m);
                        }
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_progress_parses_common_lines() {
        let p = parse_render_progress("progress=0.4").unwrap();
        assert!((p.percent - 40.0).abs() < 0.01);
        let p = parse_render_progress("percent=12.5").unwrap();
        assert!((p.percent - 12.5).abs() < 0.01);
        let p = parse_render_progress("Rendering frame 25 of 250").unwrap();
        assert_eq!((p.frame, p.total), (Some(25), Some(250)));
        assert!((p.percent - 10.0).abs() < 0.01);
        assert!(parse_render_progress("hello").is_none());
    }

    #[test]
    fn render_progress_parses_json_and_slash() {
        let p = parse_render_progress(r#"{"event":"frame","n":12,"total":250}"#).unwrap();
        assert_eq!((p.frame, p.total), (Some(12), Some(250)));
        assert!((p.percent - 4.8).abs() < 0.05);
        let p = parse_render_progress(r#"{"event":"progress","frame":50,"total":200}"#).unwrap();
        assert_eq!(p.frame, Some(50));
        let p = parse_render_progress("12/250").unwrap();
        assert_eq!((p.frame, p.total), (Some(12), Some(250)));
        let p = parse_render_progress("frame= 40 total= 200").unwrap();
        assert_eq!((p.frame, p.total), (Some(40), Some(200)));
        assert!(parse_render_progress("2024/10").is_none());
    }
}
