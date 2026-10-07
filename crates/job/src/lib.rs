//! Shared job types for EncodeCraft and EffectCraft.
//!
//! This crate is intentionally small — `serde` and `thiserror` only — so EffectCraft (or any other
//! sibling) can depend on it without pulling FFmpeg, egui, or the rest of the encoder.
//!
//! Wire format: JSON, schema version [`SCHEMA`]. See `docs/job-format.md` in the EncodeCraft repo.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Current JSON schema version for [`EnqueueRequest`] and related types.
pub const SCHEMA: u32 = 1;

/// Default TCP/HTTP port for the encoder control server (`127.0.0.1` only).
/// EffectCraft's control channel is 9877; EncodeCraft sits on the next port.
pub const DEFAULT_CONTROL_PORT: u16 = 9878;

/// Product name (window title, lock file, user-facing copy).
pub const PRODUCT: &str = "EncodeCraft";

/// Machine id (`ai.storyteller` style ids stay lowercase).
pub const APP_ID: &str = "encodecraft";

/// Environment variable that overrides the data directory (used in tests).
pub const HOME_ENV: &str = "ENCODECRAFT_HOME";

/// Environment variable that overrides the FFmpeg binary path.
pub const FFMPEG_ENV: &str = "FFMPEG_PATH";

/// Environment variable that overrides `effectcraft-cli`.
pub const EFFECTCRAFT_CLI_ENV: &str = "EFFECTCRAFT_CLI";

/// Environment variable that overrides the control/HTTP port.
pub const PORT_ENV: &str = "ENCODECRAFT_PORT";

/// Shared secret for the loopback control server. Prefer the header; env is a fallback.
pub const TOKEN_ENV: &str = "ENCODECRAFT_TOKEN";

/// HTTP header carrying the control-server token (`X-EncodeCraft-Token`).
pub const TOKEN_HEADER: &str = "X-EncodeCraft-Token";

/// File name of the per-install token inside the EncodeCraft data directory.
pub const TOKEN_FILE: &str = "ipc-token";

/// Maximum POST / JSON-lines body we will parse (enqueue JSON is tiny).
pub const MAX_CONTROL_BODY: usize = 256 * 1024;

/// Hard cap on one-source-many-sizes expansion.
pub const MAX_SIZES: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("unsupported job schema {found} (this encoder understands 1..={max})")]
    Schema { found: u32, max: u32 },
    #[error("{0}")]
    Message(String),
}

/// How to scale a source into a target frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScaleMode {
    /// Uniform scale to fit inside the frame; pad with black (letterbox/pillarbox).
    #[default]
    Fit,
    /// Uniform scale to fill the frame; crop overflow.
    Fill,
    /// Ignore aspect ratio; squash to exact width × height.
    Stretch,
    /// Uniform scale to fit; do **not** pad (output may be smaller than the target).
    Scale,
}

impl ScaleMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ScaleMode::Fit => "fit",
            ScaleMode::Fill => "fill",
            ScaleMode::Stretch => "stretch",
            ScaleMode::Scale => "scale",
        }
    }

    pub fn parse_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fit" | "letterbox" | "contain" => Some(ScaleMode::Fit),
            "fill" | "cover" | "crop" => Some(ScaleMode::Fill),
            "stretch" | "exact" => Some(ScaleMode::Stretch),
            "scale" | "fit-nopad" | "decrease" => Some(ScaleMode::Scale),
            _ => None,
        }
    }
}

/// In/out trim in seconds of **source** time. `out_sec = None` means through the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trim {
    #[serde(default)]
    pub in_sec: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_sec: Option<f64>,
}

impl Trim {
    pub fn duration_sec(&self, source_duration: Option<f64>) -> Option<f64> {
        let start = self.in_sec.max(0.0);
        match (self.out_sec, source_duration) {
            (Some(out), _) if out > start => Some(out - start),
            (None, Some(d)) if d > start => Some(d - start),
            _ => None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.in_sec > 0.001 || self.out_sec.is_some()
    }
}

/// A named output raster size. Used for DOOH screen packs and "one source → many sizes".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSize {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

impl ScreenSize {
    pub fn new(name: impl Into<String>, width: u32, height: u32) -> Self {
        Self { name: name.into(), width, height }
    }

    pub fn label(&self) -> String {
        format!("{} ({}×{})", self.name, self.width, self.height)
    }

    /// Parse `1920x1080` or `1920×1080`.
    pub fn parse_dims(s: &str) -> Option<(u32, u32)> {
        let t = s.trim().to_ascii_lowercase().replace('×', "x");
        let mut parts = t.split('x');
        let w = parts.next()?.trim().parse::<u32>().ok()?;
        let h = parts.next()?.trim().parse::<u32>().ok()?;
        if parts.next().is_some() || w == 0 || h == 0 {
            return None;
        }
        Some((w, h))
    }
}

/// Where the pixels come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum JobSource {
    /// An already-rendered media file (video, audio, image sequence glob).
    File { path: PathBuf },
    /// An EffectCraft composition. EncodeCraft renders a high-quality mezzanine through
    /// `effectcraft-cli render`, then encodes that with FFmpeg to the delivery preset.
    /// This is the Adobe Media Encoder analogue: AE does not pre-encode H.264; AME does.
    EffectCraft {
        project: PathBuf,
        /// Composition name or id. Omit to use EffectCraft's active / first comp.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comp: Option<String>,
        /// Mezzanine format passed to `effectcraft-cli render --format`.
        #[serde(default = "default_mezzanine")]
        mezzanine: String,
        /// Optional work-area-only render.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        work_area: bool,
    },
}

fn default_mezzanine() -> String {
    "prores".into()
}

impl JobSource {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        JobSource::File { path: path.into() }
    }

    pub fn display_name(&self) -> String {
        match self {
            JobSource::File { path } => path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned()),
            JobSource::EffectCraft { project, comp, .. } => {
                let proj = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| project.to_string_lossy().into_owned());
                match comp {
                    Some(c) => format!("{proj} / {c}"),
                    None => proj,
                }
            }
        }
    }

    pub fn path_hint(&self) -> &std::path::Path {
        match self {
            JobSource::File { path } => path,
            JobSource::EffectCraft { project, .. } => project,
        }
    }
}

/// A request to add one or more jobs to the encoder queue.
///
/// If [`sizes`](Self::sizes) is non-empty, the encoder expands the request into one job per size
/// (the DOOH "one source, many screens" path). Otherwise a single job uses the preset's resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnqueueRequest {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub source: JobSource,
    /// Builtin or user preset id, for example `system.h264-mp4`.
    pub preset_id: String,
    /// Optional full preset JSON overlay (merged on top of the named preset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_override: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,
    /// Output file name template, e.g. `{name}_{width}x{height}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub naming: Option<String>,
    /// When non-empty, one queue item is created per size.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sizes: Vec<ScreenSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<Trim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<ScaleMode>,
    /// If true, start the queue after accepting the job (AME's "start queue" behaviour).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub start_queue: bool,
}

fn default_schema() -> u32 {
    SCHEMA
}

impl EnqueueRequest {
    pub fn validate(&self) -> Result<(), JobError> {
        if self.schema == 0 || self.schema > SCHEMA {
            return Err(JobError::Schema { found: self.schema, max: SCHEMA });
        }
        if self.preset_id.trim().is_empty() {
            return Err(JobError::Message("preset_id is required".into()));
        }
        match &self.source {
            JobSource::File { path } if path.as_os_str().is_empty() => {
                return Err(JobError::Message("source path is empty".into()));
            }
            JobSource::EffectCraft { project, .. } if project.as_os_str().is_empty() => {
                return Err(JobError::Message("effectcraft project path is empty".into()));
            }
            _ => {}
        }
        if self.sizes.len() > MAX_SIZES {
            return Err(JobError::Message(format!("too many sizes (max {MAX_SIZES})")));
        }
        for sz in &self.sizes {
            if sz.width == 0 || sz.height == 0 {
                return Err(JobError::Message(format!("invalid screen size {}", sz.label())));
            }
        }
        Ok(())
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, JobError> {
        let req: Self = serde_json::from_slice(bytes).map_err(|e| JobError::Message(e.to_string()))?;
        req.validate()?;
        Ok(req)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, JobError> {
        serde_json::to_vec_pretty(self).map_err(|e| JobError::Message(e.to_string()))
    }
}

/// Reply from `POST /v1/enqueue` or a JSON-lines `enqueue` method.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnqueueReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// JSON-lines / HTTP control envelope (mirrors EffectCraft's `{id, method, params}`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlRequest {
    #[serde(default)]
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    /// Required on the JSON-lines channel (HTTP uses [`TOKEN_HEADER`] instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Load the per-install control token (env, then `ENCODECRAFT_HOME`, then the platform data dir).
///
/// EffectCraft should call this and send the value as [`TOKEN_HEADER`]. The file is created
/// by EncodeCraft on first launch; it is not a password the user types.
pub fn discover_ipc_token() -> Option<String> {
    if let Ok(t) = std::env::var(TOKEN_ENV) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    for dir in ipc_token_dirs() {
        if let Ok(s) = std::fs::read_to_string(dir.join(TOKEN_FILE)) {
            let t = s.trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

fn ipc_token_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(home) = std::env::var(HOME_ENV) {
        v.push(PathBuf::from(home));
    }
    if cfg!(windows) {
        if let Some(base) = std::env::var_os("APPDATA") {
            v.push(PathBuf::from(base).join("EncodeCraft").join("EncodeCraft"));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            v.push(PathBuf::from(home).join("Library/Application Support/dev.EncodeCraft.EncodeCraft"));
        }
    } else if let Some(home) = std::env::var_os("HOME") {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            v.push(PathBuf::from(xdg).join(APP_ID));
        }
        v.push(PathBuf::from(home).join(".local/share").join(APP_ID));
    }
    v
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlReply {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_file_job() {
        let req = EnqueueRequest {
            schema: SCHEMA,
            source: JobSource::file("/clips/ad.mp4"),
            preset_id: "system.h264-mp4".into(),
            preset_override: None,
            output_dir: Some(PathBuf::from("/out")),
            naming: Some("{name}_{width}x{height}".into()),
            sizes: vec![ScreenSize::new("FHD", 1920, 1080), ScreenSize::new("Portrait", 1080, 1920)],
            trim: Some(Trim { in_sec: 1.0, out_sec: Some(5.0) }),
            scale: Some(ScaleMode::Fit),
            start_queue: true,
        };
        let bytes = req.to_vec().unwrap();
        let back = EnqueueRequest::from_json(&bytes).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn roundtrip_effectcraft_source() {
        let json = r#"{
            "schema": 1,
            "source": {"kind": "effectcraft", "project": "/p/main.ecproj", "comp": "Main"},
            "preset_id": "system.h264-mp4",
            "sizes": [{"name": "DOOH-A", "width": 1200, "height": 960}]
        }"#;
        let req = EnqueueRequest::from_json(json.as_bytes()).unwrap();
        match req.source {
            JobSource::EffectCraft { comp, mezzanine, .. } => {
                assert_eq!(comp.as_deref(), Some("Main"));
                assert_eq!(mezzanine, "prores");
            }
            JobSource::File { .. } => panic!("expected effectcraft source"),
        }
        assert_eq!(req.sizes[0].width, 1200);
    }

    #[test]
    fn rejects_future_schema() {
        let json = r#"{"schema": 99, "source": {"kind": "file", "path": "a.mp4"}, "preset_id": "x"}"#;
        let err = EnqueueRequest::from_json(json.as_bytes()).unwrap_err();
        assert!(matches!(err, JobError::Schema { found: 99, .. }));
    }

    #[test]
    fn parse_dims() {
        assert_eq!(ScreenSize::parse_dims("1920x1080"), Some((1920, 1080)));
        assert_eq!(ScreenSize::parse_dims("1080×1920"), Some((1080, 1920)));
        assert_eq!(ScreenSize::parse_dims("bad"), None);
    }

    #[test]
    fn scale_mode_aliases() {
        assert_eq!(ScaleMode::parse_name("contain"), Some(ScaleMode::Fit));
        assert_eq!(ScaleMode::parse_name("cover"), Some(ScaleMode::Fill));
    }

    #[test]
    fn rejects_too_many_sizes() {
        let sizes: Vec<ScreenSize> = (0..MAX_SIZES + 1).map(|i| ScreenSize::new(format!("s{i}"), 64, 64)).collect();
        let req = EnqueueRequest {
            schema: SCHEMA,
            source: JobSource::file("a.mp4"),
            preset_id: "x".into(),
            preset_override: None,
            output_dir: None,
            naming: None,
            sizes,
            trim: None,
            scale: None,
            start_queue: false,
        };
        assert!(req.validate().is_err());
    }
}
