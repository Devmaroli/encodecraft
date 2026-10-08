//! Export presets, DOOH screen-size packs, and `{name}_{width}x{height}` naming templates.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod naming;
mod screen_rules;

pub use naming::{
    apply_template, constrain_output_dir, default_template, effective_naming, is_legacy_default_template, is_under, naming_uses_size, normalize_path,
    output_path, sanitize_filename, unique_output_path, NamingContext,
};
pub use screen_rules::{
    infer_screen_from_text, normalize_screen_name, PresetPick, PresetReason, ScreenMatchRule, ScreenPresetRules,
};

pub const BETTER_RES_ID: &str = "system.better-res";
pub const INSTALLATION_ID: &str = "system.installation";
pub const APPROVAL_RES_ID: &str = "system.approval-res";
pub const SULTAN_ID: &str = "system.sultan";

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use encodecraft_job::{JobSource, ScaleMode, ScreenSize};
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

    pub fn short_label(self) -> &'static str {
        match self {
            OutputFormat::H264Mp4 => "H.264",
            OutputFormat::HevcMp4 => "H.265",
            OutputFormat::ProResMov => "ProRes",
            OutputFormat::WebmVp9 => "WebM",
            OutputFormat::Gif => "GIF",
            OutputFormat::PngSequence => "PNG",
            OutputFormat::JpegSequence => "JPEG",
            OutputFormat::Aac => "AAC",
            OutputFormat::Wav => "WAV",
            OutputFormat::Mp3 => "MP3",
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
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum Quality {
    /// Constant rate factor / quality. Lower is better for x264/x265/VP9.
    Crf { value: u8 },
    /// Target video bitrate in kbps.
    Bitrate { kbps: u32 },
    /// Encoder-specific quality 1–100 (used for JPEG / ProRes-ish).
    Quality { value: u8 },
    /// libx264 VBR. Numbers are Mbps so they stay easy to edit.
    /// `two_pass` defaults to true so older `vbr-2pass` JSON stays 2-pass.
    #[serde(rename = "vbr-2pass", alias = "vbr2pass", alias = "twopass", alias = "vbr-2-pass", alias = "vbr-1pass")]
    Vbr2Pass {
        target_mbps: f64,
        max_mbps: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_mbps: Option<f64>,
        /// false = VBR 1-pass (Adobe Media Encoder's H.264 default when encoding mode is omitted).
        #[serde(default = "bool_true", skip_serializing_if = "is_true")]
        two_pass: bool,
    },
}

fn bool_true() -> bool {
    true
}
fn is_true(v: &bool) -> bool {
    *v
}

impl Quality {
    pub fn crf(value: u8) -> Self {
        Quality::Crf { value }
    }
    pub fn bitrate(kbps: u32) -> Self {
        Quality::Bitrate { kbps }
    }
    pub fn vbr_2pass(target_mbps: f64, max_mbps: f64) -> Self {
        Self::vbr(None, target_mbps, max_mbps, true)
    }

    pub fn vbr(min_mbps: Option<f64>, target_mbps: f64, max_mbps: f64, two_pass: bool) -> Self {
        Quality::Vbr2Pass { target_mbps, max_mbps, min_mbps, two_pass }
    }

    pub fn is_two_pass(self) -> bool {
        matches!(self, Quality::Vbr2Pass { two_pass: true, .. })
    }

    pub fn vbr_parts(self) -> Option<(Option<f64>, f64, f64, bool)> {
        match self {
            Quality::Vbr2Pass { min_mbps, target_mbps, max_mbps, two_pass } => Some((min_mbps, target_mbps, max_mbps, two_pass)),
            _ => None,
        }
    }

    /// AME leftover: min may be stored above the target; clamp so min ≤ target.
    pub fn effective_min_mbps(self) -> Option<f64> {
        let (min, target, _, _) = self.vbr_parts()?;
        Some(min.unwrap_or(0.0).min(target).max(0.0)).filter(|v| *v > 0.005)
    }

    pub fn effective_min_kbps(self) -> Option<u32> {
        self.effective_min_mbps().map(mbps_to_kbps)
    }

    pub fn bitrate_order_warning(self) -> Option<String> {
        let (min, target, max, _) = self.vbr_parts()?;
        if target > max + 0.01 {
            return Some(format!("Target {} Mbps is above the {} Mbps maximum.", compact_mbps(target), compact_mbps(max)));
        }
        if let Some(min) = min {
            if min > target + 0.01 {
                return Some(format!(
                    "Minimum {} Mbps is above the {} Mbps target — using {} Mbps (AME clamps this too).",
                    compact_mbps(min),
                    compact_mbps(target),
                    compact_mbps(target)
                ));
            }
        }
        None
    }

    pub fn target_kbps(self) -> Option<u32> {
        match self {
            Quality::Bitrate { kbps } => Some(kbps.max(1)),
            Quality::Vbr2Pass { target_mbps, .. } => Some(mbps_to_kbps(target_mbps)),
            _ => None,
        }
    }

    pub fn max_kbps(self) -> Option<u32> {
        match self {
            Quality::Vbr2Pass { max_mbps, target_mbps, .. } => {
                let t = mbps_to_kbps(target_mbps);
                Some(mbps_to_kbps(max_mbps).max(t))
            }
            Quality::Bitrate { kbps } => Some(kbps.max(1)),
            _ => None,
        }
    }
}

pub fn mbps_to_kbps(mbps: f64) -> u32 {
    let v = (mbps * 1000.0).round();
    if !v.is_finite() {
        return 1;
    }
    v.clamp(1.0, 1_000_000.0) as u32
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
    /// libx264 `-profile:v` (`high`, `main`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h264_profile: Option<String>,
    /// libx264 `-level:v`. `auto` / omitted = pick from frame size (6.2 when any side > 4096).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h264_level: Option<String>,
    /// GOP / keyframe distance in frames (`-g`). Studio AME presets use 72.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyframe_interval: Option<u32>,
    /// When the global GPU toggle is on, this H.264 preset may use NVIDIA NVENC.
    #[serde(default)]
    pub allow_gpu: bool,
    /// Runtime: this encode should use `h264_nvenc` (not persisted).
    #[serde(default, skip_serializing)]
    pub use_nvenc: bool,
}

fn default_quality() -> Quality {
    Quality::crf(23)
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            frame_rate: FrameRate::Source,
            quality: Quality::crf(23),
            scale: ScaleMode::Fit,
            prores_profile: None,
            h264_profile: None,
            h264_level: None,
            keyframe_interval: None,
            allow_gpu: false,
            use_nvenc: false,
        }
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
    /// One-off (Approval Res, Sultan, …): do not remember as the manual choice for this comp.
    #[serde(default)]
    pub one_off: bool,
    /// User-preset folder in the browser (`Ramadan campaign`). Empty = ungrouped.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
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

    pub fn bitrate_short(&self) -> String {
        match self.video.quality {
            Quality::Vbr2Pass { target_mbps, max_mbps, .. } => {
                if (max_mbps - target_mbps).abs() < 0.05 {
                    format!("{} Mbps", compact_mbps(target_mbps))
                } else {
                    format!("{}–{} Mbps", compact_mbps(target_mbps), compact_mbps(max_mbps))
                }
            }
            Quality::Bitrate { kbps } if kbps >= 1000 => format!("{:.0} Mbps", kbps as f64 / 1000.0),
            Quality::Bitrate { kbps } => format!("{kbps} kbps"),
            Quality::Crf { value } => format!("CRF {value}"),
            Quality::Quality { value } => format!("Q{value}"),
        }
    }

    /// Rough file-size estimate from target bitrate (video + optional audio).
    pub fn estimate_file_mb(&self, duration_sec: f64) -> f64 {
        let v = self.video.quality.target_kbps().unwrap_or(0) as f64;
        let a = if self.audio.enabled { f64::from(self.audio.bitrate_kbps) } else { 0.0 };
        ((v + a) / 8.0) * duration_sec / 1000.0
    }

    /// Overlay JSON fields (from an enqueue `preset_override`) onto a clone.
    pub fn merge_override(&self, overlay: &serde_json::Value) -> Result<Self, PresetError> {
        let mut v = serde_json::to_value(self)?;
        merge_json(&mut v, overlay);
        Ok(serde_json::from_value(v)?)
    }
}

fn compact_mbps(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{:.0}", v)
    } else {
        format!("{:.1}", v)
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PresetLibrary {
    pub user: Vec<ExportPreset>,
    pub screens: Vec<ScreenSize>,
    /// Saved edits to system studio presets (bitrate numbers, fps, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_overrides: Vec<ExportPreset>,
    /// Automatic Better Res / Installation assignment. Editable in Settings.
    #[serde(default)]
    pub screen_rules: ScreenPresetRules,
    /// Manual preset choices that stick on the next EffectCraft send / re-queue of the same comp.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sticky_manuals: Vec<StickyManual>,
}

/// Remembered per-comp manual preset (`project + comp + screen`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickyManual {
    pub project: String,
    pub comp: String,
    #[serde(default)]
    pub screen: String,
    pub preset_id: String,
}

impl Default for PresetLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl PresetLibrary {
    pub fn new() -> Self {
        Self {
            user: Vec::new(),
            screens: builtin_screens(),
            system_overrides: Vec::new(),
            screen_rules: ScreenPresetRules::default(),
            sticky_manuals: Vec::new(),
        }
    }

    pub fn builtins() -> Vec<ExportPreset> {
        builtin_presets()
    }

    fn resolved_builtins(&self) -> Vec<ExportPreset> {
        let mut v = builtin_presets();
        for over in &self.system_overrides {
            if let Some(p) = v.iter_mut().find(|p| p.id == over.id) {
                let mut o = over.clone();
                o.kind = PresetKind::System;
                o.id = p.id.clone();
                *p = o;
            }
        }
        v
    }

    pub fn all(&self) -> Vec<ExportPreset> {
        let mut v = self.resolved_builtins();
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
        if Self::name_suggests_one_off(&preset.name) && !self.user.iter().any(|p| p.id == preset.id) {
            preset.one_off = true;
        }
        if let Some(existing) = self.user.iter_mut().find(|p| p.id == preset.id) {
            *existing = preset;
        } else {
            self.user.push(preset);
        }
        Ok(())
    }

    /// Save a user preset, or persist bitrate/fps edits on the studio system presets.
    pub fn save_preset(&mut self, mut preset: ExportPreset) -> Result<(), PresetError> {
        if Self::is_studio_system(&preset.id) {
            preset.kind = PresetKind::System;
            if let Some(existing) = self.system_overrides.iter_mut().find(|p| p.id == preset.id) {
                *existing = preset;
            } else {
                self.system_overrides.push(preset);
            }
            return Ok(());
        }
        if Self::name_suggests_one_off(&preset.name) && !self.user.iter().any(|p| p.id == preset.id) {
            preset.one_off = true;
        }
        self.upsert_user(preset)
    }

    pub fn is_studio_system(id: &str) -> bool {
        id == BETTER_RES_ID || id == INSTALLATION_ID || id == APPROVAL_RES_ID || id == SULTAN_ID
    }

    pub fn studio_presets(&self) -> Vec<ExportPreset> {
        self.all().into_iter().filter(|p| Self::is_studio_system(&p.id)).collect()
    }

    pub fn reset_studio_to_builtin(&mut self, id: &str) -> Result<ExportPreset, PresetError> {
        if !Self::is_studio_system(id) {
            return Err(PresetError::Message("not a studio preset".into()));
        }
        self.system_overrides.retain(|p| p.id != id);
        self.get(id)
    }

    pub fn builtin_copy(id: &str) -> Result<ExportPreset, PresetError> {
        builtin_presets().into_iter().find(|p| p.id == id).ok_or_else(|| PresetError::Unknown(id.into()))
    }

    pub fn name_suggests_one_off(name: &str) -> bool {
        name.to_ascii_lowercase().contains("sultan")
    }

    pub fn is_one_off(&self, preset: &ExportPreset) -> bool {
        preset.one_off || Self::name_suggests_one_off(&preset.name)
    }

    pub fn one_off_presets(&self) -> Vec<ExportPreset> {
        self.all().into_iter().filter(|p| self.is_one_off(p)).collect()
    }

    pub fn sticky_parts(source: &JobSource, screen: Option<&ScreenSize>) -> (String, String, String) {
        let (project, comp) = match source {
            JobSource::File { path, .. } => (path.to_string_lossy().into_owned(), String::new()),
            JobSource::EffectCraft { project, comp, .. } => (project.to_string_lossy().into_owned(), comp.clone().unwrap_or_default()),
        };
        let screen = screen
            .map(|s| {
                if s.name.trim().is_empty() {
                    s.label()
                } else {
                    s.name.clone()
                }
            })
            .unwrap_or_default();
        (project, comp, screen)
    }

    fn sticky_matches(entry: &StickyManual, project: &str, comp: &str, screen: &str) -> bool {
        entry.project.eq_ignore_ascii_case(project) && entry.comp.eq_ignore_ascii_case(comp) && entry.screen.eq_ignore_ascii_case(screen)
    }

    pub fn get_sticky(&self, source: &JobSource, screen: Option<&ScreenSize>) -> Option<String> {
        let (project, comp, screen) = Self::sticky_parts(source, screen);
        self.sticky_manuals
            .iter()
            .find(|s| Self::sticky_matches(s, &project, &comp, &screen))
            .map(|s| s.preset_id.clone())
    }

    pub fn set_sticky(&mut self, source: &JobSource, screen: Option<&ScreenSize>, preset_id: String) {
        let (project, comp, screen) = Self::sticky_parts(source, screen);
        if let Some(existing) = self.sticky_manuals.iter_mut().find(|s| Self::sticky_matches(s, &project, &comp, &screen)) {
            existing.preset_id = preset_id;
        } else {
            self.sticky_manuals.push(StickyManual { project, comp, screen, preset_id });
        }
    }

    pub fn clear_sticky(&mut self, source: &JobSource, screen: Option<&ScreenSize>) {
        let (project, comp, screen) = Self::sticky_parts(source, screen);
        self.sticky_manuals.retain(|s| !Self::sticky_matches(s, &project, &comp, &screen));
    }

    /// Persist a manual pick unless the preset is one-off (Approval Res, Sultan, …).
    pub fn remember_manual_choice(&mut self, source: &JobSource, screen: Option<&ScreenSize>, preset: &ExportPreset) {
        if self.is_one_off(preset) {
            self.clear_sticky(source, screen);
            return;
        }
        self.set_sticky(source, screen, preset.id.clone());
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
        if Self::name_suggests_one_off(new_name) {
            p.one_off = true;
        }
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
                if lib.screen_rules.listed.is_empty() && lib.screen_rules.ultra_wide_names.is_empty() {
                    lib.screen_rules = ScreenPresetRules::default();
                }
                if lib.screen_rules.migrate_listed_screens() {
                    let _ = lib.save(path);
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
        studio_h264(BETTER_RES_ID, "Better Res", 2.0, 6.2, 6.2, true, false, false),
        studio_h264(INSTALLATION_ID, "Installation", 2.0, 4.0, 6.0, true, false, false),
        approval_res(),
        sultan(),
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
        one_off: false,
        group: String::new(),
    }
}

fn studio_h264(
    id: &str,
    name: &str,
    min_mbps: f64,
    target_mbps: f64,
    max_mbps: f64,
    two_pass: bool,
    audio: bool,
    one_off: bool,
) -> ExportPreset {
    ExportPreset {
        id: id.into(),
        name: name.into(),
        kind: PresetKind::System,
        format: OutputFormat::H264Mp4,
        video: VideoSettings {
            frame_rate: FrameRate::Custom(25.0),
            quality: Quality::vbr(Some(min_mbps), target_mbps, max_mbps, two_pass),
            h264_profile: Some("high".into()),
            h264_level: Some("auto".into()),
            keyframe_interval: Some(72),
            allow_gpu: true,
            ..VideoSettings::default()
        },
        audio: AudioSettings { enabled: audio, ..AudioSettings::default() },
        one_off,
        group: "Studio".into(),
    }
}

fn approval_res() -> ExportPreset {
    // AME stores min 2 Mbps above the 1 Mbps target; we keep that number and clamp at encode/UI.
    let mut p = studio_h264(APPROVAL_RES_ID, "Approval Res", 2.0, 1.0, 6.0, true, true, true);
    p.audio.bitrate_kbps = 320;
    p.audio.sample_rate = Some(48_000);
    p.audio.channels = Some(2);
    p
}

fn sultan() -> ExportPreset {
    // ADBEVideoBitrateEncoding has no ParamValue in the .epr — AME H.264 default is VBR 1-pass.
    studio_h264(SULTAN_ID, "Sultan", 2.0, 3.0, 3.0, false, false, true)
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
    m.insert("comp", "composition name, or the dropped file's base name");
    m.insert("name", "same as {comp} (kept for older templates)");
    m.insert("project", "EffectCraft project file stem");
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
            "system.better-res",
            "system.installation",
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
    fn load_migrates_old_presets_json_listed_screens() {
        let dir = std::env::temp_dir().join(format!("encodecraft-rules-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("presets.json");
        let old = serde_json::json!({
            "user": [],
            "screens": [],
            "screen_rules": {
                "enabled": true,
                "better_res_id": "system.better-res",
                "installation_id": "system.installation",
                "ultra_wide_aspect": 5.0,
                "ultra_wide_names": ["The Avenues Gate"],
                "listed": [
                    {"name": "1st Ring Road", "width": 1008, "height": 5184, "match_dims": true},
                    {"name": "Al Nassar Tower Vertical", "width": 960, "height": 5328, "match_dims": true},
                    {"name": "The Avenues Grand Avenues Entrance", "width": 1920, "height": 1080, "match_dims": false},
                    {"name": "The Avenues The Mall Entrance", "width": 1920, "height": 1080, "match_dims": false},
                    {"name": "The Avenues Grand Plaza Entrance", "width": 1920, "height": 1080, "match_dims": false}
                ]
            }
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&old).unwrap()).unwrap();
        let lib = PresetLibrary::load(&path).unwrap();
        assert!(lib.screen_rules.is_listed("Eye of Kuwait", Some(7560), Some(2100)));
        assert!(lib.screen_rules.is_listed("Al Salam Sync", Some(3072), Some(576)));
        assert!(lib.screen_rules.is_listed("Piccadilly", Some(2027), Some(720)));
        assert_eq!(lib.screen_rules.rules_version, crate::screen_rules::SCREEN_RULES_VERSION);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("Eye of Kuwait") || saved.contains("eye of kuwait"));
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn studio_presets_are_high_25fps_vbr() {
        let lib = PresetLibrary::new();
        let br = lib.get(BETTER_RES_ID).unwrap();
        assert_eq!(br.name, "Better Res");
        assert_eq!(br.video.frame_rate, FrameRate::Custom(25.0));
        assert_eq!(br.video.h264_profile.as_deref(), Some("high"));
        assert_eq!(br.video.keyframe_interval, Some(72));
        assert_eq!(br.video.quality, Quality::vbr(Some(2.0), 6.2, 6.2, true));
        assert!(!br.audio.enabled);
        let inst = lib.get(INSTALLATION_ID).unwrap();
        assert_eq!(inst.name, "Installation");
        assert_eq!(inst.video.quality, Quality::vbr(Some(2.0), 4.0, 6.0, true));
        assert!(!inst.audio.enabled);
    }

    #[test]
    fn studio_bitrate_edits_persist_on_system_id() {
        let mut lib = PresetLibrary::new();
        let mut p = lib.get(BETTER_RES_ID).unwrap();
        p.video.quality = Quality::vbr_2pass(36.0, 44.0);
        lib.save_preset(p).unwrap();
        let loaded = lib.get(BETTER_RES_ID).unwrap();
        assert_eq!(loaded.video.quality, Quality::vbr_2pass(36.0, 44.0));
        assert_eq!(loaded.kind, PresetKind::System);
    }

    #[test]
    fn get_or_parse_studio_names() {
        let lib = PresetLibrary::new();
        assert_eq!(lib.get_or_parse("Better Res").unwrap().id, BETTER_RES_ID);
        assert_eq!(lib.get_or_parse("installation").unwrap().id, INSTALLATION_ID);
        assert_eq!(lib.get_or_parse("Approval Res").unwrap().id, APPROVAL_RES_ID);
        assert_eq!(lib.get_or_parse("Sultan").unwrap().id, SULTAN_ID);
    }

    #[test]
    fn approval_res_is_one_off_1mbps_with_audio() {
        let lib = PresetLibrary::new();
        let p = lib.get(APPROVAL_RES_ID).unwrap();
        assert!(p.one_off);
        assert_eq!(p.video.frame_rate, FrameRate::Custom(25.0));
        assert_eq!(p.video.quality, Quality::vbr(Some(2.0), 1.0, 6.0, true));
        assert_eq!(p.video.quality.effective_min_mbps(), Some(1.0));
        assert!(p.video.quality.bitrate_order_warning().unwrap().contains("AME clamps"));
        assert!(p.audio.enabled);
        assert_eq!(p.audio.bitrate_kbps, 320);
        assert_eq!(p.audio.sample_rate, Some(48_000));
        assert_eq!(p.audio.channels, Some(2));
        assert!(p.video.width.is_none());
        assert!(p.video.height.is_none());
        assert!(lib.one_off_presets().iter().any(|x| x.id == APPROVAL_RES_ID));
    }

    #[test]
    fn sultan_studio_is_one_off_vbr_1pass_without_audio() {
        let lib = PresetLibrary::new();
        let p = lib.get(SULTAN_ID).unwrap();
        assert_eq!(p.name, "Sultan");
        assert!(p.one_off);
        assert_eq!(p.video.frame_rate, FrameRate::Custom(25.0));
        assert_eq!(p.video.quality, Quality::vbr(Some(2.0), 3.0, 3.0, false));
        assert!(!p.video.quality.is_two_pass());
        assert!(!p.audio.enabled);
        assert!(p.video.width.is_none());
        assert!(p.video.height.is_none());
        assert!(lib.one_off_presets().iter().any(|x| x.id == SULTAN_ID));
    }

    #[test]
    fn sultan_named_user_preset_defaults_to_one_off() {
        let mut lib = PresetLibrary::new();
        let mut p = lib.get("system.h264-mp4").unwrap();
        p.id = "user.sultan-screen".into();
        p.name = "Sultan Screen".into();
        p.one_off = false;
        lib.upsert_user(p).unwrap();
        let saved = lib.get("user.sultan-screen").unwrap();
        assert!(saved.one_off);
    }

    #[test]
    fn sticky_manual_better_res_persists_approval_res_does_not() {
        let mut lib = PresetLibrary::new();
        let source = JobSource::EffectCraft {
            project: PathBuf::from("C:/work/spot.ecproj"),
            comp: Some("Main".into()),
            mezzanine: "prores".into(),
            work_area: false,
            width: None,
            height: None,
        };
        let screen = ScreenSize::new("Honor 400", 1080, 1080);
        let better = lib.get(BETTER_RES_ID).unwrap();
        lib.remember_manual_choice(&source, Some(&screen), &better);
        assert_eq!(lib.get_sticky(&source, Some(&screen)).as_deref(), Some(BETTER_RES_ID));

        let approval = lib.get(APPROVAL_RES_ID).unwrap();
        lib.remember_manual_choice(&source, Some(&screen), &approval);
        assert_eq!(lib.get_sticky(&source, Some(&screen)), None);

        lib.remember_manual_choice(&source, Some(&screen), &better);
        assert_eq!(lib.get_sticky(&source, Some(&screen)).as_deref(), Some(BETTER_RES_ID));

        let sultan = lib.get(SULTAN_ID).unwrap();
        lib.remember_manual_choice(&source, Some(&screen), &sultan);
        assert_eq!(lib.get_sticky(&source, Some(&screen)), None);

        lib.remember_manual_choice(&source, Some(&screen), &better);
        let mut named = lib.get("system.h264-mp4").unwrap();
        named.id = "user.sultan-screen".into();
        named.name = "Sultan Screen".into();
        named.one_off = true;
        lib.remember_manual_choice(&source, Some(&screen), &named);
        assert_eq!(lib.get_sticky(&source, Some(&screen)), None);
    }
}
