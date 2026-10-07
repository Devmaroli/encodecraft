use std::collections::HashSet;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use encodecraft_job::{EnqueueRequest, JobSource, ScreenSize};
use serde::{Deserialize, Serialize};

use crate::Handle;

const VIDEO_EXT: &[&str] =
    &["mp4", "mov", "mkv", "webm", "avi", "mxf", "mts", "m4v", "mpg", "mpeg", "wmv", "gif", "png", "jpg", "jpeg", "wav", "mp3", "aac", "m4a", "aif", "aiff"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchFolder {
    pub path: PathBuf,
    pub preset_id: String,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sizes: Vec<ScreenSize>,
    #[serde(default)]
    pub enabled: bool,
}

impl WatchFolder {
    pub fn new(path: PathBuf, preset_id: String) -> Self {
        Self { path, preset_id, recursive: false, output_dir: None, sizes: Vec::new(), enabled: true }
    }
}

pub fn run(h: Handle) {
    let mut seen: HashSet<String> = HashSet::new();
    // Prime: don't encode files that were already in the folder.
    h.lock(|s| {
        for w in &s.settings.watch_folders {
            for p in list_media(&w.path, w.recursive) {
                seen.insert(key(&p));
            }
        }
    });
    loop {
        thread::sleep(Duration::from_secs(2));
        let folders = h.lock(|s| s.settings.watch_folders.clone());
        for w in folders {
            if !w.enabled {
                continue;
            }
            for p in list_media(&w.path, w.recursive) {
                let k = key(&p);
                if !seen.insert(k) {
                    continue;
                }
                let req = EnqueueRequest {
                    schema: 1,
                    source: JobSource::file(p),
                    preset_id: w.preset_id.clone(),
                    preset_override: None,
                    output_dir: w.output_dir.clone(),
                    naming: None,
                    sizes: w.sizes.clone(),
                    trim: None,
                    scale: None,
                    start_queue: true,
                };
                let _ = h.enqueue(req);
            }
        }
    }
}

fn key(p: &std::path::Path) -> String {
    let meta = std::fs::metadata(p).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let sz = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    format!("{}:{mtime}:{sz}", p.display())
}

fn list_media(root: &std::path::Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, recursive, 0, &mut out);
    out
}

fn walk(dir: &std::path::Path, recursive: bool, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if recursive {
                walk(&p, true, depth.saturating_add(1), out);
            }
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if VIDEO_EXT.iter().any(|x| *x == ext) {
            out.push(p);
        }
    }
}
