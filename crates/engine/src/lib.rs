//! Session façade: queue + presets + settings + watch folders + IPC.
//!
//! UI, CLI and the EffectCraft handoff all talk to [`Handle`].

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod demo;
mod drops;
mod fsutil;
mod ipc;
mod paths;
mod persist;
mod preview;
mod tasks;
mod token;
mod watch;
mod worker;

pub use tasks::{IoCmd, UiEvent};
pub use drops::{
    classify_dropped_paths, ecproj_comp_size, ecproj_comp_size_file, list_ecproj_compositions, list_ecproj_compositions_file, DropOutcome, SequenceDrop,
};
pub use ipc::{default_port, post_enqueue, post_start, try_connect, IpcServer};
pub use paths::{data_dir, inbox_dir, layout_path, set_home};
pub use demo::{seed_preset_ui_demo, seed_stress_queue};
pub use persist::Settings;
pub use watch::WatchFolder;

use std::collections::HashMap;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use encodecraft_ffmpeg::{find_ffmpeg_at, probe, thumbnail_png, EncodeStop, Ffmpeg, FfmpegError};
use encodecraft_job::{ControlReply, ControlRequest, EnqueueRequest, JobSource};
use encodecraft_presets::{constrain_output_dir, default_output_dir, default_template, ExportPreset, PresetLibrary};
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
    alive: Arc<AtomicBool>,
    ipc_port: Arc<Mutex<Option<u16>>>,
    dirty: Arc<AtomicBool>,
    rev: Arc<AtomicU64>,
    io: Arc<Mutex<Option<tasks::IoHub>>>,
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        Self {
            session: Arc::clone(&self.session),
            stops: Arc::clone(&self.stops),
            alive: Arc::clone(&self.alive),
            ipc_port: Arc::clone(&self.ipc_port),
            dirty: Arc::clone(&self.dirty),
            rev: Arc::clone(&self.rev),
            io: Arc::clone(&self.io),
        }
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
        Ok(Handle {
            session: Arc::new(Mutex::new(session)),
            stops: Arc::new(Mutex::new(HashMap::new())),
            alive: Arc::new(AtomicBool::new(true)),
            ipc_port: Arc::new(Mutex::new(None)),
            dirty: Arc::new(AtomicBool::new(false)),
            rev: Arc::new(AtomicU64::new(1)),
            io: Arc::new(Mutex::new(None)),
        })
    }

    pub fn open_in(dir: PathBuf) -> Result<Self, EngineError> {
        paths::set_home(dir);
        Self::open()
    }

    pub fn lock<R>(&self, f: impl FnOnce(&mut Session) -> R) -> R {
        let mut g = self.session.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut g)
    }

    /// Snapshot under the lock, then write JSON on this thread. Call from the I/O
    /// worker — not from `egui` update. Does not clear [`Self::dirty`]; the caller
    /// should have used [`Self::take_dirty`].
    pub fn persist(&self) {
        let (settings, presets, queue) = self.lock(|s| (s.settings.clone(), s.presets.clone(), s.queue.clone()));
        let _ = paths::ensure_dirs();
        let _ = settings.save(&paths::settings_path());
        let _ = presets.save(&paths::presets_path());
        let _ = persist::save_queue(&paths::queue_path(), &queue);
    }

    /// Mark the session changed so the UI snapshot refreshes and a debounced disk
    /// write runs on the I/O thread (≤ ~200 ms). Never writes from the UI thread.
    pub fn mark_dirty(&self) {
        self.note_change();
    }

    pub fn note_change(&self) {
        self.rev.fetch_add(1, Ordering::Relaxed);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn revision(&self) -> u64 {
        self.rev.load(Ordering::Relaxed)
    }

    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    pub fn submit_io(&self, cmd: tasks::IoCmd) {
        if let Ok(g) = self.io.lock() {
            if let Some(hub) = g.as_ref() {
                let _ = hub.tx.send(cmd);
            }
        }
    }

    pub fn poll_ui_events(&self) -> Vec<tasks::UiEvent> {
        if let Ok(g) = self.io.lock() {
            if let Some(hub) = g.as_ref() {
                return hub.poll();
            }
        }
        Vec::new()
    }

    pub fn emit_ui(&self, ev: tasks::UiEvent) {
        if let Ok(g) = self.io.lock() {
            if let Some(hub) = g.as_ref() {
                let _ = hub.events.send(ev);
            }
        }
    }

    pub fn schedule_probe(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        self.submit_io(tasks::IoCmd::Probe(ids));
    }

    pub fn enqueue(&self, req: EnqueueRequest) -> Result<Vec<String>, EngineError> {
        // Untrusted (HTTP / inbox): keep output inside the configured folder.
        self.enqueue_with(req, true)
    }

    pub fn enqueue_trusted(&self, req: EnqueueRequest) -> Result<Vec<String>, EngineError> {
        self.enqueue_with(req, false)
    }

    fn enqueue_with(&self, mut req: EnqueueRequest, clamp_output: bool) -> Result<Vec<String>, EngineError> {
        let ids = self.lock(|s| {
            let configured = s.resolve_output_dir(&req.source, None);
            let dest = if clamp_output {
                constrain_output_dir(req.output_dir.as_deref(), &configured).map_err(EngineError::Message)?
            } else {
                s.resolve_output_dir(&req.source, req.output_dir.as_deref())
            };
            let _ = std::fs::create_dir_all(&dest);
            req.output_dir = Some(dest.clone());
            let start = s.settings.auto_start_queue && req.start_queue;
            req.start_queue = start;
            let ids = s.queue.enqueue(&req, &s.presets, &dest, &s.settings.naming, s.settings.allow_overwrite, clamp_output)?;
            s.resolve_pending_output_names(&ids);
            Ok::<_, EngineError>(ids)
        })?;
        self.note_change();
        self.schedule_probe(ids.clone());
        Ok(ids)
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
                let running = self.lock(|s| s.queue.running);
                let n = ids.len();
                let message = if running {
                    format!("{n} job{} added", if n == 1 { "" } else { "s" })
                } else {
                    format!("{n} job{} added · Start", if n == 1 { "" } else { "s" })
                };
                Ok(json!({ "ids": ids, "added": n, "running": running, "message": message }))
            }
            "queue.start" => {
                self.lock(|s| s.queue.start());
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
        let srv = IpcServer::bind(port, self.clone())?;
        if let Ok(mut g) = self.ipc_port.lock() {
            *g = Some(srv.port);
        }
        Ok(srv)
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Persist, stop encodes, and unblock background loops so the process can exit.
    pub fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.stop_all();
        self.persist();
        if let Ok(g) = self.ipc_port.lock() {
            if let Some(port) = *g {
                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
                let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(250));
            }
        }
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

    pub fn stop_and_remove(&self, ids: &[String]) -> Vec<encodecraft_queue::Job> {
        for id in ids {
            self.stop_job(id);
        }
        self.lock(|s| s.queue.take_jobs(ids))
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
        let hub = tasks::start(self.clone());
        if let Ok(mut g) = self.io.lock() {
            *g = Some(hub);
        }
        let h = self.clone();
        thread::Builder::new().name("encodecraft-worker".into()).spawn(move || worker::run(h)).ok();
        let h = self.clone();
        thread::Builder::new().name("encodecraft-preview".into()).spawn(move || preview::run(h)).ok();
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

    pub fn resolve_output_dir(&self, source: &JobSource, requested: Option<&Path>) -> PathBuf {
        if let Some(p) = requested {
            if !p.as_os_str().is_empty() {
                return p.to_path_buf();
            }
        }
        if self.settings.output_beside_source {
            source.sidecar_output_dir()
        } else {
            self.settings.output_dir.clone()
        }
    }

    /// Resolve the destination and create it so the queue path is clickable immediately.
    pub fn prepare_output_dir(&self, source: &JobSource, requested: Option<&Path>) -> PathBuf {
        let dir = self.resolve_output_dir(source, requested);
        let _ = std::fs::create_dir_all(&dir);
        dir
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

    /// User-initiated add (picker / drag-and-drop). Destination is an `Output` folder next to the
    /// source, unless Settings uses a fixed folder.
    pub fn enqueue_user_source(&mut self, source: JobSource, preset_id: &str) -> Result<Vec<String>, EngineError> {
        let dest = self.prepare_output_dir(&source, None);
        let req = EnqueueRequest {
            schema: 1,
            source,
            preset_id: preset_id.to_string(),
            preset_override: None,
            output_dir: Some(dest.clone()),
            naming: Some(self.settings.naming.clone()),
            sizes: Vec::new(),
            trim: None,
            scale: None,
            start_queue: false,
        };
        let ids = self.queue.enqueue(&req, &self.presets, &dest, &self.settings.naming, self.settings.allow_overwrite, false)?;
        self.resolve_pending_output_names(&ids);
        Ok(ids)
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
            let source = JobSource::EffectCraft {
                project: project.clone(),
                comp,
                mezzanine: "prores".into(),
                work_area: false,
                width: None,
                height: None,
            };
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
            let source = encodecraft_job::JobSource::file(p.clone());
            let dest = self.prepare_output_dir(&source, None);
            let req = EnqueueRequest {
                schema: 1,
                source,
                preset_id: preset_id.to_string(),
                preset_override: None,
                output_dir: Some(dest.clone()),
                naming: Some(self.settings.naming.clone()),
                sizes: sizes.clone(),
                trim: None,
                scale: None,
                start_queue: false,
            };
            let added = self.queue.enqueue(&req, &self.presets, &dest, &self.settings.naming, self.settings.allow_overwrite, false)?;
            self.resolve_pending_output_names(&added);
            ids.extend(added);
        }
        Ok(ids)
    }

    /// Fill `{width}`/`{height}` from a file probe or the EffectCraft `.ecproj` when Match Source left them as `src`.
    pub fn resolve_pending_output_names(&mut self, ids: &[String]) {
        let allow = self.settings.allow_overwrite;
        let presets = self.presets.clone();
        for id in ids {
            let Some(job) = self.queue.jobs.iter_mut().find(|j| j.id == *id) else {
                continue;
            };
            if !job.size_name_pending {
                continue;
            }
            // File probes (ffprobe) run on the I/O thread. EffectCraft size is a small JSON read.
            let size = job.source.pixel_size().or_else(|| match &job.source {
                JobSource::File { .. } => None,
                JobSource::EffectCraft { project, comp, .. } => ecproj_comp_size_file(project, comp.as_deref()),
            });
            if let Some((w, h)) = size {
                let _ = job.apply_resolved_size(w, h, allow);
            }
            job.refresh_auto_preset(&presets);
        }
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

    pub fn apply_preset_to_jobs(&mut self, ids: &[String], preset_id: &str) -> Result<usize, EngineError> {
        let pr = self.presets.get_or_parse(preset_id).map_err(|e| EngineError::Message(e.to_string()))?;
        let mut n = 0usize;
        let mut touches = Vec::new();
        for id in ids {
            let Some(j) = self.queue.jobs.iter_mut().find(|j| j.id == *id) else {
                continue;
            };
            if j.status.is_active() {
                continue;
            }
            let w = j.preset.video.width;
            let h = j.preset.video.height;
            let scale = j.scale;
            let mut next = pr.clone();
            next.video.width = w;
            next.video.height = h;
            next.video.scale = scale;
            j.preset = next;
            j.preset_overridden = true;
            j.preset_reason = Some(encodecraft_presets::PresetReason::Manual);
            j.encode_warning = encodecraft_ffmpeg::h264_unencodable_message(&j.preset, w.unwrap_or(0), h.unwrap_or(0));
            touches.push((j.source.clone(), j.screen.clone()));
            n = n.saturating_add(1);
        }
        for (src, screen) in touches {
            self.presets.remember_manual_choice(&src, screen.as_ref(), &pr);
        }
        self.settings.remember_recent(&pr.id);
        Ok(n)
    }

    pub fn reset_jobs_to_auto(&mut self, ids: &[String]) -> Result<usize, EngineError> {
        let mut n = 0usize;
        let presets = self.presets.clone();
        let default_id = self.settings.default_preset_id.clone();
        for id in ids {
            let key = self.queue.jobs.iter().find(|j| j.id == *id).map(|j| (j.source.clone(), j.screen.clone()));
            if let Some((src, screen)) = key {
                self.presets.clear_sticky(&src, screen.as_ref());
            }
            let Some(j) = self.queue.jobs.iter_mut().find(|j| j.id == *id) else {
                continue;
            };
            let w = j.preset.video.width;
            let h = j.preset.video.height;
            let scale = j.scale;
            j.preset_overridden = false;
            if let Ok(base) = presets.get("system.h264-mp4") {
                j.preset = base;
                j.preset.video.width = w;
                j.preset.video.height = h;
                j.preset.video.scale = scale;
            }
            j.refresh_auto_preset(&presets);
            if j.preset_reason.is_none() {
                if let Ok(p) = presets.get(&default_id) {
                    j.preset = p;
                    j.preset.video.width = w;
                    j.preset.video.height = h;
                    j.preset.video.scale = scale;
                    j.preset_reason = Some(encodecraft_presets::PresetReason::Default);
                }
            }
            n = n.saturating_add(1);
        }
        Ok(n)
    }

    pub fn add_output_with_preset(&mut self, like_id: &str, preset_id: &str) -> Result<String, EngineError> {
        let pr = self.presets.get_or_parse(preset_id).map_err(|e| EngineError::Message(e.to_string()))?;
        let id = self.queue.add_output_like(like_id, pr.clone(), self.settings.allow_overwrite)?;
        self.settings.remember_recent(&pr.id);
        Ok(id)
    }

    pub fn snapshot_presets(&self, ids: &[String]) -> Vec<(String, encodecraft_presets::ExportPreset, bool, Option<encodecraft_presets::PresetReason>)> {
        ids.iter()
            .filter_map(|id| {
                self.queue.jobs.iter().find(|j| j.id == *id).map(|j| (j.id.clone(), j.preset.clone(), j.preset_overridden, j.preset_reason))
            })
            .collect()
    }

    pub fn restore_preset_snapshot(&mut self, snap: &[(String, encodecraft_presets::ExportPreset, bool, Option<encodecraft_presets::PresetReason>)]) {
        for (id, preset, overridden, reason) in snap {
            if let Some(j) = self.queue.jobs.iter_mut().find(|j| j.id == *id) {
                j.preset = preset.clone();
                j.preset_overridden = *overridden;
                j.preset_reason = *reason;
            }
        }
    }
}

fn drain_inbox_loop(h: Handle) {
    while h.is_alive() {
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
        h.lock(|s| {
            s.settings.output_beside_source = false;
            s.settings.output_dir = home.join("out");
        });
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
        let project = home.join("spot.ecproj");
        std::fs::write(&project, br#"{"compositions":[{"name":"Main"}]}"#).unwrap();
        let ids = h.lock(|s| s.add_effectcraft_comps(project.clone(), &["Main".into(), "Lower".into()], "system.h264-mp4")).unwrap();
        assert_eq!(ids.len(), 2);
        let out = home.join("Output");
        h.lock(|s| {
            assert!(matches!(&s.queue.jobs[0].source, JobSource::EffectCraft { comp: Some(c), .. } if c == "Main"));
            assert!(matches!(&s.queue.jobs[1].source, JobSource::EffectCraft { comp: Some(c), .. } if c == "Lower"));
            for j in &s.queue.jobs {
                assert!(j.output.starts_with(&out), "{}", j.output.display());
            }
        });
        assert!(out.is_dir(), "Output folder is created at queue time");
    }

    #[test]
    fn effectcraft_handoff_does_not_autostart() {
        let _guard = paths::lock_home();
        let home = temp_home();
        paths::set_home(home.clone());
        let h = Handle::open_core().unwrap();
        assert!(!h.lock(|s| s.settings.auto_start_queue));
        let project = home.join("spot.ecproj");
        std::fs::write(&project, br#"{"compositions":[{"name":"A"},{"name":"B"}]}"#).unwrap();
        for name in ["A", "B"] {
            let req = EnqueueRequest {
                schema: 1,
                source: JobSource::EffectCraft {
                    project: project.clone(),
                    comp: Some(name.into()),
                    mezzanine: "prores".into(),
                    work_area: false,
                    width: Some(1920),
                    height: Some(1080),
                },
                preset_id: "system.h264-mp4".into(),
                preset_override: None,
                output_dir: None,
                naming: None,
                sizes: vec![],
                trim: None,
                scale: None,
                start_queue: true,
            };
            h.enqueue(req).unwrap();
        }
        h.lock(|s| {
            assert!(!s.queue.running, "queue must stay idle");
            assert_eq!(s.queue.jobs.len(), 2);
            assert!(s.queue.jobs.iter().all(|j| j.status == JobStatus::Waiting));
            let names: Vec<_> = s.queue.jobs.iter().map(|j| j.output.file_name().unwrap().to_string_lossy().into_owned()).collect();
            assert!(names.iter().any(|n| n == "A.mp4"), "{names:?}");
            assert!(names.iter().any(|n| n == "B.mp4"), "{names:?}");
        });
    }

    #[test]
    fn bulk_preset_keeps_sticky_and_one_off_rules() {
        use encodecraft_presets::{APPROVAL_RES_ID, BETTER_RES_ID, INSTALLATION_ID};
        let _guard = paths::lock_home();
        let home = temp_home();
        paths::set_home(home.clone());
        let h = Handle::open_core().unwrap();
        let project = home.join("spot.ecproj");
        std::fs::write(&project, br#"{"compositions":[{"name":"Main","width":1080,"height":1080}]}"#).unwrap();
        let ids = h.lock(|s| s.add_effectcraft_comps(project.clone(), &["Main".into()], "system.h264-mp4")).unwrap();
        h.lock(|s| s.apply_preset_to_jobs(&ids, BETTER_RES_ID)).unwrap();
        let ids2 = h.lock(|s| s.add_effectcraft_comps(project.clone(), &["Main".into()], "system.h264-mp4")).unwrap();
        h.lock(|s| {
            let j = s.queue.jobs.iter().find(|j| j.id == ids2[0]).unwrap();
            assert_eq!(j.preset.id, BETTER_RES_ID);
        });
        h.lock(|s| s.apply_preset_to_jobs(&ids, APPROVAL_RES_ID)).unwrap();
        let ids3 = h.lock(|s| s.add_effectcraft_comps(project, &["Main".into()], "system.h264-mp4")).unwrap();
        h.lock(|s| {
            let j = s.queue.jobs.iter().find(|j| j.id == ids3[0]).unwrap();
            assert_eq!(j.preset.id, INSTALLATION_ID);
        });
    }

    #[test]
    fn production_commands_go_through_hidden_helper() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let roots = ["crates/ffmpeg/src", "crates/engine/src", "crates/ui-egui/src", "apps/encodecraft-cli/src"];
        let mut bare = Vec::new();
        for root in roots {
            let root = ws.join(root);
            if !root.is_dir() {
                continue;
            }
            fn walk(dir: &std::path::Path, bare: &mut Vec<String>) {
                let Ok(rd) = std::fs::read_dir(dir) else { return };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        walk(&p, bare);
                        continue;
                    }
                    if p.extension().and_then(|s| s.to_str()) != Some("rs") {
                        continue;
                    }
                    let Ok(src) = std::fs::read_to_string(&p) else { continue };
                    let mut in_test = false;
                    for (i, line) in src.lines().enumerate() {
                        let t = line.trim();
                        if t.starts_with("#[cfg(test)]") {
                            in_test = true;
                        }
                        if in_test {
                            continue;
                        }
                        if t.contains("Command::new") && !p.ends_with("proc.rs") {
                            bare.push(format!("{}:{}: {t}", p.display(), i + 1));
                        }
                    }
                }
            }
            walk(&root, &mut bare);
        }
        assert!(bare.is_empty(), "bare Command::new in production:\n{}", bare.join("\n"));
    }

    #[test]
    fn dropped_file_writes_to_output_next_to_source() {
        let _guard = paths::lock_home();
        let home = temp_home();
        paths::set_home(home.clone());
        let h = Handle::open_core().unwrap();
        let media = home.join("clip.mp4");
        std::fs::write(&media, b"x").unwrap();
        let ids = h.lock(|s| s.add_files(&[media.clone()], "system.h264-mp4")).unwrap();
        assert_eq!(ids.len(), 1);
        let out = home.join("Output");
        h.lock(|s| {
            assert!(s.queue.jobs[0].output.starts_with(&out), "{}", s.queue.jobs[0].output.display());
        });
        assert!(out.is_dir());
    }

    #[test]
    fn fixed_output_folder_setting_is_honoured() {
        let _guard = paths::lock_home();
        let home = temp_home();
        paths::set_home(home.clone());
        let h = Handle::open_core().unwrap();
        let dest = home.join("exports");
        std::fs::create_dir_all(&dest).unwrap();
        h.lock(|s| {
            s.settings.output_beside_source = false;
            s.settings.output_dir = dest.clone();
        });
        let project = home.join("spot.ecproj");
        std::fs::write(&project, br#"{"compositions":[{"name":"Main"}]}"#).unwrap();
        let ids = h.lock(|s| s.add_effectcraft_comps(project, &["Main".into()], "system.h264-mp4")).unwrap();
        assert_eq!(ids.len(), 1);
        h.lock(|s| {
            assert!(s.queue.jobs[0].output.starts_with(&dest), "{}", s.queue.jobs[0].output.display());
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

    #[test]
    fn effectcraft_match_source_names_from_comp_size() {
        let _guard = paths::lock_home();
        let home = temp_home();
        paths::set_home(home.clone());
        let h = Handle::open_core().unwrap();
        h.lock(|s| {
            s.settings.naming = "{name}_{width}x{height}".into();
        });
        let project = home.join("test-2.ecproj");
        std::fs::write(&project, br#"{"compositions":[{"name":"Honor 400_EN.ai Comp","width":1080,"height":1080}]}"#).unwrap();
        let ids = h
            .lock(|s| s.add_effectcraft_comps(project, &["Honor 400_EN.ai Comp".into()], "system.h264-mp4"))
            .unwrap();
        assert_eq!(ids.len(), 1);
        h.lock(|s| {
            let name = s.queue.jobs[0].output.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.contains("1080x1080"), "{name}");
            assert!(!name.contains("srcxsrc"), "{name}");
            assert!(!s.queue.jobs[0].size_name_pending);
        });
    }
}
