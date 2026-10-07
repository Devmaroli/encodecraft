//! Output file naming templates.
//!
//! Tokens: `{name}`, `{width}`, `{height}`, `{preset}`, `{codec}`, `{ext}`, `{date}`, `{time}`,
//! `{job}`, `{index}`, `{screen}`. Unknown tokens are left in place. Path separators in values
//! are replaced so a template cannot escape the output directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::ExportPreset;

pub fn default_template() -> &'static str {
    "{name}_{width}x{height}"
}

#[derive(Clone, Debug, Default)]
pub struct NamingContext {
    pub name: String,
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
        Self {
            name: sanitize_filename(source_name),
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
        m.insert("width".into(), self.width.to_string());
        m.insert("height".into(), self.height.to_string());
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
    let t = out.trim().trim_matches('.');
    if t.is_empty() {
        "untitled".into()
    } else {
        t.to_string()
    }
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
}
