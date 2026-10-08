use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use encodecraft_ffmpeg::{effectcraft_render_args, encode_with_stall, find_effectcraft_cli, find_ffmpeg_at, mezzanine_path, probe, EncodeStop};
use encodecraft_job::JobSource;
use encodecraft_queue::{unix_ms, JobStatus};

use crate::fsutil;
use crate::paths;
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

    let source_file = resolve_source(h, &job, &stop)?;
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
    let last_prog = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1)));
    let stall = h.lock(|s| s.settings.stall_timeout_sec).max(15);
    let sequence_start = match &job.source {
        JobSource::File { sequence_start, .. } => *sequence_start,
        JobSource::EffectCraft { .. } => None,
    };
    let result = encode_with_stall(
        &ff,
        &source_file,
        job.output.clone(),
        &job.preset,
        job.trim,
        media.as_ref(),
        sequence_start,
        &stop,
        stall,
        |p| {
            let snap = encodecraft_queue::ProgressSnap::from(p);
            let mut last = last_prog.lock().unwrap_or_else(|p| p.into_inner());
            if last.elapsed() < std::time::Duration::from_millis(120) && !p.done {
                return;
            }
            *last = std::time::Instant::now();
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

fn resolve_source(h: &Handle, job: &encodecraft_queue::Job, stop: &EncodeStop) -> Result<PathBuf, crate::EngineError> {
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
            h.lock(|s| s.push_log(format!("{}: rendering EffectCraft mezzanine → {}", job.id, out.display())));
            run_command(&args, stop)?;
            if !out.exists() {
                return Err(crate::EngineError::Message(format!("effectcraft-cli did not write {}", out.display())));
            }
            Ok(out)
        }
    }
}

fn run_command(args: &[String], stop: &EncodeStop) -> Result<(), crate::EngineError> {
    let (prog, rest) = args.split_first().ok_or_else(|| crate::EngineError::Message("empty command".into()))?;
    let mut child = Command::new(prog).args(rest).stdin(std::process::Stdio::null()).spawn()?;
    loop {
        if stop.is_requested() {
            let _ = child.kill();
            return Err(crate::EngineError::Message("stopped".into()));
        }
        match child.try_wait()? {
            Some(st) if st.success() => return Ok(()),
            Some(st) => return Err(crate::EngineError::Message(format!("effectcraft-cli exited {}", st.code().unwrap_or(-1)))),
            None => thread::sleep(Duration::from_millis(150)),
        }
    }
}
