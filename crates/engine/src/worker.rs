use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use encodecraft_ffmpeg::{effectcraft_render_args, encode, find_effectcraft_cli, find_ffmpeg_at, mezzanine_path, probe, EncodeStop};
use encodecraft_job::JobSource;
use encodecraft_queue::{unix_ms, JobStatus};

use crate::paths;
use crate::Handle;

pub fn run(h: Handle) {
    let mut ticks = 0u64;
    loop {
        thread::sleep(Duration::from_millis(250));
        ticks = ticks.wrapping_add(1);
        if ticks % 20 == 0 {
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
    let handle = h.clone();
    let job_id = id.clone();
    let _ = thread::Builder::new().name(format!("encode-{id}")).spawn(move || {
        let result = encode_job(&handle, &job_id);
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
        handle.persist();
    });
}

fn encode_job(h: &Handle, id: &str) -> Result<(), crate::EngineError> {
    let stop = EncodeStop::new();
    h.register_stop(id.to_string(), stop.clone());

    let job = h.lock(|s| s.queue.jobs.iter().find(|j| j.id == id).cloned()).ok_or_else(|| crate::EngineError::Message("job disappeared".into()))?;

    let override_path = h.lock(|s| s.settings.ffmpeg_path.clone());
    let ff = find_ffmpeg_at(override_path.as_deref())?;

    let source_file = resolve_source(h, &job, &stop)?;
    let media = probe(&ff, &source_file).ok();
    if let Some(ref m) = media {
        h.lock(|s| s.push_log(format!("{}: {}", job.id, m.summary())));
    }

    let hid = id.to_string();
    let h_prog = h.clone();
    let h_log = h.clone();
    encode(
        &ff,
        &source_file,
        job.output.clone(),
        &job.preset,
        job.trim,
        media.as_ref(),
        &stop,
        |p| {
            let snap = encodecraft_queue::ProgressSnap::from(p);
            h_prog.lock(|s| {
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == hid) {
                    j.progress = snap.clone();
                }
            });
        },
        |line| {
            let line = line.to_string();
            h_log.lock(|s| {
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == hid) {
                    if j.log.len() > 32_768 {
                        j.log = j.log[j.log.len() - 16_384..].to_string();
                    }
                    j.log.push_str(&line);
                    j.log.push('\n');
                }
            });
        },
    )?;
    Ok(())
}

fn resolve_source(h: &Handle, job: &encodecraft_queue::Job, stop: &EncodeStop) -> Result<PathBuf, crate::EngineError> {
    match &job.source {
        JobSource::File { path } => Ok(path.clone()),
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
    let mut child = Command::new(prog).args(rest).spawn()?;
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
