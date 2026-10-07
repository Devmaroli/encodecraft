//! Export presets, DOOH screen-size packs, and `{name}_{width}x{height}` naming templates.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod naming;

pub use naming::{
    apply_template, constrain_output_dir, default_template, is_under, normalize_path, output_path, sanitize_filename, unique_output_path, NamingContext,
};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use encodecraft_job::{ScaleMode, ScreenSize};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("{0}")]
    Message(String),
    #[error("unknown preset `{0}`")]
    Unknown(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Container + codec family. Drives FFmpeg output arguments and the default extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    H264Mp4,
    HevcMp4,
    ProResMov,
    WebmVp9,
    Gif,
    PngSequence,
    JpegSequence,
    Aac,
    Wav,
    Mp3,
}

impl OutputFormat {
    pub fn all() -> &'static [OutputFormat] {
        &[
            OutputFormat::H264Mp4,
            OutputFormat::HevcMp4,
            OutputFormat::ProResMov,
            OutputFormat::WebmVp9,
            OutputFormat::Gif,
            OutputFormat::PngSequence,
            OutputFormat::JpegSequence,
            OutputFormat::Aac,
            OutputFormat::Wav,
            OutputFormat::Mp3,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            OutputFormat::H264Mp4 => "H.264 MP4",
            OutputFormat::HevcMp4 => "H.265 / HEVC MP4",
            OutputFormat::ProResMov => "ProRes MOV",
            OutputFormat::WebmVp9 => "WebM (VP9)",
            OutputFormat::Gif => "Animated GIF",
            OutputFormat::PngSequence => "PNG sequence",
            OutputFormat::JpegSequence => "JPEG sequence",
            OutputFormat::Aac => "AAC audio",
            OutputFormat::Wav => "WAV audio",
            OutputFormat::Mp3 => "MP3 audio",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::H264Mp4 | OutputFormat::HevcMp4 => "mp4",
            OutputFormat::ProResMov => "mov",
            OutputFormat::WebmVp9 => "webm",
            OutputFormat::Gif => "gif",
            OutputFormat::PngSequence => "png",
            OutputFormat::JpegSequence => "jpg",
            OutputFormat::Aac => "m4a",
            OutputFormat::Wav => "wav",
            OutputFormat::Mp3 => "mp3",
        }
    }

    pub fn is_audio_only(self) -> bool {
        matches!(self, OutputFormat::Aac | OutputFormat::Wav | OutputFormat::Mp3)
    }

    pub fn is_sequence(self) -> bool {
        matches!(self, OutputFormat::PngSequence | OutputFormat::JpegSequence)
    }

    pub fn has_video(self) -> bool {
        !self.is_audio_only()
    }

    pub fn ffmpeg_video_codec(self) -> Option<&'static str> {
        match self {
            OutputFormat::H264Mp4 => Some("libx264"),
            OutputFormat::HevcMp4 => Some("libx265"),
            OutputFormat::ProResMov => Some("prores_ks"),
            OutputFormat::WebmVp9 => Some("libvpx-vp9"),
            OutputFormat::Gif => Some("gif"),
            OutputFormat::PngSequence => Some("png"),
            OutputFormat::JpegSequence => Some("mjpeg"),
            OutputFormat::Aac | OutputFormat::Wav | OutputFormat::Mp3 => None,
        }
    }

    pub fn ffmpeg_audio_codec(self) -> Option<&'static str> {
        match self {
            OutputFormat::H264Mp4 | OutputFormat::HevcMp4 | OutputFormat::Aac => Some("aac"),
            OutputFormat::ProResMov | OutputFormat::Wav => Some("pcm_s16le"),
            OutputFormat::WebmVp9 => Some("libopus"),
            OutputFormat::Mp3 => Some("libmp3lame"),
            OutputFormat::Gif | OutputFormat::PngSequence | OutputFormat::JpegSequence => None,
        }
    }

    pub fn parse_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "h264" | "h.264" | "mp4" | "h264-mp4" | "system.h264-mp4" => Some(OutputFormat::H264Mp4),
            "h265" | "hevc" | "h.265" | "hevc-mp4" | "system.hevc-mp4" => Some(OutputFormat::HevcMp4),
            "prores" | "mov" | "prores-mov" | "system.prores-mov" => Some(OutputFormat::ProResMov),
            "webm" | "vp9" | "webm-vp9" | "system.webm-vp9" => Some(OutputFormat::WebmVp9),
            "gif" | "system.gif" => Some(OutputFormat::Gif),
            "png" | "png-sequence" | "system.png-sequence" => Some(OutputFormat::PngSequence),
            "jpeg" | "jpg" | "jpeg-sequence" | "system.jpeg-sequence" => Some(OutputFormat::JpegSequence),
            "aac" | "m4a" | "system.aac" => Some(OutputFormat::Aac),
            "wav" | "system.wav" => Some(OutputFormat::Wav),
            "mp3" | "system.mp3" => Some(OutputFormat::Mp3),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameRate {
    Source,
    Custom(f64),
}

impl Default for FrameRate {
    fn default() -> Self {
        FrameRate::Source
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Quality {
    /// Constant rate factor / quality. Lower is better for x264/x265/VP9.
    Crf { value: u8 },
    /// Target video bitrate in kbps.
    Bitrate { kbps: u32 },
    /// Encoder-specific quality 1–100 (used for JPEG / ProRes-ish).
    Quality { value: u8 },
}

impl Quality {
    pub fn crf(value: u8) -> Self {
        Quality::Crf { value }
    }
    pub fn bitrate(kbps: u32) -> Self {
        Quality::Bitrate { kbps }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoSettings {
    /// `None` means match source (unless a screen size overrides it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default)]
    pub frame_rate: FrameRate,
    #[serde(default = "default_quality")]
    pub quality: Quality,
    #[serde(default)]
    pub scale: ScaleMode,
    /// ProRes profile: 0 Proxy … 5 4444 XQ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prores_profile: Option<u8>,
}

fn default_quality() -> Quality {
    Quality::crf(23)
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self { width: None, height: None, frame_rate: FrameRate::Source, quality: Quality::crf(23), scale: ScaleMode::Fit, prores_profile: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioSettings {
    pub enabled: bool,
    #[serde(default = "default_audio_kbps")]
    pub bitrate_kbps: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
}

fn default_audio_kbps() -> u32 {
    192
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self { enabled: true, bitrate_kbps: 192, sample_rate: None, channels: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresetKind {
    System,
    User,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub id: String,
    pub name: String,
    pub kind: PresetKind,
    pub format: OutputFormat,
    #[serde(default)]
    pub video: VideoSettings,
    #[serde(default)]
    pub audio: AudioSettings,
}

impl ExportPreset {
    pub fn extension(&self) -> &'static str {
        self.format.extension()
    }

    pub fn with_size(&self, width: u32, height: u32) -> Self {
        let mut p = self.clone();
        p.video.width = Some(width);
        p.video.height = Some(height);
        p
    }

    /// Overlay JSON fields (from an enqueue `preset_override`) onto a clone.
    pub fn merge_override(&self, overlay: &serde_json::Value) -> Result<Self, PresetError> {
        let mut v = serde_json::to_value(self)?;
        merge_json(&mut v, overlay);
        Ok(serde_json::from_value(v)?)
    }
}

fn merge_json(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                merge_json(a.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

/// In-memory preset library: builtins (immutable) + user presets.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PresetLibrary {
    pub user: Vec<ExportPreset>,
    pub screens: Vec<ScreenSize>,
}

impl PresetLibrary {
    pub fn new() -> Self {
        Self { user: Vec::new(), screens: builtin_screens() }
    }

    pub fn builtins() -> Vec<ExportPreset> {
        builtin_presets()
    }

    pub fn all(&self) -> Vec<ExportPreset> {
        let mut v = builtin_presets();
        v.extend(self.user.iter().cloned());
        v
    }

    pub fn get(&self, id: &str) -> Result<ExportPreset, PresetError> {
        self.all().into_iter().find(|p| p.id == id).ok_or_else(|| PresetError::Unknown(id.into()))
    }

    pub fn get_or_parse(&self, id_or_name: &str) -> Result<ExportPreset, PresetError> {
        if let Ok(p) = self.get(id_or_name) {
            return Ok(p);
        }
        if let Some(fmt) = OutputFormat::parse_name(id_or_name) {
            if let Some(p) = builtin_presets().into_iter().find(|p| p.format == fmt) {
                return Ok(p);
            }
        }
        self.all()
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case(id_or_name) || p.id.rsplit('.').next().is_some_and(|s| s.eq_ignore_ascii_case(id_or_name)))
            .ok_or_else(|| PresetError::Unknown(id_or_name.into()))
    }

    pub fn upsert_user(&mut self, mut preset: ExportPreset) -> Result<(), PresetError> {
        if preset.id.trim().is_empty() {
            return Err(PresetError::Message("preset id is empty".into()));
        }
        if preset.id.starts_with("system.") {
            return Err(PresetError::Message("cannot overwrite a system preset; duplicate it first".into()));
        }
        preset.kind = PresetKind::User;
        if let Some(existing) = self.user.iter_mut().find(|p| p.id == preset.id) {
            *existing = preset;
        } else {
            self.user.push(preset);
        }
        Ok(())
    }

    pub fn remove_user(&mut self, id: &str) -> Result<(), PresetError> {
        let before = self.user.len();
        self.user.retain(|p| p.id != id);
        if self.user.len() == before {
            return Err(PresetError::Unknown(id.into()));
        }
        Ok(())
    }

    pub fn duplicate(&mut self, id: &str, new_name: &str) -> Result<ExportPreset, PresetError> {
        let mut p = self.get(id)?;
        p.kind = PresetKind::User;
        p.name = new_name.to_string();
        p.id = unique_user_id(&self.user, new_name);
        self.user.push(p.clone());
        Ok(p)
    }

    pub fn export_preset(&self, id: &str, path: &Path) -> Result<(), PresetError> {
        let p = self.get(id)?;
        std::fs::write(path, serde_json::to_vec_pretty(&p)?)?;
        Ok(())
    }

    pub fn import_preset(&mut self, path: &Path) -> Result<ExportPreset, PresetError> {
        let p: ExportPreset = serde_json::from_slice(&std::fs::read(path)?)?;
        self.upsert_user(p.clone())?;
        Ok(p)
    }

    pub fn export_all_user(&self, path: &Path) -> Result<(), PresetError> {
        std::fs::write(path, serde_json::to_vec_pretty(&self.user)?)?;
        Ok(())
    }

    pub fn import_many(&mut self, path: &Path) -> Result<usize, PresetError> {
        let v: Vec<ExportPreset> = serde_json::from_slice(&std::fs::read(path)?)?;
        let n = v.len();
        for p in v {
            self.upsert_user(p)?;
        }
        Ok(n)
    }

    pub fn upsert_screen(&mut self, size: ScreenSize) {
        if let Some(existing) = self.screens.iter_mut().find(|s| s.name == size.name) {
            *existing = size;
        } else {
            self.screens.push(size);
        }
    }

    pub fn remove_screen(&mut self, name: &str) -> bool {
        let before = self.screens.len();
        self.screens.retain(|s| s.name != name);
        self.screens.len() != before
    }

    pub fn find_screen(&self, name_or_dims: &str) -> Option<ScreenSize> {
        if let Some(s) = self.screens.iter().find(|s| s.name.eq_ignore_ascii_case(name_or_dims)) {
            return Some(s.clone());
        }
        if let Some((w, h)) = ScreenSize::parse_dims(name_or_dims) {
            return self.screens.iter().cloned().find(|s| s.width == w && s.height == h).or_else(|| Some(ScreenSize::new(format!("{w}x{h}"), w, h)));
        }
        None
    }

    pub fn save(&self, path: &Path) -> Result<(), PresetError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        atomic_write(path, &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, PresetError> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let bytes = std::fs::read(path)?;
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(mut lib) => {
                if lib.screens.is_empty() {
                    lib.screens = builtin_screens();
                }
                Ok(lib)
            }
            Err(_) => {
                let bad = path.with_extension("json.corrupt");
                let _ = std::fs::rename(path, bad);
                Ok(Self::new())
            }
        }
    }
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = std::fs::remove_file(path);
            std::fs::rename(&tmp, path)
        }
    }
}

fn unique_user_id(user: &[ExportPreset], name: &str) -> String {
    let slug = slugify(name);
    let mut id = format!("user.{slug}");
    let mut n = 2u32;
    while user.iter().any(|p| p.id == id) {
        id = format!("user.{slug}-{n}");
        n = n.saturating_add(1);
    }
    id
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() {
        "preset".into()
    } else {
        t
    }
}

pub fn builtin_presets() -> Vec<ExportPreset> {
    vec![
        preset("system.h264-mp4", "H.264 Match Source", OutputFormat::H264Mp4, Quality::crf(23), true),
        preset("system.hevc-mp4", "HEVC Match Source", OutputFormat::HevcMp4, Quality::crf(28), true),
        {
            let mut p = preset("system.prores-mov", "Apple ProRes 422", OutputFormat::ProResMov, Quality::Quality { value: 80 }, true);
            p.video.prores_profile = Some(2);
            p
        },
        preset("system.webm-vp9", "WebM VP9", OutputFormat::WebmVp9, Quality::crf(32), true),
        {
            let mut p = preset("system.gif", "Animated GIF", OutputFormat::Gif, Quality::crf(0), false);
            p.video.frame_rate = FrameRate::Custom(15.0);
            p.video.width = Some(480);
            p
        },
        {
            let mut p = preset("system.png-sequence", "PNG Sequence", OutputFormat::PngSequence, Quality::Quality { value: 100 }, false);
            p.audio.enabled = false;
            p
        },
        {
            let mut p = preset("system.jpeg-sequence", "JPEG Sequence", OutputFormat::JpegSequence, Quality::Quality { value: 85 }, false);
            p.audio.enabled = false;
            p
        },
        {
            let mut p = preset("system.aac", "AAC Audio", OutputFormat::Aac, Quality::bitrate(192), true);
            p.audio.bitrate_kbps = 192;
            p
        },
        {
            let p = preset("system.wav", "WAV Audio", OutputFormat::Wav, Quality::bitrate(0), true);
            p
        },
        {
            let mut p = preset("system.mp3", "MP3 Audio", OutputFormat::Mp3, Quality::bitrate(192), true);
            p.audio.bitrate_kbps = 192;
            p
        },
    ]
}

fn preset(id: &str, name: &str, format: OutputFormat, quality: Quality, audio: bool) -> ExportPreset {
    ExportPreset {
        id: id.into(),
        name: name.into(),
        kind: PresetKind::System,
        format,
        video: VideoSettings { quality, ..VideoSettings::default() },
        audio: AudioSettings { enabled: audio && format.ffmpeg_audio_codec().is_some(), ..AudioSettings::default() },
    }
}

/// Built-in digital-out-of-home (and common web/social) screen sizes.
pub fn builtin_screens() -> Vec<ScreenSize> {
    vec![
        ScreenSize::new("FHD Landscape", 1920, 1080),
        ScreenSize::new("FHD Portrait", 1080, 1920),
        ScreenSize::new("DOOH 1200×960", 1200, 960),
        ScreenSize::new("DOOH 1080×864", 1080, 864),
        ScreenSize::new("UHD 4K", 3840, 2160),
        ScreenSize::new("QHD Landscape", 2560, 1440),
        ScreenSize::new("HD Landscape", 1280, 720),
        ScreenSize::new("HD Portrait", 720, 1280),
        ScreenSize::new("Square 1080", 1080, 1080),
        ScreenSize::new("Portrait 9:16 1440", 1440, 2560),
        ScreenSize::new("Wide 1920×540", 1920, 540),
        ScreenSize::new("Poster 1080×1350", 1080, 1350),
    ]
}

pub fn parse_size_list(s: &str) -> Result<Vec<ScreenSize>, PresetError> {
    let mut out = Vec::new();
    for part in s.split(|c| c == ',' || c == ';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (w, h) = ScreenSize::parse_dims(part).ok_or_else(|| PresetError::Message(format!("invalid size `{part}`")))?;
        out.push(ScreenSize::new(format!("{w}x{h}"), w, h));
    }
    if out.is_empty() {
        return Err(PresetError::Message("no sizes given".into()));
    }
    Ok(out)
}

/// Tokens available to naming templates.
pub fn naming_tokens() -> BTreeMap<&'static str, &'static str> {
    let mut m = BTreeMap::new();
    m.insert("name", "source file stem or composition name");
    m.insert("width", "output width in pixels");
    m.insert("height", "output height in pixels");
    m.insert("preset", "preset name slug");
    m.insert("codec", "container / codec family");
    m.insert("ext", "file extension without a dot");
    m.insert("date", "local date YYYY-MM-DD");
    m.insert("time", "local time HHMMSS");
    m.insert("job", "queue job id");
    m.insert("index", "1-based job index in a batch");
    m.insert("screen", "screen-size name slug");
    m
}

pub fn default_output_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("ENCODECRAFT_OUTPUT") {
        return PathBuf::from(p);
    }
    directories::UserDirs::new()
        .and_then(|u| u.video_dir().map(|p| p.join("EncodeCraft")))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join("encodecraft-output"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_cover_requested_formats() {
        let ids: Vec<_> = builtin_presets().into_iter().map(|p| p.id).collect();
        for need in [
            "system.h264-mp4",
            "system.hevc-mp4",
            "system.prores-mov",
            "system.webm-vp9",
            "system.gif",
            "system.png-sequence",
            "system.jpeg-sequence",
            "system.aac",
            "system.wav",
            "system.mp3",
        ] {
            assert!(ids.iter().any(|id| id == need), "missing {need}");
        }
    }

    #[test]
    fn user_preset_roundtrip_file() {
        let dir = std::env::temp_dir().join(format!("encodecraft-preset-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut lib = PresetLibrary::new();
        let dup = lib.duplicate("system.h264-mp4", "DOOH H.264").unwrap();
        assert!(dup.id.starts_with("user."));
        let path = dir.join("dooh.json");
        lib.export_preset(&dup.id, &path).unwrap();
        let mut lib2 = PresetLibrary::new();
        let imported = lib2.import_preset(&path).unwrap();
        assert_eq!(imported.name, "DOOH H.264");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cannot_overwrite_system_id() {
        let mut lib = PresetLibrary::new();
        let mut p = builtin_presets().remove(0);
        p.id = "system.h264-mp4".into();
        assert!(lib.upsert_user(p).is_err());
    }

    #[test]
    fn screen_pack_includes_dooh_examples() {
        let screens = builtin_screens();
        let has = |w, h| screens.iter().any(|s| s.width == w && s.height == h);
        assert!(has(1920, 1080));
        assert!(has(1080, 1920));
        assert!(has(1200, 960));
        assert!(has(1080, 864));
    }

    #[test]
    fn parse_size_list_csv() {
        let v = parse_size_list("1920x1080, 1080x1920, 1200×960").unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[2].height, 960);
    }

    #[test]
    fn merge_override_changes_crf() {
        let p = builtin_presets().into_iter().find(|p| p.id == "system.h264-mp4").unwrap();
        let over = serde_json::json!({"video": {"quality": {"mode": "crf", "value": 18}}});
        let m = p.merge_override(&over).unwrap();
        assert_eq!(m.video.quality, Quality::crf(18));
        assert_eq!(m.format, OutputFormat::H264Mp4);
    }
}
