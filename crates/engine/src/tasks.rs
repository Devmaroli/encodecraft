//! Background I/O: persist, ffprobe, thumbnails, .ecproj listing.
//!
//! The egui thread never waits on these. Results arrive on [`UiEvent`].

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use encodecraft_ffmpeg::{probe, thumbnail_png};
use encodecraft_job::JobSource;

use crate::drops::{classify_dropped_paths, list_ecproj_compositions_file, DropOutcome};
use crate::paths;
use crate::Handle;

pub enum IoCmd {
    Persist,
    Probe(Vec<String>),
    Thumb(String),
    ListEcproj(PathBuf),
    Classify(Vec<PathBuf>),
}

pub enum UiEvent {
    Changed,
    Thumb { id: String, rgba: Vec<u8>, w: u32, h: u32 },
    PreviewFrame { id: String, name: String, rgba: Vec<u8>, w: u32, h: u32, output_w: u32, output_h: u32 },
    Ecproj { path: PathBuf, comps: Vec<String>, note: Option<String> },
    Dropped(DropOutcome),
    Slow { name: String, ms: f64 },
}

#[derive(Clone)]
pub struct IoHub {
    pub tx: Sender<IoCmd>,
    pub events: Sender<UiEvent>,
    rx: Arc<Mutex<Receiver<UiEvent>>>,
}

impl IoHub {
    pub fn poll(&self) -> Vec<UiEvent> {
        let Ok(rx) = self.rx.lock() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

pub fn start(handle: Handle) -> IoHub {
    let (cmd_tx, cmd_rx) = mpsc::channel::<IoCmd>();
    let (ui_tx, ui_rx) = mpsc::channel::<UiEvent>();
    let worker = handle.clone();
    let events = ui_tx.clone();
    thread::Builder::new().name("encodecraft-io".into()).spawn(move || io_loop(worker, cmd_rx, ui_tx)).ok();
    IoHub { tx: cmd_tx, events, rx: Arc::new(Mutex::new(ui_rx)) }
}

fn io_loop(h: Handle, rx: Receiver<IoCmd>, ui: Sender<UiEvent>) {
    while h.is_alive() {
        let cmd = match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(c) => c,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if h.take_dirty() {
                    persist_timed(&h, &ui);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match cmd {
            IoCmd::Persist => {
                let _ = h.take_dirty();
                persist_timed(&h, &ui);
            }
            IoCmd::Probe(ids) => probe_ids(&h, &ids, &ui),
            IoCmd::Thumb(id) => thumb_one(&h, &id, &ui),
            IoCmd::ListEcproj(path) => list_ecproj(&path, &ui),
            IoCmd::Classify(paths) => {
                let t0 = Instant::now();
                let outcome = classify_dropped_paths(&paths);
                emit_slow(&ui, "classify_drops", t0);
                let _ = ui.send(UiEvent::Dropped(outcome));
            }
        }
    }
}

fn persist_timed(h: &Handle, ui: &Sender<UiEvent>) {
    let t0 = Instant::now();
    h.persist();
    emit_slow(ui, "persist", t0);
}

fn emit_slow(ui: &Sender<UiEvent>, name: &str, t0: Instant) {
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    if ms >= 8.0 {
        log::debug!("slow {name}: {ms:.1} ms");
        let _ = ui.send(UiEvent::Slow { name: name.into(), ms });
    }
}

fn probe_ids(h: &Handle, ids: &[String], ui: &Sender<UiEvent>) {
    let ff = h.lock(|s| s.ffmpeg.as_ref().ok().cloned());
    let Some(ff) = ff else {
        h.lock(|s| {
            for id in ids {
                if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == *id) {
                    j.metadata_pending = false;
                }
            }
        });
        h.note_change();
        let _ = ui.send(UiEvent::Changed);
        return;
    };
    for id in ids {
        if !h.is_alive() {
            break;
        }
        let job = h.lock(|s| s.queue.jobs.iter().find(|j| j.id == *id).cloned());
        let Some(job) = job else {
            continue;
        };
        let t0 = Instant::now();
        match &job.source {
            JobSource::File { path, .. } => {
                if let Ok(info) = probe(&ff, path) {
                    let summary = info.summary();
                    h.lock(|s| {
                        let allow = s.settings.allow_overwrite;
                        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == *id) {
                            let _ = j.apply_resolved_size(info.width(), info.height(), allow);
                            j.duration_sec = info.duration_sec;
                            j.metadata_pending = false;
                        }
                        s.push_log(format!("{id}: {summary}"));
                    });
                } else {
                    h.lock(|s| {
                        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == *id) {
                            j.metadata_pending = false;
                        }
                    });
                }
            }
            JobSource::EffectCraft { project, comp, .. } => {
                if let Some((w, hgt)) = crate::ecproj_comp_size_file(project, comp.as_deref()) {
                    h.lock(|s| {
                        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == *id) {
                            let allow = s.settings.allow_overwrite;
                            let _ = j.apply_resolved_size(w, hgt, allow);
                            j.metadata_pending = false;
                        }
                    });
                } else {
                    h.lock(|s| {
                        if let Some(j) = s.queue.jobs.iter_mut().find(|j| j.id == *id) {
                            j.metadata_pending = false;
                        }
                    });
                }
            }
        }
        emit_slow(ui, "probe", t0);
        h.note_change();
        let _ = ui.send(UiEvent::Changed);
    }
}

fn thumb_one(h: &Handle, id: &str, ui: &Sender<UiEvent>) {
    let job = h.lock(|s| s.queue.jobs.iter().find(|j| j.id == id).cloned());
    let Some(job) = job else {
        return;
    };
    let src = match &job.source {
        JobSource::File { path, .. } => path.clone(),
        JobSource::EffectCraft { .. } => return,
    };
    let Some(ff) = h.lock(|s| s.ffmpeg.as_ref().ok().cloned()) else {
        return;
    };
    let dest = paths::thumbs_dir().join(format!("{}.jpg", job.id));
    let t0 = Instant::now();
    if !dest.exists() && thumbnail_png(&ff, &src, &dest, job.trim.in_sec.max(0.1)).is_err() {
        return;
    }
    emit_slow(ui, "thumbnail", t0);
    let Ok(bytes) = std::fs::read(&dest) else {
        return;
    };
    let Ok(img) = image::load_from_memory(&bytes) else {
        return;
    };
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let hgt = rgba.height();
    let _ = ui.send(UiEvent::Thumb { id: job.id, rgba: rgba.into_raw(), w, h: hgt });
}

fn list_ecproj(path: &std::path::Path, ui: &Sender<UiEvent>) {
    let t0 = Instant::now();
    let (comps, note) = match list_ecproj_compositions_file(path) {
        Ok(c) if c.is_empty() => {
            (Vec::new(), Some("Could not list compositions in this project. Queue with EffectCraft’s default/first comp, or type a name.".into()))
        }
        Ok(c) => (c, None),
        Err(e) => (Vec::new(), Some(format!("Could not read project ({e}). Queue with the default composition, or type a name."))),
    };
    emit_slow(ui, "list_ecproj", t0);
    let _ = ui.send(UiEvent::Ecproj { path: path.to_path_buf(), comps, note });
}
