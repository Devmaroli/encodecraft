//! Stream EffectCraft yuv420p into FFmpeg. Pass 1 overlaps the render; pass 2 reads the raw file.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use encodecraft_ffmpeg::{
    audio_wav_path, build_encode_plans, encode_raw_from_file, feed_pass_stdin, first_stdin_plan, hidden_command, intermediate_yuv_path, load_sidecar,
    media_from_header, parse_stream_event, partial_output, probe_cli_caps, read_exact_frame, read_growing_file, sidecar_path, spawn_plan, stream_render_args,
    wait_spawned_plan, EncodeStop, Ffmpeg, PlanInput, RawVideoInput, SpawnedPlan, StreamEvent, StreamHeader,
};
use encodecraft_job::JobSource;
use encodecraft_presets::ExportPreset;
use encodecraft_queue::{unix_ms, Job};
use serde_json::json;

use crate::ecserve::ServePool;
use crate::paths;
use crate::stage::JobProgress;
use crate::tasks::UiEvent;
use crate::{EngineError, Handle};

/// Settings checkbox, overridden by `ENCODECRAFT_GPU_RENDER=0/1`. Off by default — CPU render was
/// faster than GPU on RTX 3070 Ti. EffectCraft still CPU-fallbacks if `--gpu` is passed and the
/// adapter cannot take the frame.
pub fn gpu_render_from(env: Option<&str>, settings_on: bool) -> bool {
    match env.map(str::trim) {
        Some("0") | Some("false") | Some("off") | Some("no") => false,
        Some("1") | Some("true") | Some("on") | Some("yes") => true,
        _ => settings_on,
    }
}

pub fn gpu_render_enabled(settings_on: bool) -> bool {
    gpu_render_from(std::env::var("ENCODECRAFT_GPU_RENDER").ok().as_deref(), settings_on)
}

pub fn job_wants_audio(preset: &ExportPreset) -> bool {
    preset.audio.enabled && preset.format.has_video() && preset.format.ffmpeg_audio_codec().is_some()
}

fn preview_jpeg_from_yuv(frame: &[u8], width: u32, height: u32, dest: &Path) -> bool {
    let w = width.max(2);
    let h = height.max(2);
    let need = (w as usize).saturating_mul(h as usize);
    if frame.len() < need {
        return false;
    }
    let mut img = image::RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let yv = frame[(y * w + x) as usize];
            img.put_pixel(x, y, image::Rgb([yv, yv, yv]));
        }
    }
    let max_w = 480u32;
    let scaled = if w > max_w {
        let nh = ((h as u64 * max_w as u64) / w as u64).max(2) as u32;
        image::imageops::resize(&img, max_w, nh, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    if let Some(dir) = dest.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    scaled.save(dest).is_ok()
}

fn emit_preview(h: &Handle, job: &Job, dest: &Path, header: &StreamHeader) {
    let Ok(bytes) = std::fs::read(dest) else { return };
    let Ok(img) = image::load_from_memory(&bytes) else { return };
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let height = rgba.height();
    let name = job.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| job.source.display_name());
    h.emit_ui(UiEvent::PreviewFrame {
        id: job.id.clone(),
        name,
        rgba: rgba.into_raw(),
        w,
        h: height,
        output_w: header.width.max(job.preset.video.width.unwrap_or(0)),
        output_h: header.height.max(job.preset.video.height.unwrap_or(0)),
    });
}

fn apply_render_progress(h: &Handle, id: &str, mapper: &JobProgress, started_ms: u64, n: u64, total: u64) {
    let stage_pct = if total > 0 { (n as f32 / total as f32 * 100.0).clamp(0.0, 100.0) } else { 0.0 };
    let percent = mapper.overall(0, stage_pct);
    let elapsed = (unix_ms().saturating_sub(started_ms) as f64) / 1000.0;
    let eta = if percent > 1.0 && elapsed > 0.4 { Some((elapsed / (percent as f64 / 100.0) - elapsed).max(0.0)) } else { None };
    h.lock(|s| {
        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
            j.progress.stage = "Rendering comp".into();
            j.progress.percent = percent;
            j.progress.eta_sec = eta;
            if n > 0 {
                j.progress.frame = n;
            }
            if total > 0 {
                j.progress.total_frames = total;
            }
        }
    });
    h.note_change();
    h.emit_ui(UiEvent::Changed);
}

struct Pass1 {
    spawned: SpawnedPlan,
    stdin: Option<std::process::ChildStdin>,
}

fn start_pass1(
    ff: &Ffmpeg,
    preset: &ExportPreset,
    header: &StreamHeader,
    yuv: &Path,
    audio: Option<&Path>,
    tmp_out: PathBuf,
    stall: u64,
) -> Result<Pass1, EngineError> {
    let media = media_from_header(header, yuv);
    let raw = RawVideoInput::from_header(header, true);
    let plans = build_encode_plans(PlanInput {
        source: yuv,
        output: tmp_out,
        preset,
        trim: Default::default(),
        media: Some(&media),
        sequence_start: None,
        raw: Some(raw),
        audio,
    });
    let duration = header.duration_sec();
    let (i, plan) = first_stdin_plan(&plans).ok_or_else(|| EngineError::Message("encode plan has no stdin pass".into()))?;
    let mut spawned = spawn_plan(ff, plan, duration, stall, i, plans.len())?;
    let stdin = spawned.stdin();
    Ok(Pass1 { spawned, stdin })
}

fn maybe_preview(h: &Handle, job: &Job, header: &StreamHeader, frame: &[u8], preview_on: bool, last: &mut Instant) {
    if !preview_on || last.elapsed() < Duration::from_millis(400) {
        return;
    }
    *last = Instant::now();
    let dest = paths::thumbs_dir().join(format!("preview-{}.jpg", job.id));
    if preview_jpeg_from_yuv(frame, header.width, header.height, &dest) {
        emit_preview(h, job, &dest, header);
    }
}

fn finish_encode(
    h: &Handle,
    job: &Job,
    ff: &Ffmpeg,
    preset: &ExportPreset,
    header: &StreamHeader,
    yuv: &Path,
    audio: Option<&Path>,
    stop: &EncodeStop,
    stall: u64,
    mapper: &JobProgress,
) -> Result<encodecraft_ffmpeg::EncodeResult, EngineError> {
    let hid = job.id.clone();
    let h_prog = h.clone();
    let mapper = mapper.clone();
    let result = encode_raw_from_file(
        ff,
        yuv,
        job.output.clone(),
        preset,
        header,
        audio.filter(|p| p.is_file()),
        true,
        stop,
        stall,
        move |p| {
            let stage_i = if p.pass_index == 0 { 1 } else { 1 + p.pass_index.min(1) };
            let percent = mapper.overall(stage_i.min(mapper.labels.len().saturating_sub(1)), p.pass_percent);
            h_prog.lock(|s| {
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == hid) {
                    j.progress.stage = p.stage.clone();
                    j.progress.percent = percent;
                    j.progress.frame = p.frame;
                }
            });
            h_prog.note_change();
            h_prog.emit_ui(UiEvent::Changed);
        },
        |_| {},
    )?;
    Ok(result)
}

pub fn try_stream(
    h: &Handle,
    job: &Job,
    ff: &Ffmpeg,
    preset: &ExportPreset,
    stop: &EncodeStop,
    mapper: Arc<JobProgress>,
    started_ms: u64,
    stall: u64,
    serve: &ServePool,
) -> Result<Option<encodecraft_ffmpeg::EncodeResult>, EngineError> {
    let JobSource::EffectCraft { .. } = &job.source else {
        return Ok(None);
    };
    let cli = h
        .lock(|s| s.settings.effectcraft_cli.clone())
        .and_then(|p| if p.is_file() { Some(p) } else { None })
        .or_else(|| encodecraft_ffmpeg::find_effectcraft_cli().ok())
        .ok_or(encodecraft_ffmpeg::FfmpegError::EffectCraftMissing)?;
    let caps = probe_cli_caps(&cli);
    if !caps.streaming() {
        h.lock(|s| s.push_log(format!("{}: EffectCraft CLI has no yuv420p stream — ProRes mezzanine fallback", job.id)));
        return Ok(None);
    }
    let settings_gpu = h.lock(|s| s.settings.gpu_render);
    let gpu = gpu_render_enabled(settings_gpu);
    let fps = if caps.fps_flag { job.preset.video.frame_rate.output_fps() } else { None };
    let work = paths::work_dir();
    std::fs::create_dir_all(&work)?;
    let yuv = intermediate_yuv_path(&work, &job.id);
    let sidecar = sidecar_path(&work, &job.id);
    let wav = audio_wav_path(&work, &job.id);
    let _ = std::fs::remove_file(&yuv);
    let audio = if job_wants_audio(preset) { Some(wav.as_path()) } else { None };

    h.lock(|s| {
        s.push_log(format!("{}: streaming yuv420p (true 2-pass, no ProRes mezzanine)", job.id));
        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == job.id) {
            j.progress.stage = "Rendering comp".into();
            j.progress.percent = 0.0;
        }
    });

    if let Some(f) = fps {
        h.lock(|s| s.push_log(format!("{}: render at {} fps (preset output rate)", job.id, encodecraft_ffmpeg::format_cli_fps(f))));
    }
    if caps.serve && !serve.rendering() {
        match stream_via_serve(h, job, &cli, gpu, fps, &yuv, &sidecar, audio, ff, preset, stop, &mapper, started_ms, stall, serve) {
            Ok(r) => return Ok(Some(r)),
            Err(e) => {
                h.lock(|s| s.push_log(format!("{}: warm serve failed ({e}) — cold stream", job.id)));
            }
        }
    }
    let r = stream_via_cold(h, job, &cli, gpu, fps, &yuv, &sidecar, audio, ff, preset, stop, &mapper, started_ms, stall)?;
    Ok(Some(r))
}

#[allow(clippy::too_many_arguments)]
fn stream_via_cold(
    h: &Handle,
    job: &Job,
    cli: &Path,
    gpu: bool,
    fps: Option<f64>,
    yuv: &Path,
    sidecar: &Path,
    audio: Option<&Path>,
    ff: &Ffmpeg,
    preset: &ExportPreset,
    stop: &EncodeStop,
    mapper: &JobProgress,
    started_ms: u64,
    stall: u64,
) -> Result<encodecraft_ffmpeg::EncodeResult, EngineError> {
    let args = stream_render_args(cli, &job.source, Path::new("-"), sidecar, audio, gpu, fps)?;
    let (prog, rest) = args.split_first().ok_or_else(|| EngineError::Message("empty render argv".into()))?;
    let mut child = hidden_command(prog).args(rest).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| EngineError::Message("render has no stdout".into()))?;
    let stderr = child.stderr.take().ok_or_else(|| EngineError::Message("render has no stderr".into()))?;
    let (tx, rx) = mpsc::channel::<StreamEvent>();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(ev) = parse_stream_event(&line) {
                if tx.send(ev).is_err() {
                    break;
                }
            }
        }
    });

    let mut header: Option<StreamHeader> = None;
    let start = Instant::now();
    while header.is_none() {
        if stop.is_requested() {
            let _ = child.kill();
            return Err(EngineError::Message("stopped".into()));
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(StreamEvent::Header(hd)) => header = Some(hd),
            Ok(StreamEvent::Error { message }) => return Err(EngineError::Message(message)),
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(hd) = load_sidecar(sidecar) {
                    header = Some(hd);
                } else if start.elapsed() > Duration::from_secs(45) {
                    let _ = child.kill();
                    return Err(EngineError::Message("timed out waiting for stream header".into()));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                header = load_sidecar(sidecar);
                break;
            }
        }
    }
    let header = header.ok_or_else(|| EngineError::Message("effectcraft-cli did not send a stream header".into()))?;
    h.lock(|s| {
        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == job.id) {
            let _ = j.apply_resolved_size(header.width, header.height, s.settings.allow_overwrite);
        }
        s.push_log(format!("{}: {}×{} {} fps, {} frames", job.id, header.width, header.height, header.ffmpeg_fps(), header.frames));
        if header.gpu {
            s.push_log(format!("{}: EffectCraft GPU render", job.id));
        } else if gpu {
            s.push_log(format!("{}: EffectCraft GPU render unavailable — CPU", job.id));
        }
    });
    apply_render_progress(h, &job.id, mapper, started_ms, 0, header.frames);

    let audio = if header.audio { audio } else { None };
    let tmp = partial_output(&job.output);
    let mut pass1 = match start_pass1(ff, preset, &header, yuv, audio, tmp, stall) {
        Ok(p) => p,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    let mut yuv_file = File::create(yuv)?;
    let bpf = header.bytes_per_frame() as usize;
    let preview_on = h.lock(|s| s.settings.show_encode_preview);
    let mut last_preview = Instant::now() - Duration::from_secs(1);
    let mut stdout = stdout;
    let mut n = 0u64;
    loop {
        if stop.is_requested() {
            let _ = child.kill();
            drop(pass1.stdin.take());
            return Err(EngineError::Message("stopped".into()));
        }
        while let Ok(ev) = rx.try_recv() {
            match ev {
                StreamEvent::Frame { n: fn_, total, .. } => apply_render_progress(h, &job.id, mapper, started_ms, fn_, total),
                StreamEvent::Error { message } => {
                    let _ = child.kill();
                    drop(pass1.stdin.take());
                    return Err(EngineError::Message(message));
                }
                StreamEvent::Gpu { used, reason } => {
                    h.lock(|s| s.push_log(format!("{}: gpu used={used} {}", job.id, reason.unwrap_or_default())));
                }
                _ => {}
            }
        }
        match read_exact_frame(&mut stdout, bpf, &|| stop.is_requested())? {
            None => break,
            Some(frame) => {
                n += 1;
                if let Some(ref mut sin) = pass1.stdin {
                    feed_pass_stdin(sin, &frame).map_err(|e| EngineError::Message(e.to_string()))?;
                }
                yuv_file.write_all(&frame)?;
                maybe_preview(h, job, &header, &frame, preview_on, &mut last_preview);
                apply_render_progress(h, &job.id, mapper, started_ms, n, header.frames.max(n));
            }
        }
    }
    drop(yuv_file);
    if let Some(ref mut sin) = pass1.stdin {
        let _ = sin.flush();
    }
    drop(pass1.stdin.take());
    let st = child.wait()?;
    if !st.success() {
        return Err(EngineError::Message(format!("effectcraft-cli exited {}", st.code().unwrap_or(-1))));
    }
    wait_pass1(h, job, mapper, &mut pass1, stop)?;
    finish_encode(h, job, ff, preset, &header, yuv, audio, stop, stall, mapper)
}

fn wait_pass1(h: &Handle, job: &Job, mapper: &JobProgress, pass1: &mut Pass1, stop: &EncodeStop) -> Result<(), EngineError> {
    let hid = job.id.clone();
    let h_prog = h.clone();
    let mapper = mapper.clone();
    wait_spawned_plan(
        &mut pass1.spawned,
        stop,
        move |p| {
            let percent = mapper.overall(1.min(mapper.labels.len().saturating_sub(1)), p.pass_percent);
            h_prog.lock(|s| {
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == hid) {
                    j.progress.stage = p.stage.clone();
                    j.progress.percent = percent;
                    j.progress.frame = p.frame;
                }
            });
            h_prog.note_change();
            h_prog.emit_ui(UiEvent::Changed);
        },
        |_| {},
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stream_via_serve(
    h: &Handle,
    job: &Job,
    cli: &Path,
    gpu: bool,
    fps: Option<f64>,
    yuv: &Path,
    sidecar: &Path,
    audio: Option<&Path>,
    ff: &Ffmpeg,
    preset: &ExportPreset,
    stop: &EncodeStop,
    mapper: &JobProgress,
    started_ms: u64,
    stall: u64,
    serve: &ServePool,
) -> Result<encodecraft_ffmpeg::EncodeResult, EngineError> {
    let JobSource::EffectCraft { project, comp, work_area, .. } = &job.source else {
        return Err(EngineError::Message("not an EffectCraft job".into()));
    };
    let server = serve.ensure(cli, gpu)?;
    let _ = server.rpc("project.open", json!({"path": project}));
    let mut params = json!({
        "project": project,
        "out": yuv,
        "pixFmt": "yuv420p",
        "sidecar": sidecar,
        "workArea": work_area,
        "quality": "best",
        "resolution": "full",
    });
    if let Some(c) = comp {
        params["comp"] = json!(c);
    }
    if let Some(a) = audio {
        params["audioOut"] = json!(a);
    }
    if let Some(f) = fps {
        params["fps"] = json!(f);
    }

    let mut header: Option<StreamHeader> = None;
    let mut pass1: Option<Pass1> = None;
    let mut offset = 0u64;
    let mut last_preview = Instant::now() - Duration::from_secs(1);
    let preview_on = h.lock(|s| s.settings.show_encode_preview);
    let mut fail: Option<String> = None;

    server.render_start(params, |ev| {
        if stop.is_requested() {
            server.cancel();
        }
        match ev {
            StreamEvent::Header(hd) => {
                h.lock(|s| {
                    if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == job.id) {
                        let _ = j.apply_resolved_size(hd.width, hd.height, s.settings.allow_overwrite);
                    }
                    s.push_log(format!("{}: {}×{} via warm serve", job.id, hd.width, hd.height));
                });
                apply_render_progress(h, &job.id, mapper, started_ms, 0, hd.frames);
                let wav = if hd.audio { audio } else { None };
                match start_pass1(ff, preset, &hd, yuv, wav, partial_output(&job.output), stall) {
                    Ok(p) => pass1 = Some(p),
                    Err(e) => {
                        fail = Some(e.to_string());
                        server.cancel();
                    }
                }
                header = Some(hd);
            }
            StreamEvent::Frame { n, total, .. } => {
                apply_render_progress(h, &job.id, mapper, started_ms, n, total);
                if header.is_none() {
                    header = load_sidecar(sidecar);
                    if let Some(hd) = header.as_ref() {
                        if pass1.is_none() {
                            let wav = if hd.audio { audio } else { None };
                            match start_pass1(ff, preset, hd, yuv, wav, partial_output(&job.output), stall) {
                                Ok(p) => pass1 = Some(p),
                                Err(e) => {
                                    fail = Some(e.to_string());
                                    server.cancel();
                                    return;
                                }
                            }
                        }
                    }
                }
                let Some(hd) = header.clone() else { return };
                let bpf = hd.bytes_per_frame();
                match read_growing_file(yuv, offset, bpf, &|| stop.is_requested(), Duration::from_secs(stall.max(30))) {
                    Ok(frame) => {
                        offset += bpf;
                        if let Some(p1) = pass1.as_mut() {
                            if let Some(ref mut sin) = p1.stdin {
                                if let Err(e) = feed_pass_stdin(sin, &frame) {
                                    fail = Some(e.to_string());
                                    server.cancel();
                                }
                            }
                        }
                        maybe_preview(h, job, &hd, &frame, preview_on, &mut last_preview);
                    }
                    Err(e) => {
                        fail = Some(e.to_string());
                        server.cancel();
                    }
                }
            }
            StreamEvent::Error { message } => {
                fail = Some(message);
                server.cancel();
            }
            StreamEvent::Cancelled { reason } => fail = Some(reason.unwrap_or_else(|| "cancelled".into())),
            StreamEvent::Gpu { used, reason } => {
                h.lock(|s| s.push_log(format!("{}: gpu used={used} {}", job.id, reason.unwrap_or_default())));
            }
            _ => {}
        }
    })?;
    if let Some(msg) = fail {
        return Err(EngineError::Message(msg));
    }
    if let Some(mut p1) = pass1 {
        if let Some(ref mut sin) = p1.stdin {
            let _ = sin.flush();
        }
        drop(p1.stdin.take());
        wait_pass1(h, job, mapper, &mut p1, stop)?;
    }
    let header = header.or_else(|| load_sidecar(sidecar)).ok_or_else(|| EngineError::Message("no stream header".into()))?;
    let audio = if header.audio { audio } else { None };
    finish_encode(h, job, ff, preset, &header, yuv, audio, stop, stall, mapper)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_render_env_overrides_settings() {
        assert!(gpu_render_from(None, true), "checkbox on");
        assert!(!gpu_render_from(None, false), "checkbox off (default)");
        assert!(!gpu_render_from(Some("0"), true));
        assert!(!gpu_render_from(Some("off"), true));
        assert!(gpu_render_from(Some("1"), false), "env forces GPU");
        assert!(gpu_render_from(Some("on"), false));
        assert!(!gpu_render_enabled(false));
    }

    #[test]
    fn yuv_preview_writes_jpeg() {
        let w = 8u32;
        let h = 8u32;
        let bpf = encodecraft_ffmpeg::yuv420p_bytes_per_frame(w, h) as usize;
        let frame = vec![128u8; bpf];
        let dir = std::env::temp_dir().join(format!("ec-prev-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let dest = dir.join("p.jpg");
        assert!(preview_jpeg_from_yuv(&frame, w, h, &dest));
        assert!(dest.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
