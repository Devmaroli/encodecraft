//! Output file naming templates.
//!
//! Tokens: `{comp}`, `{name}`, `{project}`, `{width}`, `{height}`, `{preset}`, `{codec}`, `{ext}`,
//! `{date}`, `{time}`, `{job}`, `{index}`, `{screen}`. Unknown tokens are left in place.
//! Path separators in values are replaced so a template cannot escape the output directory.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use encodecraft_job::JobSource;

use super::ExportPreset;

pub fn default_template() -> &'static str {
    "{comp}"
}

#[derive(Clone, Debug, Default)]
pub struct NamingContext {
    pub name: String,
    pub comp: String,
    pub project: String,
    pub width: u32,
    pub height: u32,
    pub preset: String,
    pub codec: String,
    pub ext: String,
    pub job: String,
    pub index: u32,
    pub screen: String,
    pub date: String,
    pub time: String,
}

impl NamingContext {
    pub fn from_preset(source_name: &str, preset: &ExportPreset, width: u32, height: u32, job: &str, index: u32, screen: &str) -> Self {
        let (date, time) = local_stamp();
        let stem = sanitize_filename(source_name);
        Self {
            name: stem.clone(),
            comp: stem,
            project: String::new(),
            width,
            height,
            preset: sanitize_filename(&preset.name),
            codec: preset.format.extension().into(),
            ext: preset.extension().into(),
            job: sanitize_filename(job),
            index,
            screen: sanitize_filename(screen),
            date,
            time,
        }
    }

    pub fn from_source(source: &JobSource, preset: &ExportPreset, width: u32, height: u32, job: &str, index: u32, screen: &str) -> Self {
        let (date, time) = local_stamp();
        let comp = sanitize_filename(&source.comp_name());
        Self {
            name: comp.clone(),
            comp,
            project: sanitize_filename(&source.project_name()),
            width,
            height,
            preset: sanitize_filename(&preset.name),
            codec: preset.format.extension().into(),
            ext: preset.extension().into(),
            job: sanitize_filename(job),
            index,
            screen: sanitize_filename(screen),
            date,
            time,
        }
    }

    pub fn tokens(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("name".into(), self.name.clone());
        m.insert("comp".into(), self.comp.clone());
        m.insert("project".into(), self.project.clone());
        m.insert("width".into(), if self.width == 0 { "src".into() } else { self.width.to_string() });
        m.insert("height".into(), if self.height == 0 { "src".into() } else { self.height.to_string() });
        m.insert("preset".into(), self.preset.clone());
        m.insert("codec".into(), self.codec.clone());
        m.insert("ext".into(), self.ext.clone());
        m.insert("job".into(), self.job.clone());
        m.insert("index".into(), self.index.to_string());
        m.insert("screen".into(), self.screen.clone());
        m.insert("date".into(), self.date.clone());
        m.insert("time".into(), self.time.clone());
        m
    }
}

pub fn naming_uses_size(template: &str) -> bool {
    template.contains("{width}") || template.contains("{height}")
}

pub fn apply_template(template: &str, ctx: &NamingContext) -> String {
    let tokens = ctx.tokens();
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        match rest.find('}') {
            Some(end) => {
                let key = rest[..end].trim();
                if let Some(v) = tokens.get(key) {
                    out.push_str(v);
                } else {
                    out.push('{');
                    out.push_str(&rest[..end]);
                    out.push('}');
                }
                rest = &rest[end + 1..];
            }
            None => {
                out.push('{');
                out.push_str(rest);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let stem = sanitize_filename(&out);
    if stem.is_empty() {
        ctx.name.clone()
    } else {
        stem
    }
}

pub fn output_path(dir: &Path, template: &str, ctx: &NamingContext, sequence: bool) -> PathBuf {
    let stem = apply_template(template, ctx);
    if sequence {
        dir.join(stem).join(format!("frame_%06d.{}", ctx.ext))
    } else {
        dir.join(format!("{stem}.{}", ctx.ext))
    }
}

pub fn sanitize_filename(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => out.push('_'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let t = out.trim().trim_matches('.').trim();
    let mut t = if t.is_empty() { "untitled".to_string() } else { t.to_string() };
    if t.starts_with('-') {
        t.insert(0, '_');
    }
    if is_windows_reserved(&t) {
        t = format!("_{t}");
    }
    if t.chars().count() > 240 {
        t = t.chars().take(240).collect();
        t = t.trim_matches('.').trim().to_string();
        if t.is_empty() {
            t = "untitled".into();
        }
    }
    t
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    let u = stem.to_ascii_uppercase();
    matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || (u.len() == 4 && u.as_bytes()[3].is_ascii_digit() && (u.starts_with("COM") || u.starts_with("LPT")))
}

/// Lexical `..` / `.` normalisation (does not touch the filesystem).
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                let _ = out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// True if `path` is `root` or a descendant (lexical, component-wise).
pub fn is_under(path: &Path, root: &Path) -> bool {
    let path = normalize_path(path);
    let root = normalize_path(root);
    path == root || path.starts_with(&root)
}

/// Client-supplied output folders must stay inside the user's configured output directory.
pub fn constrain_output_dir(requested: Option<&Path>, configured: &Path) -> Result<PathBuf, String> {
    let root = if configured.is_absolute() {
        normalize_path(configured)
    } else {
        normalize_path(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(configured))
    };
    let Some(req) = requested else {
        return Ok(root);
    };
    if req.as_os_str().is_empty() {
        return Ok(root);
    }
    let candidate = if req.is_absolute() { normalize_path(req) } else { normalize_path(&root.join(req)) };
    if is_under(&candidate, &root) {
        Ok(candidate)
    } else {
        Err("output_dir must be inside the EncodeCraft output folder".into())
    }
}

/// If `path` already exists and overwrites are off, append `-2`, `-3`, …
pub fn unique_output_path(path: PathBuf, overwrite: bool) -> PathBuf {
    if overwrite {
        return path;
    }
    let is_seq = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains('%'));
    let conflict = if is_seq { path.parent().map(|p| p.exists()).unwrap_or(false) } else { path.exists() };
    if !conflict {
        return path;
    }
    for n in 2..10_000u32 {
        if is_seq {
            let seq_dir = match path.parent() {
                Some(p) => p,
                None => return path,
            };
            let parent = seq_dir.parent().unwrap_or(Path::new("."));
            let stem = seq_dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "seq".into());
            let new_dir = parent.join(format!("{stem} ({n})"));
            if !new_dir.exists() {
                return new_dir.join(path.file_name().unwrap_or_default());
            }
        } else {
            let parent = path.parent().unwrap_or(Path::new("."));
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
            let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let cand = parent.join(format!("{stem} ({n}){ext}"));
            if !cand.exists() {
                return cand;
            }
        }
    }
    path
}

fn local_stamp() -> (String, String) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    // Calendar date without extra crates: approximate via UTC. Good enough for filenames.
    let days = now / 86400;
    let secs = now % 86400;
    let (y, m, d) = civil_from_days(days as i64);
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    let ss = secs % 60;
    (format!("{y:04}-{m:02}-{d:02}"), format!("{hh:02}{mm:02}{ss:02}"))
}

/// Howard Hinnant civil-from-days (UTC).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExportPreset, OutputFormat, PresetKind, VideoSettings};

    fn dummy() -> ExportPreset {
        ExportPreset {
            id: "system.h264-mp4".into(),
            name: "H.264 Match Source".into(),
            kind: PresetKind::System,
            format: OutputFormat::H264Mp4,
            video: VideoSettings { width: Some(1920), height: Some(1080), ..VideoSettings::default() },
            audio: Default::default(),
            one_off: false,
            group: String::new(),
        }
    }

    #[test]
    fn template_expands_size() {
        let ctx = NamingContext::from_preset("My Ad!", &dummy(), 1920, 1080, "job-1", 1, "FHD Landscape");
        assert_eq!(apply_template("{name}_{width}x{height}", &ctx), "My Ad!_1920x1080");
    }

    #[test]
    fn template_strips_path_separators() {
        let mut ctx = NamingContext::from_preset("clip", &dummy(), 100, 100, "j", 1, "");
        ctx.name = "a/b\\c".into();
        let s = apply_template("{name}", &ctx);
        assert!(!s.contains('/'));
        assert!(!s.contains('\\'));
    }

    #[test]
    fn unknown_token_kept() {
        let ctx = NamingContext::from_preset("clip", &dummy(), 10, 10, "j", 1, "");
        assert!(apply_template("{name}_{nope}", &ctx).contains("{nope}"));
    }

    #[test]
    fn sequence_path_uses_pattern() {
        let ctx = NamingContext::from_preset("clip", &dummy(), 1920, 1080, "j", 1, "");
        let p = output_path(Path::new("/out"), "{name}", &ctx, true);
        assert!(p.to_string_lossy().contains("frame_%06d"));
    }

    #[test]
    fn sanitize_blocks_traversal_reserved_and_dash() {
        assert!(!sanitize_filename("../etc/passwd").contains('/'));
        assert!(!sanitize_filename("a\\b").contains('\\'));
        assert!(sanitize_filename("CON").starts_with('_'));
        assert!(sanitize_filename("nul.txt").to_ascii_uppercase().starts_with("_NUL"));
        assert!(sanitize_filename("-ss").starts_with('_'));
        assert_eq!(sanitize_filename("إعلان"), "إعلان");
        assert!(sanitize_filename("my clip").contains(' '));
    }

    #[test]
    fn constrain_rejects_escape() {
        let root = Path::new("/home/user/EncodeCraft");
        assert!(constrain_output_dir(Some(Path::new("/etc")), root).is_err());
        assert!(constrain_output_dir(Some(Path::new("/home/user/EncodeCraft/../secret")), root).is_err());
        let ok = constrain_output_dir(Some(Path::new("dooh")), root).unwrap();
        assert!(is_under(&ok, root));
        let abs = constrain_output_dir(Some(Path::new("/home/user/EncodeCraft/batch")), root).unwrap();
        assert_eq!(normalize_path(&abs), normalize_path(&root.join("batch")));
    }

    #[test]
    fn unique_path_appends_number() {
        let dir = std::env::temp_dir().join(format!(
            "ec-unique-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("clip.mp4");
        std::fs::write(&p, b"x").unwrap();
        let u = unique_output_path(p.clone(), false);
        assert_ne!(u, p);
        let name = u.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.contains("clip (2)"), "{name}");
        let same = unique_output_path(p.clone(), true);
        assert_eq!(same, p);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
