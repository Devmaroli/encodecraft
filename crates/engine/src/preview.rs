//! Side-channel encode preview: a few fps of scaled JPEGs, never teed into the encode.

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use encodecraft_ffmpeg::{grab_preview_jpeg, mezzanine_path, partial_output};
use encodecraft_job::JobSource;
use encodecraft_queue::{Job, JobStatus};

use crate::paths;
use crate::tasks::UiEvent;
use crate::Handle;

const MIN_GROWING_BYTES: u64 = 48_000;
const GRAB_EVERY: Duration = Duration::from_millis(500);

#[derive(Clone)]
struct PreviewTarget {
    id: String,
    name: String,
    output: PathBuf,
    source: PathBuf,
    at_sec: f64,
    out_w: u32,
    out_h: u32,
    has_video: bool,
}

pub fn should_grab_preview(enabled: bool, status: JobStatus, has_video: bool) -> bool {
    enabled && status.is_active() && has_video
}

pub fn run(h: Handle) {
    let mut last: HashMap<String, Instant> = HashMap::new();
    while h.is_alive() {
        thread::sleep(Duration::from_millis(200));
        let enabled = h.lock(|s| s.settings.show_encode_preview);
        if !enabled {
            last.clear();
            continue;
        }
        let ff = h.lock(|s| s.ffmpeg.as_ref().ok().cloned());
        let Some(ff) = ff else {
            continue;
        };
        let targets = h.lock(|s| collect_targets(&s.queue.jobs));
        let live: Vec<String> = targets.iter().map(|t| t.id.clone()).collect();
        last.retain(|id, _| live.contains(id));
        for t in targets {
            if last.get(&t.id).is_some_and(|at| at.elapsed() < GRAB_EVERY) {
                continue;
            }
            last.insert(t.id.clone(), Instant::now());
            grab_one(&h, &ff, &t);
        }
    }
}

fn collect_targets(jobs: &[Job]) -> Vec<PreviewTarget> {
    jobs.iter()
        .filter(|j| j.status.is_active())
        .map(|j| {
            let name = j.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| j.source.display_name());
            let source = match &j.source {
                JobSource::File { path, .. } => path.clone(),
                JobSource::EffectCraft { mezzanine, .. } => mezzanine_path(&paths::work_dir(), &j.id, mezzanine),
            };
            let out_w = j.preset.video.width.unwrap_or(0);
            let out_h = j.preset.video.height.unwrap_or(0);
            PreviewTarget {
                id: j.id.clone(),
                name,
                output: j.output.clone(),
                source,
                at_sec: j.trim.in_sec.max(0.0) + j.progress.out_time_sec.max(0.0),
                out_w,
                out_h,
                has_video: j.preset.format.has_video(),
            }
        })
        .collect()
}

fn grab_one(h: &Handle, ff: &encodecraft_ffmpeg::Ffmpeg, t: &PreviewTarget) {
    if !should_grab_preview(true, JobStatus::Encoding, t.has_video) {
        return;
    }
    if t.out_w < 2 || t.out_h < 2 {
        return;
    }
    let dest = paths::thumbs_dir().join(format!("preview-{}.jpg", t.id));
    let mut ok = false;
    let partial = partial_output(&t.output);
    if let Ok(meta) = std::fs::metadata(&partial) {
        if meta.len() >= MIN_GROWING_BYTES {
            ok = grab_preview_jpeg(ff, &dest, t.out_w, t.out_h, &partial, None, true).is_ok();
        }
    }
    if !ok && t.output.is_file() {
        if let Ok(meta) = std::fs::metadata(&t.output) {
            if meta.len() >= MIN_GROWING_BYTES {
                ok = grab_preview_jpeg(ff, &dest, t.out_w, t.out_h, &t.output, None, true).is_ok();
            }
        }
    }
    if !ok && t.source.is_file() {
        ok = grab_preview_jpeg(ff, &dest, t.out_w, t.out_h, &t.source, Some(t.at_sec), false).is_ok();
    }
    if !ok {
        return;
    }
    let Ok(bytes) = std::fs::read(&dest) else {
        return;
    };
    let Ok(img) = image::load_from_memory(&bytes) else {
        return;
    };
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let hgt = rgba.height();
    h.emit_ui(UiEvent::PreviewFrame {
        id: t.id.clone(),
        name: t.name.clone(),
        rgba: rgba.into_raw(),
        w,
        h: hgt,
        output_w: t.out_w,
        output_h: t.out_h,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_off_never_grabs() {
        assert!(!should_grab_preview(false, JobStatus::Encoding, true));
        assert!(should_grab_preview(true, JobStatus::Encoding, true));
        assert!(!should_grab_preview(true, JobStatus::Waiting, true));
        assert!(!should_grab_preview(true, JobStatus::Done, true));
        assert!(!should_grab_preview(true, JobStatus::Encoding, false));
    }
}
