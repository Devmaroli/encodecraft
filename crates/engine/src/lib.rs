//! Session façade: queue + presets + settings + watch folders + IPC.
//!
//! UI, CLI and the EffectCraft handoff all talk to [`Handle`].

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod drops;
mod fsutil;
mod ipc;
mod paths;
mod persist;
mod token;
mod watch;
mod worker;

pub use drops::{classify_dropped_paths, list_ecproj_compositions, list_ecproj_compositions_file, DropOutcome, SequenceDrop};
pub use ipc::{default_port, post_enqueue, try_connect, IpcServer};
pub use paths::{data_dir, inbox_dir, set_home};
pub use persist::Settings;
pub use watch::WatchFolder;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use encodecraft_ffmpeg::{find_ffmpeg_at, probe, thumbnail_png, EncodeStop, Ffmpeg, FfmpegError};
use encodecraft_job::{ControlReply, ControlRequest, EnqueueRequest, JobSource};
use encodecraft_presets::{default_output_dir, default_template, ExportPreset, PresetLibrary};
use encodecraft_queue::{EncodeQueue, Job, JobStatus, QueueError};
use serde_json::{json, Value};

pub type Shared = Arc<Mutex<Session>>;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Queue(#[from] QueueError),
    #[error(transparent)]
    Ffmpeg(#[from] FfmpegError),
    #[error("{0}")]
    Message(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct Handle {
    pub session: Shared,
    stops: Arc<Mutex<HashMap<String, EncodeStop>>>,
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        Self { session: Arc::clone(&self.session), stops: Arc::clone(&self.stops) }
    }
}

impl Handle {
    pub fn open() -> Result<Self, EngineError> {
        let h = Self::open_core()?;
        h.spawn_workers();
        Ok(h)
    }

    /// Load state without background workers (unit tests, one-shot CLI helpers).
    pub fn open_core() -> Result<Self, EngineError> {
        paths::ensure_dirs()?;
        let session = Session::load()?;
        Ok(Handle { session: Arc::new(Mutex::new(session)), stops: Arc::new(Mutex::new(HashMap::new())) })
    }

    pub fn open_in(dir: PathBuf) -> Result<Self, EngineError> {
        paths::set_home(dir);
        Self::open()
    }

    pub fn lock<R>(&self, f: impl FnOnce(&mut Session) -> R) -> R {
        let mut g = self.session.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut g)
    }

    pub fn persist(&self) {
        self.lock(|s| {
            let _ = s.save();
        });
    }

    pub fn enqueue(&self, req: EnqueueRequest) -> Result<Vec<String>, EngineError> {
        // Untrusted (HTTP / inbox): keep output inside the configured folder.
        self.enqueue_with(req, true)
    }

    pub fn enqueue_trusted(&self, req: EnqueueRequest) -> Result<Vec<String>, EngineError> {
        self.enqueue_with(req, false)
    }

    fn enqueue_with(&self, req: EnqueueRequest, clamp_output: bool) -> Result<Vec<String>, EngineError> {
        self.lock(|s| {
            let ids = s.queue.enqueue(&req, &s.presets, &s.settings.output_dir, &s.settings.naming, s.settings.allow_overwrite, clamp_output)?;
            let _ = s.save();
            Ok(ids)
        })
    }

    pub fn execute_json(&self, req: ControlRequest) -> ControlReply {
        match self.dispatch(&req.method, &req.params) {
            Ok(result) => ControlReply { id: req.id, ok: true, result: Some(result), error: None },
            Err(e) => ControlReply { id: req.id, ok: false, result: None, error: Some(e.to_string()) },
        }
    }

    pub fn dispatch(&self, method: &str, params: &Value) -> Result<Value, EngineError> {
        match method {
            "enqueue" | "queue.enqueue" => {
                let req: EnqueueRequest = serde_json::from_value(params.clone()).map_err(|e| EngineError::Message(e.to_string()))?;
                req.validate().map_err(|e| EngineError::Message(e.to_string()))?;
                let ids = self.enqueue(req)?;
                Ok(json!({ "ids": ids }))
            }
            "queue.start" => {
                self.lock(|s| s.queue.running = true);
                Ok(json!({ "running": true }))
            }
            "queue.pause" => {
                self.lock(|s| s.queue.running = false);
                Ok(json!({ "running": false }))
            }
            "queue.stop" => {
                self.stop_all();
                Ok(json!({ "stopped": true }))
            }
            "queue.list" => self.lock(|s| serde_json::to_value(&s.queue.jobs).map_err(|e| EngineError::Message(e.to_string()))),
            "queue.remove" => {
                let id = params.get("id").and_then(Value::as_str).ok_or_else(|| EngineError::Message("id required".into()))?;
                self.lock(|s| s.queue.remove(id))?;
                Ok(json!({ "ok": true }))
            }
            "queue.duplicate" => {
                let id = params.get("id").and_then(Value::as_str).ok_or_else(|| EngineError::Message("id required".into()))?;
                let new_id = self.lock(|s| s.queue.duplicate(id))?;
                Ok(json!({ "id": new_id }))
            }
            "ffmpeg.status" => {
                let v = self.lock(|s| match &s.ffmpeg {
                    Ok(f) => json!({"ok": true, "message": f.ok_message()}),
                    Err(e) => json!({"ok": false, "message": e.to_string()}),
                });
                Ok(v)
            }
            "presets.list" => self.lock(|s| serde_json::to_value(s.presets.all()).map_err(|e| EngineError::Message(e.to_string()))),
            "ping" => Ok(json!({ "pong": true, "product": encodecraft_job::PRODUCT })),
            other => Err(EngineError::Message(format!("unknown method `{other}`"))),
        }
    }

    pub fn start_ipc(&self, port: u16) -> Result<IpcServer, EngineError> {
        IpcServer::bind(port, self.clone())
    }

    pub fn stop_job(&self, id: &str) {
        if let Ok(g) = self.stops.lock() {
            if let Some(s) = g.get(id) {
                s.request();
            }
        }
        self.lock(|s| {
            if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == id) {
                if j.status.is_active() {
                    j.status = JobStatus::Stopped;
                    j.finished_ms = Some(encodecraft_queue::unix_ms());
                }
            }
        });
    }

    pub fn stop_all(&self) {
        self.lock(|s| s.queue.running = false);
        if let Ok(g) = self.stops.lock() {
            for s in g.values() {
                s.request();
            }
        }
        self.lock(|s| {
            for j in &mut s.queue.jobs {
                if j.status.is_active() {
                    j.status = JobStatus::Stopped;
                    j.finished_ms = Some(encodecraft_queue::unix_ms());
                } else if j.status == JobStatus::Waiting {
                    j.status = JobStatus::Stopped;
                }
            }
        });
    }

    fn spawn_workers(&self) {
        let h = self.clone();
        thread::Builder::new().name("encodecraft-worker".into()).spawn(move || worker::run(h)).ok();
        let h = self.clone();
        thread::Builder::new().name("encodecraft-watch".into()).spawn(move || watch::run(h)).ok();
        let h = self.clone();
        thread::Builder::new().name("encodecraft-inbox".into()).spawn(move || drain_inbox_loop(h)).ok();
    }

    pub fn register_stop(&self, id: String, stop: EncodeStop) {
        if let Ok(mut g) = self.stops.lock() {
            g.insert(id, stop);
        }
    }

    pub fn unregister_stop(&self, id: &str) {
        if let Ok(mut g) = self.stops.lock() {
            g.remove(id);
        }
    }
}

pub struct Session {
    pub queue: EncodeQueue,
    pub presets: PresetLibrary,
    pub settings: Settings,
    pub ffmpeg: Result<Ffmpeg, FfmpegError>,
    pub log: Vec<String>,
    pub selected: Option<String>,
    pub ipc_token: String,
}

impl Session {
    pub fn load() -> Result<Self, EngineError> {
        paths::ensure_dirs()?;
        let settings = Settings::load(&paths::settings_path());
        let presets = PresetLibrary::load(&paths::presets_path()).unwrap_or_else(|_| PresetLibrary::new());
        let queue = persist::load_queue(&paths::queue_path());
        let ffmpeg = find_ffmpeg_at(settings.ffmpeg_path.as_deref());
        let ipc_token = token::load_or_create().unwrap_or_default();
        Ok(Self { queue, presets, settings, ffmpeg, log: Vec::new(), selected: None, ipc_token })
    }

    pub fn save(&self) -> Result<(), EngineError> {
        paths::ensure_dirs()?;
        self.settings.save(&paths::settings_path())?;
        self.presets.save(&paths::presets_path()).map_err(|e| EngineError::Message(e.to_string()))?;
        persist::save_queue(&paths::queue_path(), &self.queue)?;
        Ok(())
    }

    pub fn push_log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 300 {
            let extra = self.log.len() - 200;
            self.log.drain(0..extra);
        }
    }

    pub fn add_files(&mut self, paths: &[PathBuf], preset_id: &str) -> Result<Vec<String>, EngineError> {
        let mut ids = Vec::new();
        for p in paths {
            ids.extend(self.enqueue_user_source(JobSource::file(p.clone()), preset_id)?);
        }
        if let Some(last) = ids.last() {
            self.selected = Some(last.clone());
        }
        Ok(ids)
    }

    /// User-initiated add (picker / drag-and-drop). Output stays in Settings ▸ output folder.
    pub fn enqueue_user_source(&mut self, source: JobSource, preset_id: &str) -> Result<Vec<String>, EngineError> {
        let req = EnqueueRequest {
            schema: 1,
            source,
            preset_id: preset_id.to_string(),
            preset_override: None,
            output_dir: Some(self.settings.output_dir.clone()),
            naming: Some(self.settings.naming.clone()),
            sizes: Vec::new(),
            trim: None,
            scale: None,
            start_queue: false,
        };
        self.queue.enqueue(&req, &self.presets, &self.settings.output_dir, &self.settings.naming, self.settings.allow_overwrite, false).map_err(Into::into)
    }

    pub fn add_drop_outcome(&mut self, outcome: &DropOutcome, preset_id: &str) -> Result<Vec<String>, EngineError> {
        let mut ids = Vec::new();
        for p in &outcome.media {
            ids.extend(self.enqueue_user_source(JobSource::file(p.clone()), preset_id)?);
        }
        for seq in &outcome.sequences {
            ids.extend(self.enqueue_user_source(JobSource::image_sequence(seq.first_frame.clone(), seq.start), preset_id)?);
        }
        if let Some(last) = ids.last() {
            self.selected = Some(last.clone());
        }
        Ok(ids)
    }

    pub fn add_effectcraft_comps(&mut self, project: PathBuf, comps: &[String], preset_id: &str) -> Result<Vec<String>, EngineError> {
        let chosen: Vec<Option<String>> = if comps.is_empty() { vec![None] } else { comps.iter().cloned().map(Some).collect() };
        let mut ids = Vec::new();
        for comp in chosen {
            let source = JobSource::EffectCraft { project: project.clone(), comp, mezzanine: "prores".into(), work_area: false };
            ids.extend(self.enqueue_user_source(source, preset_id)?);
        }
        if let Some(last) = ids.last() {
            self.selected = Some(last.clone());
        }
        Ok(ids)
    }

    pub fn add_files_to_sizes(&mut self, paths: &[PathBuf], preset_id: &str, sizes: Vec<encodecraft_job::ScreenSize>) -> Result<Vec<String>, EngineError> {
        let mut ids = Vec::new();
        for p in paths {
            let req = EnqueueRequest {
                schema: 1,
                source: encodecraft_job::JobSource::file(p.clone()),
                preset_id: preset_id.to_string(),
                preset_override: None,
                output_dir: Some(self.settings.output_dir.clone()),
                naming: Some(self.settings.naming.clone()),
                sizes: sizes.clone(),
                trim: None,
                scale: None,
                start_queue: false,
            };
            ids.extend(self.queue.enqueue(&req, &self.presets, &self.settings.output_dir, &self.settings.naming, self.settings.allow_overwrite, false)?);
        }
        Ok(ids)
    }

    pub fn probe_job(&self, job: &Job) -> Option<encodecraft_ffmpeg::MediaInfo> {
        let ff = self.ffmpeg.as_ref().ok()?;
        match &job.source {
            JobSource::File { path, .. } => probe(ff, path).ok(),
            JobSource::EffectCraft { .. } => None,
        }
    }

    pub fn thumbnail(&self, job: &Job) -> Option<PathBuf> {
        let ff = self.ffmpeg.as_ref().ok()?;
        let src = match &job.source {
            JobSource::File { path, .. } => path,
            _ => return None,
        };
        let dest = paths::thumbs_dir().join(format!("{}.jpg", job.id));
        if dest.exists() {
            return Some(dest);
        }
        thumbnail_png(ff, src, &dest, job.trim.in_sec.max(0.1)).ok()?;
        Some(dest)
    }
}

fn drain_inbox_loop(h: Handle) {
    loop {
        drain_inbox(&h);
        thread::sleep(Duration::from_millis(750));
    }
}

fn drain_inbox(h: &Handle) {
    let dir = paths::inbox_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json")).collect();
    files.sort();
    for path in files {
        if fsutil::is_symlink(&path) {
            h.lock(|s| s.push_log(format!("inbox skipped symlink {}", path.display())));
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() == 0 {
            continue;
        }
        if meta.len() > encodecraft_job::MAX_CONTROL_BODY as u64 {
            h.lock(|s| s.push_log(format!("inbox too large {}", path.display())));
            let bad = path.with_extension("json.bad");
            let _ = std::fs::rename(&path, bad);
            continue;
        }
        if !fsutil::is_stable(&path, Duration::from_millis(200)) {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        match EnqueueRequest::from_json(&bytes) {
            Ok(req) => {
                let _ = h.enqueue(req);
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => {
                h.lock(|s| s.push_log(format!("inbox {}: {e}", path.display())));
                let bad = path.with_extension("json.bad");
                let _ = std::fs::rename(&path, bad);
            }
        }
    }
}

/// Write a job into the inbox (used by the CLI when the app is not running).
pub fn drop_inbox(req: &EnqueueRequest) -> Result<PathBuf, EngineError> {
    paths::ensure_dirs()?;
    let path = paths::inbox_dir().join(format!("{}-{}.json", encodecraft_queue::unix_ms(), encodecraft_queue::new_id("in")));
    fsutil::atomic_write(&path, &req.to_vec().map_err(|e| EngineError::Message(e.to_string()))?)?;
    Ok(path)
}

pub fn write_lock(port: u16) -> Result<(), EngineError> {
    paths::ensure_dirs()?;
    let body = json!({ "pid": std::process::id(), "port": port, "product": encodecraft_job::PRODUCT });
    std::fs::write(paths::lock_path(), serde_json::to_vec_pretty(&body)?)?;
    Ok(())
}

pub fn read_lock_port() -> Option<u16> {
    let bytes = std::fs::read(paths::lock_path()).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("port")?.as_u64().map(|n| n as u16)
}

pub fn clear_lock() {
    let _ = std::fs::remove_file(paths::lock_path());
}

/// Default output folder helper for callers that have not loaded settings yet.
pub fn fallback_output() -> PathBuf {
    default_output_dir()
}

pub fn fallback_naming() -> String {
    default_template().into()
}

pub fn default_preset_id() -> &'static str {
    "system.h264-mp4"
}

/// Snapshot a job for the UI without holding the session lock for long.
pub fn job_copy(h: &Handle, id: &str) -> Option<Job> {
    h.lock(|s| s.queue.jobs.iter().find(|j| j.id == id).cloned())
}

pub fn ffmpeg_banner(s: &Session) -> String {
    match &s.ffmpeg {
        Ok(f) => f.ok_message(),
        Err(e) => e.to_string(),
    }
}

impl Settings {
    pub fn default_preset<'a>(&self, lib: &'a PresetLibrary) -> Result<ExportPreset, EngineError> {
        lib.get_or_parse(&self.default_preset_id).map_err(|e| EngineError::Message(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use encodecraft_job::{JobSource, ScreenSize};

    fn temp_home() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "encodecraft-eng-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    #[test]
    fn enqueue_via_handle() {
        let _guard = paths::lock_home();
        let home = temp_home();
        paths::set_home(home.clone());
        let h = Handle::open_core().unwrap();
        h.lock(|s| s.settings.output_dir = home.join("out"));
        let ids = h
            .enqueue(EnqueueRequest {
                schema: 1,
                source: JobSource::file("/x/ad.mp4"),
                preset_id: "h264".into(),
                preset_override: None,
                output_dir: Some(home.join("out").join("dooh")),
                naming: Some("{name}_{width}x{height}".into()),
                sizes: vec![ScreenSize::new("L", 1920, 1080), ScreenSize::new("P", 1080, 1920)],
                trim: None,
                scale: None,
                start_queue: false,
            })
            .unwrap();
        assert_eq!(ids.len(), 2);
        let n = h.lock(|s| s.queue.jobs.len());
        assert_eq!(n, 2);
        let escape = h.enqueue(EnqueueRequest {
            schema: 1,
            source: JobSource::file("/x/ad.mp4"),
            preset_id: "h264".into(),
            preset_override: None,
            output_dir: Some(PathBuf::from("/etc")),
            naming: None,
            sizes: vec![],
            trim: None,
            scale: None,
            start_queue: false,
        });
        assert!(escape.is_err());
    }

    #[test]
    fn inbox_drop_parses() {
        let _guard = paths::lock_home();
        let home = temp_home();
        paths::set_home(home.clone());
        let req = EnqueueRequest {
            schema: 1,
            source: JobSource::file("clip.mp4"),
            preset_id: "system.gif".into(),
            preset_override: None,
            output_dir: None,
            naming: None,
            sizes: vec![],
            trim: None,
            scale: None,
            start_queue: false,
        };
        let p = drop_inbox(&req).unwrap();
        assert!(p.exists());
        let back = EnqueueRequest::from_json(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(back.preset_id, "system.gif");
    }

    #[test]
    fn drop_effectcraft_jobs_use_comp_names_and_stay_in_output_dir() {
        let _guard = paths::lock_home();
        let home = temp_home();
        paths::set_home(home.clone());
        let h = Handle::open_core().unwrap();
        let out = home.join("exports");
        std::fs::create_dir_all(&out).unwrap();
        h.lock(|s| s.settings.output_dir = out.clone());
        let project = home.join("spot.ecproj");
        std::fs::write(&project, br#"{"compositions":[{"name":"Main"}]}"#).unwrap();
        let ids = h.lock(|s| s.add_effectcraft_comps(project.clone(), &["Main".into(), "Lower".into()], "system.h264-mp4")).unwrap();
        assert_eq!(ids.len(), 2);
        h.lock(|s| {
            assert!(matches!(&s.queue.jobs[0].source, JobSource::EffectCraft { comp: Some(c), .. } if c == "Main"));
            assert!(matches!(&s.queue.jobs[1].source, JobSource::EffectCraft { comp: Some(c), .. } if c == "Lower"));
            for j in &s.queue.jobs {
                assert!(j.output.starts_with(&out), "{}", j.output.display());
            }
        });
    }

    #[test]
    fn drop_sequence_enqueues_one_job() {
        let _guard = paths::lock_home();
        let home = temp_home();
        paths::set_home(home.clone());
        let h = Handle::open_core().unwrap();
        h.lock(|s| s.settings.output_dir = home.join("out"));
        let seq = SequenceDrop { first_frame: home.join("shot_0001.png"), start: 1, frames: 3 };
        let outcome = DropOutcome { sequences: vec![seq], ..DropOutcome::default() };
        let ids = h.lock(|s| s.add_drop_outcome(&outcome, "system.h264-mp4")).unwrap();
        assert_eq!(ids.len(), 1);
        h.lock(|s| match &s.queue.jobs[0].source {
            JobSource::File { sequence_start, .. } => assert_eq!(*sequence_start, Some(1)),
            JobSource::EffectCraft { .. } => panic!("expected file sequence"),
        });
    }
}
