//! Shared job types for EncodeCraft and EffectCraft.
//!
//! This crate is intentionally small — `serde` and `thiserror` only — so EffectCraft (or any other
//! sibling) can depend on it without pulling FFmpeg, egui, or the rest of the encoder.
//!
//! Wire format: JSON, schema version [`SCHEMA`]. See `docs/job-format.md` in the EncodeCraft repo.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};

/// Current JSON schema version for [`EnqueueRequest`] and related types.
pub const SCHEMA: u32 = 1;

/// String form used by EffectCraft 0.4's vendored job crate. EncodeCraft maps it to [`SCHEMA`].
pub const SCHEMA_ID: &str = "encodecraft.job/v1";

/// Default TCP/HTTP port for the encoder control server (`127.0.0.1` only).
/// EffectCraft's control channel is 9877; EncodeCraft sits on the next port.
pub const DEFAULT_CONTROL_PORT: u16 = 9878;

/// Product name (window title, lock file, user-facing copy).
pub const PRODUCT: &str = "EncodeCraft";

/// Machine id (`ai.storyteller` style ids stay lowercase).
pub const APP_ID: &str = "encodecraft";

/// Environment variable that overrides the data directory (used in tests).
pub const HOME_ENV: &str = "ENCODECRAFT_HOME";

/// Marker file next to `encodecraft.exe`. When present, queue/presets/settings
/// live in a `data` folder beside the executable instead of `%APPDATA%`.
pub const PORTABLE_MARKER: &str = "portable.txt";

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

/// Strip a Windows long-path / verbatim prefix for display and Explorer.
///
/// `\\?\C:\foo` → `C:\foo`. `\\?\UNC\server\share` → `\\server\share`.
/// Internal encode paths may keep the prefix; UI and `explorer.exe` should not.
pub fn strip_verbatim_path(s: &str) -> String {
    const UNC: &str = r"\\?\UNC\";
    const VERB: &str = r"\\?\";
    if let Some(rest) = strip_prefix_ignore_ascii_case(s, UNC) {
        let mut out = String::from(r"\\");
        out.push_str(rest);
        return out;
    }
    if let Some(rest) = strip_prefix_ignore_ascii_case(s, VERB) {
        return rest.to_string();
    }
    if let Some(rest) = s.strip_prefix("//?/UNC/") {
        return format!("//{rest}");
    }
    if let Some(rest) = s.strip_prefix("//?/") {
        return rest.to_string();
    }
    s.to_string()
}

fn strip_prefix_ignore_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes()) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// [`strip_verbatim_path`] applied to a filesystem path.
pub fn display_path(path: impl AsRef<Path>) -> String {
    strip_verbatim_path(&path.as_ref().to_string_lossy())
}

/// Same as [`display_path`] as a `PathBuf` (Explorer / exists checks).
pub fn display_path_buf(path: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(display_path(path))
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("unsupported job schema {found} (EncodeCraft understands 1 or \"encodecraft.job/v1\")")]
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
    /// An already-rendered media file (video, audio, or the first frame of an image sequence).
    File {
        path: PathBuf,
        /// When set, `path` is the first frame of a numbered still sequence. FFmpeg is given
        /// `name_%0Nd.ext` plus `-start_number`. Omitted in JSON for ordinary files.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence_start: Option<u32>,
    },
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
        /// Comp raster size when the sender knows it (used for `{width}`/`{height}` naming).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
    },
}

fn default_mezzanine() -> String {
    "prores".into()
}

impl JobSource {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        JobSource::File { path: path.into(), sequence_start: None }
    }

    /// Numbered stills (`shot_0001.png`, `shot_0002.png`, …) queued as one job.
    pub fn image_sequence(first_frame: impl Into<PathBuf>, start: u32) -> Self {
        JobSource::File { path: first_frame.into(), sequence_start: Some(start) }
    }

    pub fn display_name(&self) -> String {
        match self {
            JobSource::File { .. } => self.comp_name(),
            JobSource::EffectCraft { project, comp, .. } => {
                let proj = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| project.to_string_lossy().into_owned());
                match comp.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                    Some(c) => format!("{proj} / {c}"),
                    None => proj,
                }
            }
        }
    }

    /// Stem used for the output file: composition name, or the dropped file's base name.
    pub fn comp_name(&self) -> String {
        match self {
            JobSource::File { path, sequence_start } => {
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned());
                if sequence_start.is_some() {
                    strip_frame_number(&stem).unwrap_or(stem)
                } else {
                    stem
                }
            }
            JobSource::EffectCraft { project, comp, .. } => match comp.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                Some(c) => c.to_string(),
                None => project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| project.to_string_lossy().into_owned()),
            },
        }
    }

    /// EffectCraft project stem; empty for dropped media.
    pub fn project_name(&self) -> String {
        match self {
            JobSource::File { .. } => String::new(),
            JobSource::EffectCraft { project, .. } => {
                project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| project.to_string_lossy().into_owned())
            }
        }
    }

    pub fn path_hint(&self) -> &std::path::Path {
        match self {
            JobSource::File { path, .. } => path,
            JobSource::EffectCraft { project, .. } => project,
        }
    }

    /// `Output` folder next to the `.ecproj` or dropped source file.
    pub fn sidecar_output_dir(&self) -> PathBuf {
        let parent = self.path_hint().parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
        parent.join("Output")
    }

    /// Group key so one EffectCraft project / one file shows as a parent row with child outputs.
    pub fn group_key(&self) -> String {
        match self {
            JobSource::File { path, .. } => format!("file:{}", path.to_string_lossy()),
            JobSource::EffectCraft { project, .. } => format!("ec:{}", project.to_string_lossy()),
        }
    }

    pub fn group_title(&self) -> String {
        match self {
            JobSource::File { path, .. } => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned()),
            JobSource::EffectCraft { project, .. } => {
                project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| project.to_string_lossy().into_owned())
            }
        }
    }

    pub fn is_effectcraft(&self) -> bool {
        matches!(self, JobSource::EffectCraft { .. })
    }

    /// Pixel size carried on the request (EffectCraft comp size). `None` until probed.
    pub fn pixel_size(&self) -> Option<(u32, u32)> {
        match self {
            JobSource::EffectCraft { width, height, .. } => match (*width, *height) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            },
            JobSource::File { .. } => None,
        }
    }
}

/// A request to add one or more jobs to the encoder queue.
///
/// If [`sizes`](Self::sizes) is non-empty, the encoder expands the request into one job per size
/// (the DOOH "one source, many screens" path). Otherwise a single job uses the preset's resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnqueueRequest {
    /// Integer `1` or the string `"encodecraft.job/v1"` (EffectCraft 0.4). Serialized as `1`.
    #[serde(default = "default_schema", deserialize_with = "deserialize_schema")]
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

fn deserialize_schema<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    struct SchemaVisitor;
    impl<'de> Visitor<'de> for SchemaVisitor {
        type Value = u32;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "1 or \"{SCHEMA_ID}\"")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
            u32::try_from(v).map_err(|_| E::custom(format!("job schema {v} is out of range")))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u32, E> {
            if v < 0 {
                return Err(E::custom(format!("job schema {v} is invalid (EncodeCraft understands 1 or \"{SCHEMA_ID}\")")));
            }
            self.visit_u64(v as u64)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<u32, E> {
            parse_schema_token(v).map_err(E::custom)
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<u32, E> {
            self.visit_str(&v)
        }
    }
    deserializer.deserialize_any(SchemaVisitor)
}

/// Map a schema token (integer-as-text or [`SCHEMA_ID`]) to a version number.
pub fn parse_schema_token(s: &str) -> Result<u32, String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case(SCHEMA_ID) || t == "1" {
        return Ok(1);
    }
    if let Ok(n) = t.parse::<u32>() {
        return Ok(n);
    }
    Err(format!("unknown job schema `{t}`. EncodeCraft understands 1 or \"{SCHEMA_ID}\"."))
}

fn humanize_json_error(e: &serde_json::Error) -> String {
    let loc = format!("line {} column {}", e.line(), e.column());
    let full = e.to_string();
    let core = full.split(" at line ").next().unwrap_or(&full);
    if core.contains("job schema") {
        return format!("{core} ({loc})");
    }
    format!("Could not parse enqueue JSON ({loc}): {core}. EncodeCraft understands schema 1 or \"{SCHEMA_ID}\"; extra fields such as source_version are ignored.")
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
            JobSource::File { path, .. } if path.as_os_str().is_empty() => {
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
        let req: Self = serde_json::from_slice(bytes).map_err(|e| JobError::Message(humanize_json_error(&e)))?;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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
///
/// On Windows the file lives at `%APPDATA%\EncodeCraft\EncodeCraft\data\ipc-token`
/// (`directories::ProjectDirs` `data_dir`). Older snippets looked one folder up (without `\data`);
/// [`ipc_token_dirs`] still checks that location as a fallback.
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

/// Directories EncodeCraft (and EffectCraft) should search for [`TOKEN_FILE`].
pub fn ipc_token_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(home) = std::env::var(HOME_ENV) {
        let p = PathBuf::from(home);
        if !p.as_os_str().is_empty() {
            v.push(p);
        }
    }
    if let Some(p) = portable_data_dir() {
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v.extend(platform_token_dirs());
    v
}

/// Canonical user-data directory (queue, presets, `ipc-token`, inbox).
///
/// Order: `ENCODECRAFT_HOME`, then a [`PORTABLE_MARKER`] next to the executable
/// (`<exe dir>/data`), then the platform folder:
/// Windows `%APPDATA%\EncodeCraft\EncodeCraft\data`, macOS
/// `~/Library/Application Support/dev.EncodeCraft.EncodeCraft`, Linux
/// `$XDG_DATA_HOME/encodecraft` or `~/.local/share/encodecraft`.
pub fn canonical_data_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var(HOME_ENV) {
        let p = PathBuf::from(home);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    if let Some(p) = portable_data_dir() {
        return Some(p);
    }
    platform_data_dir()
}

/// `Some` when `portable.txt` sits next to the running executable.
pub fn portable_data_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    portable_data_dir_in(exe.parent()?)
}

/// Data folder used when [`PORTABLE_MARKER`] is in `exe_dir`.
pub fn portable_data_dir_in(exe_dir: impl AsRef<std::path::Path>) -> Option<PathBuf> {
    let dir = exe_dir.as_ref();
    if dir.join(PORTABLE_MARKER).is_file() {
        Some(dir.join("data"))
    } else {
        None
    }
}

/// Inbox folder inside [`canonical_data_dir`].
pub fn canonical_inbox_dir() -> Option<PathBuf> {
    Some(canonical_data_dir()?.join("inbox"))
}

/// Windows data dir used by the real app: `%APPDATA%\EncodeCraft\EncodeCraft\data`.
pub fn windows_data_dir(appdata: impl Into<PathBuf>) -> PathBuf {
    appdata.into().join("EncodeCraft").join("EncodeCraft").join("data")
}

/// Token search order on Windows: `...\data` first, then the pre-fix folder without `\data`.
pub fn windows_token_search_dirs(appdata: impl Into<PathBuf>) -> Vec<PathBuf> {
    let root = appdata.into().join("EncodeCraft").join("EncodeCraft");
    vec![root.join("data"), root]
}

pub fn windows_inbox_dir(appdata: impl Into<PathBuf>) -> PathBuf {
    windows_data_dir(appdata).join("inbox")
}

fn platform_data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(windows_data_dir)
    } else if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Application Support/dev.EncodeCraft.EncodeCraft"))
    } else if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        Some(PathBuf::from(xdg).join(APP_ID))
    } else {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".local/share").join(APP_ID))
    }
}

fn platform_token_dirs() -> Vec<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(windows_token_search_dirs).unwrap_or_default()
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(macos_token_search_dirs).unwrap_or_default()
    } else {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let xdg = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
        match home {
            Some(h) => linux_token_search_dirs(h, xdg),
            None => xdg.map(|p| vec![p.join(APP_ID), p.join(APP_ID).join("data")]).unwrap_or_default(),
        }
    }
}

/// macOS: ProjectDirs `data_dir` first, then a `\data`-style extra folder and a qualifier-less path.
pub fn macos_token_search_dirs(home: impl Into<PathBuf>) -> Vec<PathBuf> {
    let support = home.into().join("Library/Application Support");
    let canonical = support.join("dev.EncodeCraft.EncodeCraft");
    vec![canonical.clone(), canonical.join("data"), support.join("EncodeCraft").join("EncodeCraft")]
}

/// Linux: `$XDG_DATA_HOME/encodecraft` or `~/.local/share/encodecraft`, plus a `data/` fallback.
pub fn linux_token_search_dirs(home: impl Into<PathBuf>, xdg_data_home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(xdg) = xdg_data_home {
        v.push(xdg.join(APP_ID));
        v.push(xdg.join(APP_ID).join("data"));
    }
    let share = home.into().join(".local/share").join(APP_ID);
    if !v.contains(&share) {
        v.push(share.clone());
        v.push(share.join("data"));
    }
    v
}

fn strip_frame_number(stem: &str) -> Option<String> {
    let bytes = stem.as_bytes();
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1].is_ascii_digit() {
        i -= 1;
    }
    if i == 0 || i == bytes.len() {
        return None;
    }
    let mut prefix = stem[..i].to_string();
    while prefix.ends_with('_') || prefix.ends_with('-') || prefix.ends_with('.') {
        prefix.pop();
    }
    if prefix.is_empty() {
        None
    } else {
        Some(prefix)
    }
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
        let msg = err.to_string();
        assert!(msg.contains("99"));
        assert!(msg.contains("encodecraft.job/v1"));
    }

    #[test]
    fn accepts_effectcraft_0_4_schema_string_and_source_version() {
        // Exact wire shape from EffectCraft 0.4.0's vendored job crate (live 400 on EncodeCraft 0.1.0).
        let json = r#"{
            "schema": "encodecraft.job/v1",
            "source_version": 1,
            "source": {
                "kind": "effectcraft",
                "project": "C:\\Users\\Ada\\Documents\\spot.ecproj",
                "comp": "Main",
                "mezzanine": "prores",
                "work_area": false,
                "source_version": "ignored-on-source-too"
            },
            "preset_id": "system.h264-mp4",
            "start_queue": true
        }"#;
        let req = EnqueueRequest::from_json(json.as_bytes()).unwrap();
        assert_eq!(req.schema, 1);
        assert_eq!(req.preset_id, "system.h264-mp4");
        assert!(req.start_queue);
        match req.source {
            JobSource::EffectCraft { ref project, ref comp, ref mezzanine, work_area, width, height } => {
                assert!(project.to_string_lossy().ends_with("spot.ecproj"));
                assert_eq!(comp.as_deref(), Some("Main"));
                assert_eq!(mezzanine, "prores");
                assert!(!work_area);
                assert!(width.is_none() && height.is_none());
            }
            JobSource::File { .. } => panic!("expected effectcraft source"),
        }
    }

    #[test]
    fn unknown_schema_string_is_human_readable() {
        let json = r#"{"schema":"encodecraft.job/v9","source":{"kind":"file","path":"a.mp4"},"preset_id":"x"}"#;
        let err = EnqueueRequest::from_json(json.as_bytes()).unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("invalid type"));
        assert!(msg.contains("encodecraft.job/v9"));
        assert!(msg.contains("encodecraft.job/v1"));
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
    fn portable_marker_uses_data_folder_next_to_exe() {
        let root = std::env::temp_dir().join(format!("ec-portable-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        assert!(portable_data_dir_in(&root).is_none());
        std::fs::write(root.join(PORTABLE_MARKER), "portable\n").unwrap();
        assert_eq!(portable_data_dir_in(&root), Some(root.join("data")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn windows_token_dirs_include_data_subdir() {
        let dirs = windows_token_search_dirs(r"C:\Users\Ada\AppData\Roaming");
        let data = dirs[0].to_string_lossy();
        let root = dirs[1].to_string_lossy();
        assert!(data.replace('\\', "/").ends_with("EncodeCraft/EncodeCraft/data"), "{data}");
        assert!(root.replace('\\', "/").ends_with("EncodeCraft/EncodeCraft"), "{root}");
        assert!(!root.replace('\\', "/").ends_with("/data"));
        let inbox = windows_inbox_dir(r"C:\Users\Ada\AppData\Roaming").to_string_lossy().replace('\\', "/");
        assert!(inbox.ends_with("EncodeCraft/EncodeCraft/data/inbox"), "{inbox}");
    }

    #[test]
    fn macos_and_linux_token_dirs_match_projectdirs() {
        let mac = macos_token_search_dirs("/Users/ada");
        assert_eq!(mac[0], PathBuf::from("/Users/ada/Library/Application Support/dev.EncodeCraft.EncodeCraft"));
        assert!(mac.iter().any(|p| p.ends_with("data")));
        let lin = linux_token_search_dirs("/home/ada", None);
        assert_eq!(lin[0], PathBuf::from("/home/ada/.local/share/encodecraft"));
        let xdg = linux_token_search_dirs("/home/ada", Some(PathBuf::from("/home/ada/.data")));
        assert_eq!(xdg[0], PathBuf::from("/home/ada/.data/encodecraft"));
    }

    #[test]
    fn sidecar_output_dir_is_output_next_to_project_or_file() {
        let ec = JobSource::EffectCraft {
            project: PathBuf::from("/Users/ASUS/OneDrive/Desktop/test-2.ecproj"),
            comp: Some("Main".into()),
            mezzanine: "prores".into(),
            work_area: false,
            width: None,
            height: None,
        };
        assert_eq!(ec.sidecar_output_dir(), PathBuf::from("/Users/ASUS/OneDrive/Desktop/Output"));
        let file = JobSource::file("/Users/ASUS/Videos/clip.mov");
        assert_eq!(file.sidecar_output_dir(), PathBuf::from("/Users/ASUS/Videos/Output"));
    }

    #[test]
    fn image_sequence_display_name_strips_frame_number() {
        let src = JobSource::image_sequence("/clips/hero_0001.png", 1);
        assert_eq!(src.display_name(), "hero");
        assert_eq!(JobSource::file("/clips/hero.mp4").display_name(), "hero");
    }

    #[test]
    fn file_json_omits_sequence_start() {
        let req = EnqueueRequest {
            schema: SCHEMA,
            source: JobSource::file("a.mp4"),
            preset_id: "x".into(),
            preset_override: None,
            output_dir: None,
            naming: None,
            sizes: vec![],
            trim: None,
            scale: None,
            start_queue: false,
        };
        let s = String::from_utf8(req.to_vec().unwrap()).unwrap();
        assert!(!s.contains("sequence_start"));
    }

    #[test]
    fn effectcraft_tagged_source_reads_comp_size() {
        let json = r#"{
            "schema": 1,
            "source": {
                "kind": "effectcraft",
                "project": "C:\\work\\spot.ecproj",
                "comp": "Honor 400_EN.ai Comp",
                "width": 1080,
                "height": 1080
            },
            "preset_id": "system.h264-mp4",
            "start_queue": true
        }"#;
        let req = EnqueueRequest::from_json(json.as_bytes()).unwrap();
        assert_eq!(req.source.pixel_size(), Some((1080, 1080)));
        match req.source {
            JobSource::EffectCraft { ref comp, width, height, .. } => {
                assert_eq!(comp.as_deref(), Some("Honor 400_EN.ai Comp"));
                assert_eq!(width, Some(1080));
                assert_eq!(height, Some(1080));
            }
            JobSource::File { .. } => panic!("expected effectcraft"),
        }
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

    #[test]
    fn strip_verbatim_path_windows_long_and_unc() {
        assert_eq!(
            strip_verbatim_path(r"\\?\C:\Users\ASUS\Documents\Craft\Output\InstallTest _ Eye of Kuwait_1920x1080.mp4"),
            r"C:\Users\ASUS\Documents\Craft\Output\InstallTest _ Eye of Kuwait_1920x1080.mp4"
        );
        assert_eq!(
            strip_verbatim_path(r"\\?\UNC\fileserver\share\Output\out.mp4"),
            r"\\fileserver\share\Output\out.mp4"
        );
        assert_eq!(strip_verbatim_path(r"C:\Users\ASUS\out.mp4"), r"C:\Users\ASUS\out.mp4");
        assert_eq!(strip_verbatim_path("/home/ada/out.mp4"), "/home/ada/out.mp4");
        assert_eq!(
            display_path(Path::new(r"\\?\C:\Users\ASUS\Documents\spot.ecproj")),
            r"C:\Users\ASUS\Documents\spot.ecproj"
        );
        assert_eq!(
            display_path_buf(Path::new(r"\\?\UNC\office\jobs\spot.ecproj")),
            PathBuf::from(r"\\office\jobs\spot.ecproj")
        );
    }
}
