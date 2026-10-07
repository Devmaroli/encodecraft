//! Classify Explorer / EffectCraft drag-and-drop payloads into queue jobs.
//!
//! Desktop drops are a user action (same trust as File ▸ Add source). HTTP/inbox still
//! goes through [`crate::Handle::enqueue`] with output-folder confinement.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::fsutil;

const MAX_DROP_FILES: usize = 256;
const MAX_ECPROJ_BYTES: u64 = 32 * 1024 * 1024;

pub const VIDEO_EXT: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "mxf", "mts", "m2ts", "m4v", "mpg", "mpeg", "wmv", "flv", "ts", "gif",
];
pub const AUDIO_EXT: &[&str] = &["wav", "mp3", "aac", "m4a", "aif", "aiff", "flac", "ogg", "opus", "wma"];
pub const STILL_EXT: &[&str] = &["png", "jpg", "jpeg", "exr", "tif", "tiff", "tga", "dpx", "bmp", "webp"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceDrop {
    pub first_frame: PathBuf,
    pub start: u32,
    pub frames: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropOutcome {
    pub media: Vec<PathBuf>,
    pub sequences: Vec<SequenceDrop>,
    pub projects: Vec<PathBuf>,
    pub unsupported: Vec<(PathBuf, String)>,
}

impl DropOutcome {
    pub fn is_empty(&self) -> bool {
        self.media.is_empty() && self.sequences.is_empty() && self.projects.is_empty()
    }

    pub fn unsupported_message(&self) -> Option<String> {
        if self.unsupported.is_empty() {
            return None;
        }
        let n = self.unsupported.len();
        let (path, reason) = &self.unsupported[0];
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned());
        if n == 1 {
            Some(format!("Can't queue “{name}”: {reason}"))
        } else {
            Some(format!("Can't queue {n} dropped items. “{name}”: {reason}"))
        }
    }
}

pub fn is_media_path(path: &Path) -> bool {
    let ext = ext_lower(path);
    VIDEO_EXT.contains(&ext.as_str()) || AUDIO_EXT.contains(&ext.as_str()) || STILL_EXT.contains(&ext.as_str())
}

pub fn is_ecproj(path: &Path) -> bool {
    ext_lower(path) == "ecproj"
}

pub fn classify_dropped_paths(paths: &[PathBuf]) -> DropOutcome {
    let mut out = DropOutcome::default();
    if paths.is_empty() {
        out.unsupported.push((PathBuf::from("(no path)"), "EncodeCraft needs a file path. Save the EffectCraft project and drop the .ecproj from Explorer, or drop media files.".into()));
        return out;
    }
    for p in paths {
        classify_one(p, &mut out);
    }
    out
}

fn classify_one(path: &Path, out: &mut DropOutcome) {
    if path.as_os_str().is_empty() {
        out.unsupported.push((path.to_path_buf(), "empty path".into()));
        return;
    }
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => {
            // Hovered / in-flight copies: classify by extension so the overlay copy is honest.
            classify_by_ext(path, out);
            return;
        }
    };
    if meta.file_type().is_symlink() {
        if path.is_file() && (is_media_path(path) || is_ecproj(path)) {
            classify_by_ext(path, out);
        } else {
            out.unsupported.push((path.to_path_buf(), "skipped symlink (drop the real file or folder)".into()));
        }
        return;
    }
    if meta.is_dir() {
        classify_dir(path, out);
        return;
    }
    classify_by_ext(path, out);
}

fn classify_by_ext(path: &Path, out: &mut DropOutcome) {
    if is_ecproj(path) {
        out.projects.push(path.to_path_buf());
        return;
    }
    if is_media_path(path) {
        out.media.push(path.to_path_buf());
        return;
    }
    let ext = ext_lower(path);
    let reason = if ext.is_empty() {
        "not a video, audio, image sequence, or EffectCraft project (.ecproj)".into()
    } else {
        format!(".{ext} is not a supported media type or EffectCraft project")
    };
    out.unsupported.push((path.to_path_buf(), reason));
}

fn classify_dir(dir: &Path, out: &mut DropOutcome) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        out.unsupported.push((dir.to_path_buf(), "could not read folder".into()));
        return;
    };
    let mut files: Vec<PathBuf> = Vec::new();
    for e in rd.flatten() {
        if files.len() >= MAX_DROP_FILES {
            out.unsupported.push((dir.to_path_buf(), format!("folder has more than {MAX_DROP_FILES} files; add a smaller set")));
            return;
        }
        let p = e.path();
        if fsutil::is_symlink(&p) {
            continue;
        }
        let Ok(ft) = e.file_type() else {
            continue;
        };
        if ft.is_dir() {
            continue;
        }
        files.push(p);
    }
    if files.is_empty() {
        out.unsupported.push((dir.to_path_buf(), "no video, audio, image sequence, or .ecproj in this folder".into()));
        return;
    }
    let (singles, sequences) = group_sequences(files);
    let mut added = false;
    for p in singles {
        if is_ecproj(&p) {
            out.projects.push(p);
            added = true;
        } else if is_media_path(&p) {
            out.media.push(p);
            added = true;
        }
    }
    if !sequences.is_empty() {
        added = true;
        out.sequences.extend(sequences);
    }
    if !added {
        out.unsupported.push((dir.to_path_buf(), "no video, audio, image sequence, or .ecproj in this folder".into()));
    }
}

/// Group numbered stills that were dropped together (or found in one folder).
pub fn group_sequences(files: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<SequenceDrop>) {
    let mut leftover: Vec<PathBuf> = Vec::new();
    let mut buckets: BTreeMap<String, Vec<(u32, PathBuf, usize)>> = BTreeMap::new();
    for p in files {
        if !is_still(&p) {
            leftover.push(p);
            continue;
        }
        match parse_numbered_stem(&p) {
            Some((key, num, width)) => buckets.entry(key).or_default().push((num, p, width)),
            None => leftover.push(p),
        }
    }
    let mut sequences = Vec::new();
    for (_key, mut frames) in buckets {
        frames.sort_by_key(|(n, _, _)| *n);
        if frames.len() < 2 {
            leftover.extend(frames.into_iter().map(|(_, p, _)| p));
            continue;
        }
        let start = frames[0].0;
        let first = frames[0].1.clone();
        sequences.push(SequenceDrop { first_frame: first, start, frames: frames.len() as u32 });
    }
    leftover.sort();
    sequences.sort_by(|a, b| a.first_frame.cmp(&b.first_frame));
    (leftover, sequences)
}

fn is_still(path: &Path) -> bool {
    STILL_EXT.contains(&ext_lower(path).as_str())
}

fn parse_numbered_stem(path: &Path) -> Option<(String, u32, usize)> {
    let stem = path.file_stem()?.to_str()?;
    let ext = ext_lower(path);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let bytes = stem.as_bytes();
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1].is_ascii_digit() {
        i -= 1;
    }
    if i == 0 || i == bytes.len() {
        return None;
    }
    let width = bytes.len() - i;
    if width > 8 {
        return None;
    }
    let num: u32 = stem[i..].parse().ok()?;
    let prefix = &stem[..i];
    let key = format!("{}|{}|{}", parent.to_string_lossy(), prefix, ext);
    Some((key, num, width))
}

/// FFmpeg `-i` pattern and digit width for a first-frame still (`hero_0001.png` → `hero_%04d.png`).
#[cfg(test)]
pub fn ffmpeg_sequence_pattern(first_frame: &Path) -> Option<(PathBuf, u32, usize)> {
    let ( _key, num, width) = parse_numbered_stem(first_frame)?;
    let stem = first_frame.file_stem()?.to_str()?;
    let prefix = &stem[..stem.len() - width];
    let ext = first_frame.extension()?.to_string_lossy();
    let name = format!("{prefix}%0{width}d.{ext}");
    let path = match first_frame.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(name),
        _ => PathBuf::from(name),
    };
    Some((path, num, width))
}

fn ext_lower(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Composition names from an EffectCraft `.ecproj` (versioned JSON). Unknown shapes yield `[]`.
pub fn list_ecproj_compositions(bytes: &[u8]) -> Vec<String> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    walk_comps(&v, &mut names, None);
    names.sort();
    names.dedup();
    names
}

pub fn list_ecproj_compositions_file(path: &Path) -> Result<Vec<String>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_ECPROJ_BYTES {
        return Err(format!("project is larger than {} MB", MAX_ECPROJ_BYTES / (1024 * 1024)));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(list_ecproj_compositions(&bytes))
}

/// Comp width×height from an `.ecproj` (best-effort). Used to fill `{width}`/`{height}` before encode.
pub fn ecproj_comp_size(bytes: &[u8], comp: Option<&str>) -> Option<(u32, u32)> {
    let v = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
    let mut found = Vec::new();
    walk_comp_sizes(&v, None, &mut found);
    if found.is_empty() {
        return None;
    }
    if let Some(want) = comp.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((_, w, h)) = found.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(want)) {
            return Some((*w, *h));
        }
        return None;
    }
    Some((found[0].1, found[0].2))
}

pub fn ecproj_comp_size_file(path: &Path, comp: Option<&str>) -> Option<(u32, u32)> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_ECPROJ_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    ecproj_comp_size(&bytes, comp)
}

fn walk_comp_sizes(v: &serde_json::Value, parent_key: Option<&str>, out: &mut Vec<(String, u32, u32)>) {
    match v {
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(map) = item.as_object() {
                    if let Some(dims) = dims_of_map(map) {
                        if let Some(n) = comp_name(item).or_else(|| map.get("name").or_else(|| map.get("id")).and_then(|x| x.as_str()).map(|s| s.to_string())) {
                            out.push((n, dims.0, dims.1));
                        }
                    }
                }
                walk_comp_sizes(item, None, out);
            }
        }
        serde_json::Value::Object(map) => {
            if parent_key.is_some_and(key_is_comps) {
                for (k, child) in map {
                    if let Some(cm) = child.as_object() {
                        if let Some(dims) = dims_of_map(cm) {
                            let n = comp_name(child).unwrap_or_else(|| k.clone());
                            out.push((n, dims.0, dims.1));
                        }
                    }
                    walk_comp_sizes(child, None, out);
                }
                return;
            }
            if let Some(dims) = dims_of_map(map) {
                if let Some(n) = typed_comp_name(map).or_else(|| map.get("name").and_then(|x| x.as_str()).map(|s| s.to_string())) {
                    if map.contains_key("layers") || typed_comp_name(map).is_some() || parent_key.is_some_and(key_is_comps) {
                        out.push((n, dims.0, dims.1));
                    }
                }
            }
            for (k, child) in map {
                walk_comp_sizes(child, Some(k.as_str()), out);
            }
        }
        _ => {}
    }
}

fn dims_of_map(map: &serde_json::Map<String, serde_json::Value>) -> Option<(u32, u32)> {
    let num = |key: &str| -> Option<u32> {
        let v = map.get(key)?;
        let n = v
            .as_u64()
            .or_else(|| v.as_i64().and_then(|i| u64::try_from(i).ok()))
            .or_else(|| v.as_f64().map(|f| f.round() as u64))?;
        if (1..100_000).contains(&n) {
            Some(n as u32)
        } else {
            None
        }
    };
    if let (Some(w), Some(h)) = (num("width").or_else(|| num("w")), num("height").or_else(|| num("h"))) {
        return Some((w, h));
    }
    let sz = map.get("size").or_else(|| map.get("compSize"))?;
    if let Some(arr) = sz.as_array() {
        if arr.len() >= 2 {
            let w = arr[0].as_u64().or_else(|| arr[0].as_f64().map(|f| f.round() as u64))?;
            let h = arr[1].as_u64().or_else(|| arr[1].as_f64().map(|f| f.round() as u64))?;
            if w > 0 && h > 0 && w < 100_000 && h < 100_000 {
                return Some((w as u32, h as u32));
            }
        }
        return None;
    }
    sz.as_object().and_then(dims_of_map)
}

fn walk_comps(v: &serde_json::Value, out: &mut Vec<String>, parent_key: Option<&str>) {
    match v {
        serde_json::Value::Array(arr) => {
            if parent_key.is_some_and(key_is_comps) {
                for item in arr {
                    if let Some(n) = comp_name(item).or_else(|| item.as_object().and_then(|o| o.get("name").or_else(|| o.get("id"))).and_then(|x| x.as_str()).map(|s| s.to_string())) {
                        push_name(out, n);
                    }
                }
            } else {
                for item in arr {
                    walk_comps(item, out, None);
                }
            }
        }
        serde_json::Value::Object(map) => {
            if parent_key.is_some_and(key_is_comps) {
                for (k, child) in map {
                    if child.is_object() || child.is_null() {
                        push_name(out, k.clone());
                    }
                    if let Some(n) = comp_name(child) {
                        push_name(out, n);
                    }
                }
                return;
            }
            if let Some(n) = typed_comp_name(map) {
                push_name(out, n);
            }
            for (k, child) in map {
                walk_comps(child, out, Some(k.as_str()));
            }
        }
        _ => {}
    }
}

fn key_is_comps(k: &str) -> bool {
    matches!(k.to_ascii_lowercase().as_str(), "compositions" | "comps" | "comp_list" | "compList")
}

fn typed_comp_name(map: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let kind = map
        .get("type")
        .or_else(|| map.get("kind"))
        .or_else(|| map.get("itemType"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let kl = kind.to_ascii_lowercase();
    let is_comp = kl == "composition" || kl == "comp" || map.get("isComp").and_then(|v| v.as_bool()) == Some(true);
    if !is_comp {
        return None;
    }
    map.get("name").or_else(|| map.get("id")).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn comp_name(v: &serde_json::Value) -> Option<String> {
    let obj = v.as_object()?;
    if let Some(n) = typed_comp_name(obj) {
        return Some(n);
    }
    if obj.contains_key("layers") || obj.contains_key("width") && obj.contains_key("height") && obj.contains_key("frameRate") {
        return obj.get("name").or_else(|| obj.get("id")).and_then(|x| x.as_str()).map(|s| s.to_string());
    }
    obj.get("name").and_then(|x| x.as_str()).map(|s| s.to_string()).filter(|_n| obj.contains_key("id") && obj.len() <= 8)
}

fn push_name(out: &mut Vec<String>, name: String) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    out.push(name.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "ec-drop-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    #[test]
    fn classifies_media_and_rejects_junk() {
        let dir = tmp();
        let mp4 = dir.join("ad.mp4");
        let wav = dir.join("vox.wav");
        let txt = dir.join("notes.txt");
        let exe = dir.join("setup.exe");
        std::fs::write(&mp4, b"x").unwrap();
        std::fs::write(&wav, b"x").unwrap();
        std::fs::write(&txt, b"x").unwrap();
        std::fs::write(&exe, b"x").unwrap();
        let out = classify_dropped_paths(&[mp4.clone(), wav.clone(), txt.clone(), exe.clone()]);
        assert_eq!(out.media, vec![mp4, wav]);
        assert_eq!(out.projects.len(), 0);
        assert_eq!(out.unsupported.len(), 2);
        let msg = out.unsupported_message().unwrap();
        assert!(msg.contains("notes.txt") || msg.contains(".txt") || msg.contains("2 dropped"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn classifies_ecproj() {
        let dir = tmp();
        let p = dir.join("spot.ecproj");
        std::fs::write(&p, br#"{"compositions":[{"name":"Main"}]}"#).unwrap();
        let out = classify_dropped_paths(&[p.clone()]);
        assert_eq!(out.projects, vec![p]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_of_numbered_stills_is_one_sequence() {
        let dir = tmp();
        for i in 1..=4 {
            std::fs::write(dir.join(format!("hero_{i:04}.png")), b"x").unwrap();
        }
        std::fs::write(dir.join("readme.txt"), b"no").unwrap();
        let out = classify_dropped_paths(&[dir.clone()]);
        assert_eq!(out.sequences.len(), 1);
        assert_eq!(out.sequences[0].start, 1);
        assert_eq!(out.sequences[0].frames, 4);
        assert!(out.media.is_empty());
        let (pattern, start, width) = ffmpeg_sequence_pattern(&out.sequences[0].first_frame).unwrap();
        assert_eq!(start, 1);
        assert_eq!(width, 4);
        assert!(pattern.to_string_lossy().ends_with("hero_%04d.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mixed_drop_keeps_video_and_groups_stills() {
        let dir = tmp();
        std::fs::write(dir.join("clip.mov"), b"x").unwrap();
        std::fs::write(dir.join("a_01.jpg"), b"x").unwrap();
        std::fs::write(dir.join("a_02.jpg"), b"x").unwrap();
        let files = vec![dir.join("clip.mov"), dir.join("a_01.jpg"), dir.join("a_02.jpg")];
        let (singles, seqs) = group_sequences(files);
        assert_eq!(singles, vec![dir.join("clip.mov")]);
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0].start, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_drop_is_unsupported() {
        let out = classify_dropped_paths(&[]);
        assert!(out.unsupported_message().unwrap().contains("file path"));
    }

    #[test]
    fn parses_ecproj_shapes() {
        let a = br#"{"compositions":[{"name":"Main"},{"name":"Lower third"}]}"#;
        assert_eq!(list_ecproj_compositions(a), vec!["Lower third".to_string(), "Main".to_string()]);
        let b = br#"{"comps":{"Hero":{},"Bug":{}}}"#;
        let names = list_ecproj_compositions(b);
        assert!(names.contains(&"Hero".to_string()));
        assert!(names.contains(&"Bug".to_string()));
        let c = br#"{"project":{"items":[{"kind":"composition","name":"Spot"},{"kind":"footage","name":"clip.mp4"}]}}"#;
        assert_eq!(list_ecproj_compositions(c), vec!["Spot".to_string()]);
        let d = br#"{"items":[{"type":"comp","name":"Open","layers":[]}]}"#;
        assert_eq!(list_ecproj_compositions(d), vec!["Open".to_string()]);
        assert!(list_ecproj_compositions(b"not json").is_empty());
    }

    #[test]
    fn reads_comp_size_from_ecproj() {
        let a = br#"{"compositions":[{"name":"Honor 400_EN.ai Comp","width":1080,"height":1080},{"name":"Other","width":1920,"height":1080}]}"#;
        assert_eq!(ecproj_comp_size(a, Some("Honor 400_EN.ai Comp")), Some((1080, 1080)));
        assert_eq!(ecproj_comp_size(a, Some("Other")), Some((1920, 1080)));
        let b = br#"{"items":[{"type":"comp","name":"Square","layers":[],"width":1080,"height":1080}]}"#;
        assert_eq!(ecproj_comp_size(b, Some("Square")), Some((1080, 1080)));
        let c = br#"{"comps":{"Main":{"width":960,"height":540,"layers":[]}}}"#;
        assert_eq!(ecproj_comp_size(c, Some("Main")), Some((960, 540)));
    }

    #[test]
    fn folder_with_only_junk_is_unsupported() {
        let dir = tmp();
        std::fs::write(dir.join("todo.docx"), b"x").unwrap();
        let out = classify_dropped_paths(&[dir.clone()]);
        assert!(out.is_empty());
        assert!(!out.unsupported.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
