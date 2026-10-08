use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use encodecraft_job::{EnqueueRequest, JobSource, ScreenSize};
use encodecraft_presets::is_under;
use serde::{Deserialize, Serialize};

use crate::drops::is_media_path;
use crate::fsutil;
use crate::Handle;

/// Skip absurd files (likely a copy error or a zip bomb dropped on the folder).
const MAX_WATCH_BYTES: u64 = 64 * 1024 * 1024 * 1024;

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

#[derive(Clone, Copy)]
struct Fingerprint {
    size: u64,
    mtime: u64,
    ticks: u8,
}

pub fn run(h: Handle) {
    let mut seen: HashMap<String, Fingerprint> = HashMap::new();
    h.lock(|s| {
        for w in &s.settings.watch_folders {
            for p in list_media(&w.path, w.recursive) {
                if let Some(fp) = fingerprint(&p) {
                    seen.insert(key(&p), Fingerprint { ticks: 8, ..fp });
                }
            }
        }
    });
    while h.is_alive() {
        thread::sleep(Duration::from_secs(2));
        let (folders, output_dir, work_dir) = h.lock(|s| (s.settings.watch_folders.clone(), s.settings.output_dir.clone(), crate::paths::work_dir()));
        for w in folders {
            if !w.enabled {
                continue;
            }
            let skip_out = w.output_dir.clone().unwrap_or_else(|| output_dir.clone());
            let skip_sidecar = w.path.join("Output");
            for p in list_media(&w.path, w.recursive) {
                if fsutil::is_symlink(&p) {
                    continue;
                }
                if is_under(&p, &skip_out) || is_under(&p, &output_dir) || is_under(&p, &skip_sidecar) || is_under(&p, &work_dir) {
                    continue;
                }
                if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains("encodecraft-partial")) {
                    continue;
                }
                let Some(fp) = fingerprint(&p) else {
                    continue;
                };
                if fp.size == 0 || fp.size > MAX_WATCH_BYTES {
                    continue;
                }
                let k = key(&p);
                match seen.get_mut(&k) {
                    Some(prev) if prev.size == fp.size && prev.mtime == fp.mtime => {
                        if prev.ticks < 3 {
                            prev.ticks = prev.ticks.saturating_add(1);
                        }
                        if prev.ticks != 2 {
                            continue;
                        }
                        prev.ticks = 3;
                    }
                    Some(prev) => {
                        *prev = Fingerprint { ticks: 1, ..fp };
                        continue;
                    }
                    None => {
                        seen.insert(k, Fingerprint { ticks: 1, ..fp });
                        continue;
                    }
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
                let _ = h.enqueue_trusted(req);
            }
        }
    }
}

fn fingerprint(p: &Path) -> Option<Fingerprint> {
    let meta = std::fs::metadata(p).ok()?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64)?;
    Some(Fingerprint { size: meta.len(), mtime, ticks: 0 })
}

fn key(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn list_media(root: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, recursive, 0, &mut out);
    out
}

fn walk(dir: &Path, recursive: bool, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let ft = e.file_type().ok();
        if ft.as_ref().is_some_and(|t| t.is_symlink()) {
            continue;
        }
        if ft.as_ref().is_some_and(|t| t.is_dir()) {
            if recursive {
                walk(&p, true, depth.saturating_add(1), out);
            }
            continue;
        }
        if is_media_path(&p) {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_partial_and_output_dir() {
        let out = PathBuf::from("/exports");
        let inside = PathBuf::from("/exports/clip.mp4");
        assert!(is_under(&inside, &out));
        assert!(!is_under(Path::new("/incoming/clip.mp4"), &out));
    }
}
