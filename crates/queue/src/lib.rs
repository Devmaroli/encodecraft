//! Encode queue: add / reorder / duplicate / expand one source to many screen sizes.
//! Encoding itself is driven by `encodecraft-engine` so this crate stays UI-free.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use encodecraft_ffmpeg::EncodeProgress;
use encodecraft_job::{EnqueueRequest, JobError, JobSource, ScaleMode, ScreenSize, Trim};
use encodecraft_ffmpeg::h264_unencodable_message;
use encodecraft_presets::{
    apply_template, constrain_output_dir, default_template, infer_screen_from_text, output_path, unique_output_path, ExportPreset, NamingContext,
    PresetLibrary, PresetReason,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error(transparent)]
    Job(#[from] JobError),
    #[error("{0}")]
    Message(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    #[default]
    Waiting,
    Encoding,
    Paused,
    Done,
    Failed,
    Stopped,
}

impl JobStatus {
    pub fn label(self) -> &'static str {
        match self {
            JobStatus::Waiting => "Waiting",
            JobStatus::Encoding => "Encoding",
            JobStatus::Paused => "Paused",
            JobStatus::Done => "Done",
            JobStatus::Failed => "Failed",
            JobStatus::Stopped => "Stopped",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, JobStatus::Encoding)
    }

    pub fn is_queued(self) -> bool {
        matches!(self, JobStatus::Waiting | JobStatus::Paused)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub source: JobSource,
    pub preset: ExportPreset,
    pub output: PathBuf,
    pub trim: Trim,
    pub scale: ScaleMode,
    pub screen: Option<ScreenSize>,
    /// Auto-assignment reason shown in the queue (`ultra wide`, `listed screen`, …).
    #[serde(default)]
    pub preset_reason: Option<PresetReason>,
    /// User picked a different preset after auto-assignment.
    #[serde(default)]
    pub preset_overridden: bool,
    /// Non-fatal / pre-encode flag (H.264 cannot encode this raster).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encode_warning: Option<String>,
    /// Naming template used when this job was queued (so we can rebuild `{width}x{height}` later).
    #[serde(default = "default_job_naming")]
    pub naming: String,
    /// Output name still has unresolved `{width}`/`{height}` (`srcxsrc`).
    #[serde(default)]
    pub size_name_pending: bool,
    pub status: JobStatus,
    #[serde(default)]
    pub progress: ProgressSnap,
    #[serde(default)]
    pub log: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub started_ms: Option<u64>,
    #[serde(default)]
    pub finished_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProgressSnap {
    pub percent: f32,
    pub eta_sec: Option<f64>,
    pub speed: f32,
    pub fps: f32,
}

impl From<&EncodeProgress> for ProgressSnap {
    fn from(p: &EncodeProgress) -> Self {
        Self { percent: p.percent, eta_sec: p.eta_sec, speed: p.speed, fps: p.fps }
    }
}

impl Job {
    /// Fill match-source dimensions and rebuild `{width}x{height}` once the real size is known.
    /// Returns true if the output path changed.
    pub fn apply_resolved_size(&mut self, width: u32, height: u32, allow_overwrite: bool) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        let w = self.preset.video.width.filter(|n| *n > 0).unwrap_or(width);
        let h = self.preset.video.height.filter(|n| *n > 0).unwrap_or(height);
        if self.preset.video.width.unwrap_or(0) == 0 {
            self.preset.video.width = Some(w);
        }
        if self.preset.video.height.unwrap_or(0) == 0 {
            self.preset.video.height = Some(h);
        }
        self.encode_warning = h264_unencodable_message(&self.preset, w, h);
        let fname = self.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let pending = self.size_name_pending || fname.contains("srcxsrc") || fname.contains("_src.") || fname.contains("xsrc.");
        if !pending {
            return false;
        }
        let dir = self.output.parent().map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("."));
        let naming = if self.naming.is_empty() { default_template() } else { self.naming.as_str() };
        let ctx = NamingContext::from_preset(
            &self.source.display_name(),
            &self.preset,
            w,
            h,
            &self.id,
            1,
            self.screen.as_ref().map(|s| s.name.as_str()).unwrap_or(""),
        );
        let base = output_path(&dir, naming, &ctx, self.preset.format.is_sequence());
        let next = unique_output_path(base, allow_overwrite);
        if next == self.output {
            self.size_name_pending = false;
            return false;
        }
        self.output = next;
        self.size_name_pending = false;
        true
    }
}

static JOB_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn new_id(prefix: &str) -> String {
    let n = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
    let ms = unix_ms();
    format!("{prefix}-{ms:x}-{n}")
}

pub fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs().saturating_mul(1000) + u64::from(d.subsec_millis())).unwrap_or(0)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EncodeQueue {
    pub jobs: Vec<Job>,
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
    #[serde(default)]
    pub running: bool,
}

fn default_parallel() -> usize {
    2
}

fn default_job_naming() -> String {
    default_template().into()
}

impl EncodeQueue {
    pub fn new() -> Self {
        Self { jobs: Vec::new(), max_parallel: 2, running: false }
    }

    pub fn selected<'a>(&'a self, id: Option<&str>) -> Option<&'a Job> {
        let id = id?;
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn selected_mut<'a>(&'a mut self, id: Option<&str>) -> Option<&'a mut Job> {
        let id = id?;
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.jobs.iter().position(|j| j.id == id)
    }

    pub fn add_job(&mut self, job: Job) {
        self.jobs.push(job);
    }

    pub fn remove(&mut self, id: &str) -> Result<Job, QueueError> {
        let i = self.index_of(id).ok_or_else(|| QueueError::Message(format!("no job {id}")))?;
        if self.jobs.get(i).is_some_and(|j| j.status.is_active()) {
            return Err(QueueError::Message("stop the job before removing it".into()));
        }
        Ok(self.jobs.remove(i))
    }

    pub fn remove_finished(&mut self) {
        self.jobs.retain(|j| !matches!(j.status, JobStatus::Done | JobStatus::Failed | JobStatus::Stopped));
    }

    pub fn duplicate(&mut self, id: &str) -> Result<String, QueueError> {
        let i = self.index_of(id).ok_or_else(|| QueueError::Message(format!("no job {id}")))?;
        let mut copy = self.jobs[i].clone();
        copy.id = new_id("job");
        copy.status = JobStatus::Waiting;
        copy.progress = ProgressSnap::default();
        copy.log.clear();
        copy.error = None;
        copy.started_ms = None;
        copy.finished_ms = None;
        copy.created_ms = unix_ms();
        let new_id_s = copy.id.clone();
        self.jobs.insert(i + 1, copy);
        Ok(new_id_s)
    }

    pub fn move_job(&mut self, id: &str, delta: isize) -> Result<(), QueueError> {
        let i = self.index_of(id).ok_or_else(|| QueueError::Message(format!("no job {id}")))?;
        let j = if delta < 0 { i.saturating_sub(delta.unsigned_abs()) } else { i.saturating_add(delta as usize).min(self.jobs.len().saturating_sub(1)) };
        if i != j {
            self.jobs.swap(i, j);
        }
        Ok(())
    }

    pub fn reset_for_retry(&mut self, id: &str) -> Result<(), QueueError> {
        let job = self.jobs.iter_mut().find(|j| j.id == id).ok_or_else(|| QueueError::Message(format!("no job {id}")))?;
        job.status = JobStatus::Waiting;
        job.progress = ProgressSnap::default();
        job.error = None;
        job.finished_ms = None;
        Ok(())
    }

    pub fn next_waiting(&self) -> Option<String> {
        self.jobs.iter().find(|j| j.status == JobStatus::Waiting).map(|j| j.id.clone())
    }

    pub fn active_count(&self) -> usize {
        self.jobs.iter().filter(|j| j.status.is_active()).count()
    }

    pub fn slots_free(&self) -> usize {
        self.max_parallel.max(1).saturating_sub(self.active_count())
    }

    /// Build one or more jobs from an enqueue request.
    pub fn expand(
        req: &EnqueueRequest,
        lib: &PresetLibrary,
        default_out: &std::path::Path,
        default_naming: &str,
        allow_overwrite: bool,
        clamp_output: bool,
    ) -> Result<Vec<Job>, QueueError> {
        req.validate()?;
        let mut preset = lib.get_or_parse(&req.preset_id).map_err(|e| QueueError::Message(e.to_string()))?;
        if let Some(over) = &req.preset_override {
            preset = preset.merge_override(over).map_err(|e| QueueError::Message(e.to_string()))?;
        }
        let trim = req.trim.unwrap_or_default();
        let scale = req.scale.unwrap_or(preset.video.scale);
        let naming = req.naming.as_deref().unwrap_or(default_naming);
        let out_dir = if clamp_output {
            constrain_output_dir(req.output_dir.as_deref(), default_out).map_err(QueueError::Message)?
        } else {
            req.output_dir.clone().unwrap_or_else(|| default_out.to_path_buf())
        };
        let sizes: Vec<Option<ScreenSize>> = if req.sizes.is_empty() { vec![None] } else { req.sizes.iter().cloned().map(Some).collect() };

        let mut jobs = Vec::new();
        let mut claimed: Vec<PathBuf> = Vec::new();
        let source_hint = source_screen_hint(&req.source);
        let auto = lib.screen_rules.is_auto_eligible(&req.preset_id);
        for (n, screen) in sizes.into_iter().enumerate() {
            let mut screen = screen;
            if screen.is_none() {
                screen = infer_screen_from_text(&source_hint, &lib.screen_rules);
            }
            let mut pr = preset.clone();
            let mut preset_reason = None;
            let mut preset_overridden = false;
            if auto {
                if let Some(pick) = lib.screen_rules.pick(screen.as_ref(), &source_hint) {
                    if let Ok(chosen) = lib.get_or_parse(&pick.preset_id) {
                        pr = chosen;
                        preset_reason = Some(pick.reason);
                    }
                }
            } else if !req.preset_id.is_empty() {
                preset_overridden = true;
                preset_reason = Some(PresetReason::Manual);
            }
            pr.video.scale = scale;
            if let Some(sz) = &screen {
                if sz.width > 0 {
                    pr.video.width = Some(sz.width);
                }
                if sz.height > 0 {
                    pr.video.height = Some(sz.height);
                }
            }
            if pr.video.width.unwrap_or(0) == 0 || pr.video.height.unwrap_or(0) == 0 {
                if let Some((sw, sh)) = req.source.pixel_size() {
                    if pr.video.width.unwrap_or(0) == 0 {
                        pr.video.width = Some(sw);
                    }
                    if pr.video.height.unwrap_or(0) == 0 {
                        pr.video.height = Some(sh);
                    }
                }
            }
            let w = pr.video.width.unwrap_or(0);
            let h = pr.video.height.unwrap_or(0);
            let encode_warning = h264_unencodable_message(&pr, w, h);
            let id = new_id("job");
            let ctx = NamingContext::from_preset(
                &req.source.display_name(),
                &pr,
                w,
                h,
                &id,
                (n as u32).saturating_add(1),
                screen.as_ref().map(|s| s.name.as_str()).unwrap_or(""),
            );
            let base = output_path(&out_dir, naming, &ctx, pr.format.is_sequence());
            let mut output = unique_output_path(base.clone(), allow_overwrite);
            if !allow_overwrite {
                let mut n = 2u32;
                while claimed.iter().any(|p| p == &output) && n < 10_000 {
                    let parent = base.parent().unwrap_or(std::path::Path::new("."));
                    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
                    let ext = base.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                    output = unique_output_path(parent.join(format!("{stem}-{n}{ext}")), false);
                    n = n.saturating_add(1);
                }
            }
            claimed.push(output.clone());
            jobs.push(Job {
                id,
                source: req.source.clone(),
                preset: pr,
                output,
                trim,
                scale,
                screen,
                preset_reason,
                preset_overridden,
                encode_warning,
                naming: naming.to_string(),
                size_name_pending: w == 0 || h == 0,
                status: JobStatus::Waiting,
                progress: ProgressSnap::default(),
                log: String::new(),
                error: None,
                created_ms: unix_ms(),
                started_ms: None,
                finished_ms: None,
            });
        }
        Ok(jobs)
    }

    pub fn enqueue(
        &mut self,
        req: &EnqueueRequest,
        lib: &PresetLibrary,
        default_out: &std::path::Path,
        default_naming: &str,
        allow_overwrite: bool,
        clamp_output: bool,
    ) -> Result<Vec<String>, QueueError> {
        let jobs = Self::expand(req, lib, default_out, default_naming, allow_overwrite, clamp_output)?;
        let ids: Vec<String> = jobs.iter().map(|j| j.id.clone()).collect();
        self.jobs.extend(jobs);
        if req.start_queue {
            self.running = true;
        }
        Ok(ids)
    }

    pub fn persist_view(&self) -> Self {
        let mut q = self.clone();
        for j in &mut q.jobs {
            if j.status.is_active() {
                j.status = JobStatus::Waiting;
                j.progress = ProgressSnap::default();
            }
            if j.log.len() > 16_384 {
                j.log = j.log[j.log.len() - 16_384..].to_string();
            }
        }
        q
    }
}

fn source_screen_hint(source: &JobSource) -> String {
    let mut bits = vec![source.display_name()];
    match source {
        JobSource::File { path, .. } => {
            if let Some(n) = path.file_name() {
                bits.push(n.to_string_lossy().into_owned());
            }
            bits.push(path.to_string_lossy().into_owned());
        }
        JobSource::EffectCraft { project, comp, .. } => {
            if let Some(c) = comp {
                bits.push(c.clone());
            }
            if let Some(n) = project.file_name() {
                bits.push(n.to_string_lossy().into_owned());
            }
        }
    }
    bits.join(" ")
}

pub fn preview_name(source: &JobSource, preset: &ExportPreset, w: u32, h: u32, template: &str) -> String {
    let ctx = NamingContext::from_preset(&source.display_name(), preset, w, h, "preview", 1, "");
    apply_template(if template.is_empty() { default_template() } else { template }, &ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use encodecraft_job::{EnqueueRequest, JobSource};
    use std::path::PathBuf;

    fn req_many() -> EnqueueRequest {
        EnqueueRequest {
            schema: 1,
            source: JobSource::file("/ads/hero.mp4"),
            preset_id: "system.h264-mp4".into(),
            preset_override: None,
            output_dir: Some(PathBuf::from("/out")),
            naming: Some("{name}_{width}x{height}".into()),
            sizes: vec![ScreenSize::new("A", 1920, 1080), ScreenSize::new("B", 1080, 1920), ScreenSize::new("C", 1200, 960), ScreenSize::new("D", 1080, 864)],
            trim: None,
            scale: Some(ScaleMode::Fit),
            start_queue: false,
        }
    }

    #[test]
    fn one_source_many_sizes() {
        let lib = PresetLibrary::new();
        let jobs = EncodeQueue::expand(&req_many(), &lib, std::path::Path::new("/out"), "{name}", false, true).unwrap();
        assert_eq!(jobs.len(), 4);
        assert!(jobs[0].output.to_string_lossy().contains("hero_1920x1080"));
        assert!(jobs[1].output.to_string_lossy().contains("1080x1920"));
        assert!(jobs[2].output.to_string_lossy().contains("1200x960"));
        assert_eq!(jobs[3].preset.video.height, Some(864));
    }

    #[test]
    fn reorder_and_duplicate() {
        let lib = PresetLibrary::new();
        let mut q = EncodeQueue::new();
        q.enqueue(&req_many(), &lib, std::path::Path::new("/out"), "{name}", false, true).unwrap();
        let first = q.jobs[0].id.clone();
        let last = q.jobs[3].id.clone();
        q.move_job(&first, 1).unwrap();
        assert_eq!(q.jobs[1].id, first);
        let dup = q.duplicate(&last).unwrap();
        assert_eq!(q.jobs.len(), 5);
        assert_eq!(q.jobs[4].id, dup);
        assert_eq!(q.jobs[4].status, JobStatus::Waiting);
        q.remove(&dup).unwrap();
        assert_eq!(q.jobs.len(), 4);
    }

    #[test]
    fn rejects_output_dir_escape() {
        let lib = PresetLibrary::new();
        let mut req = req_many();
        req.output_dir = Some(PathBuf::from("/etc"));
        let err = EncodeQueue::expand(&req, &lib, std::path::Path::new("/out"), "{name}", false, true).unwrap_err();
        assert!(err.to_string().contains("inside"));
    }

    #[test]
    fn auto_assigns_studio_presets_per_screen() {
        let lib = PresetLibrary::new();
        let mut req = req_many();
        req.preset_id = "system.h264-mp4".into();
        req.sizes = vec![
            ScreenSize::new("The Avenues Gate", 1920, 1080),
            ScreenSize::new("1.7HD", 1920, 1080),
            ScreenSize::new("The Avenues Grand Avenues Entrance", 1920, 1080),
            ScreenSize::new("1st Ring Road", 1008, 5184),
            ScreenSize::new("Al Nassar Tower Vertical", 960, 5328),
            ScreenSize::new("Kuwait Gate", 4460, 378),
        ];
        let jobs = EncodeQueue::expand(&req, &lib, std::path::Path::new("/out"), "{name}", false, true).unwrap();
        assert_eq!(jobs[0].preset.id, "system.better-res");
        assert_eq!(jobs[0].preset_reason, Some(PresetReason::UltraWide));
        assert_eq!(jobs[1].preset.id, "system.installation");
        assert_eq!(jobs[1].preset_reason, Some(PresetReason::OtherScreen));
        assert_eq!(jobs[2].preset.id, "system.better-res");
        assert_eq!(jobs[2].preset_reason, Some(PresetReason::ListedScreen));
        assert_eq!(jobs[3].preset.id, "system.better-res");
        assert_eq!(jobs[3].preset_reason, Some(PresetReason::ListedScreen));
        assert!(jobs[3].encode_warning.is_none());
        assert_eq!(jobs[4].preset.id, "system.better-res");
        assert_eq!(jobs[5].preset_reason, Some(PresetReason::UltraWide));
        assert_eq!(jobs[5].preset.id, "system.better-res");
    }

    #[test]
    fn hevc_request_is_not_auto_replaced() {
        let lib = PresetLibrary::new();
        let mut req = req_many();
        req.preset_id = "system.hevc-mp4".into();
        req.sizes = vec![ScreenSize::new("The Avenues Gate", 1920, 1080)];
        let jobs = EncodeQueue::expand(&req, &lib, std::path::Path::new("/out"), "{name}", false, true).unwrap();
        assert_eq!(jobs[0].preset.id, "system.hevc-mp4");
        assert!(jobs[0].preset_overridden);
    }

    #[test]
    fn huge_h264_canvas_is_flagged() {
        let lib = PresetLibrary::new();
        let mut req = req_many();
        req.preset_id = "system.better-res".into();
        req.sizes = vec![ScreenSize::new("Impossible", 32000, 32000)];
        let jobs = EncodeQueue::expand(&req, &lib, std::path::Path::new("/out"), "{name}", false, true).unwrap();
        let msg = jobs[0].encode_warning.as_deref().unwrap_or("");
        assert!(msg.contains("H.264 cannot encode"), "{msg}");
    }

    #[test]
    fn match_source_uses_effectcraft_comp_size_in_name() {
        let lib = PresetLibrary::new();
        let req = EnqueueRequest {
            schema: 1,
            source: JobSource::EffectCraft {
                project: PathBuf::from("/p/test-2.ecproj"),
                comp: Some("Honor 400_EN.ai Comp".into()),
                mezzanine: "prores".into(),
                work_area: false,
                width: Some(1080),
                height: Some(1080),
            },
            preset_id: "system.h264-mp4".into(),
            preset_override: None,
            output_dir: Some(PathBuf::from("/out")),
            naming: Some("{name}_{width}x{height}".into()),
            sizes: Vec::new(),
            trim: None,
            scale: None,
            start_queue: false,
        };
        let jobs = EncodeQueue::expand(&req, &lib, std::path::Path::new("/out"), "{name}_{width}x{height}", false, true).unwrap();
        let name = jobs[0].output.file_name().unwrap().to_string_lossy();
        assert!(name.contains("1080x1080"), "{name}");
        assert!(!name.contains("srcxsrc"), "{name}");
        assert!(!jobs[0].size_name_pending);
        assert_eq!(jobs[0].preset.video.width, Some(1080));
        assert_eq!(jobs[0].preset.video.height, Some(1080));
    }

    #[test]
    fn match_source_placeholder_renamed_after_probe() {
        let lib = PresetLibrary::new();
        let req = EnqueueRequest {
            schema: 1,
            source: JobSource::EffectCraft {
                project: PathBuf::from("/p/test-2.ecproj"),
                comp: Some("Honor 400_EN.ai Comp".into()),
                mezzanine: "prores".into(),
                work_area: false,
                width: None,
                height: None,
            },
            preset_id: "system.h264-mp4".into(),
            preset_override: None,
            output_dir: Some(PathBuf::from("/out")),
            naming: Some("{name}_{width}x{height}".into()),
            sizes: Vec::new(),
            trim: None,
            scale: None,
            start_queue: false,
        };
        let mut jobs = EncodeQueue::expand(&req, &lib, std::path::Path::new("/out"), "{name}_{width}x{height}", false, true).unwrap();
        let before = jobs[0].output.file_name().unwrap().to_string_lossy().into_owned();
        assert!(before.contains("srcxsrc"), "{before}");
        assert!(jobs[0].size_name_pending);
        assert!(jobs[0].apply_resolved_size(1080, 1080, true));
        let after = jobs[0].output.file_name().unwrap().to_string_lossy().into_owned();
        assert!(after.contains("1080x1080"), "{after}");
        assert!(!after.contains("srcxsrc"), "{after}");
        assert!(!jobs[0].size_name_pending);
    }
}
