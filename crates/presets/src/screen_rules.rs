//! Automatic Better Res / Installation assignment from screen name and size.
//!
//! Rules live on [`PresetLibrary`] (`presets.json`) so the user can add screens later.

use encodecraft_job::ScreenSize;
use serde::{Deserialize, Serialize};

use crate::{BETTER_RES_ID, INSTALLATION_ID};

/// Why a queue item received Better Res or Installation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetReason {
    UltraWide,
    ListedScreen,
    OtherScreen,
    /// EffectCraft / `preset_id: auto` with no named screen — Installation.
    StudioDefault,
    /// File with no screen name — the configured default preset.
    Default,
    #[default]
    Manual,
}

impl PresetReason {
    pub fn label(self) -> &'static str {
        match self {
            PresetReason::UltraWide => "ultra wide",
            PresetReason::ListedScreen => "listed screen",
            PresetReason::OtherScreen => "other screen",
            PresetReason::StudioDefault => "installation default",
            PresetReason::Default => "default",
            PresetReason::Manual => "override",
        }
    }

    /// Queue tag copy from the approved mockup.
    pub fn tag_label(self) -> &'static str {
        match self {
            PresetReason::UltraWide => "Auto · ultra wide",
            PresetReason::ListedScreen => "Auto · listed screen",
            PresetReason::OtherScreen | PresetReason::StudioDefault => "Auto · other screen",
            PresetReason::Default => "Default",
            PresetReason::Manual => "Manual",
        }
    }

    pub fn is_auto(self) -> bool {
        matches!(self, PresetReason::UltraWide | PresetReason::ListedScreen | PresetReason::OtherScreen | PresetReason::StudioDefault)
    }
}

/// Result of automatic studio-preset selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetPick {
    pub preset_id: String,
    pub reason: PresetReason,
}

impl PresetPick {
    pub fn better_res(reason: PresetReason) -> Self {
        Self { preset_id: BETTER_RES_ID.into(), reason }
    }
    pub fn installation(reason: PresetReason) -> Self {
        Self { preset_id: INSTALLATION_ID.into(), reason }
    }
}

/// One named screen that should always get Better Res.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenMatchRule {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// When true, matching width×height alone (no name) counts. Off for generic 1920×1080.
    #[serde(default)]
    pub match_dims: bool,
}

impl ScreenMatchRule {
    pub fn new(name: impl Into<String>, aliases: &[&str], width: u32, height: u32, match_dims: bool) -> Self {
        Self {
            name: name.into(),
            aliases: aliases.iter().map(|s| (*s).to_string()).collect(),
            width: Some(width),
            height: Some(height),
            match_dims,
        }
    }
}

/// Editable Better Res / Installation assignment rules (stored in presets JSON).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenPresetRules {
    /// When false, enqueue keeps the requested preset_id.
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_better_res")]
    pub better_res_id: String,
    #[serde(default = "default_installation")]
    pub installation_id: String,
    /// Landscape `width / height` greater than this counts as ultra wide. Default 5.
    #[serde(default = "default_ultra_wide_aspect")]
    pub ultra_wide_aspect: f64,
    /// Names that always count as ultra wide (e.g. The Avenues Gate).
    #[serde(default = "default_ultra_wide_names")]
    pub ultra_wide_names: Vec<String>,
    /// Named screens that always get Better Res (Ring Road, Al Nassar, Avenues Frame, …).
    #[serde(default = "default_listed_screens")]
    pub listed: Vec<ScreenMatchRule>,
}

fn default_true() -> bool {
    true
}
fn default_better_res() -> String {
    BETTER_RES_ID.into()
}
fn default_installation() -> String {
    INSTALLATION_ID.into()
}
fn default_ultra_wide_aspect() -> f64 {
    5.0
}
fn default_ultra_wide_names() -> Vec<String> {
    vec!["The Avenues Gate".into()]
}
fn default_listed_screens() -> Vec<ScreenMatchRule> {
    vec![
        ScreenMatchRule::new("1st Ring Road", &["1st ring", "first ring road"], 1008, 5184, true),
        ScreenMatchRule::new("Al Nassar Tower Vertical", &["al nassar", "nassar tower vertical", "nassar tower"], 960, 5328, true),
        ScreenMatchRule::new(
            "The Avenues Grand Avenues Entrance",
            &["grand avenues", "avenues entrance"],
            1920,
            1080,
            false,
        ),
        ScreenMatchRule::new("The Avenues The Mall Entrance", &["the mall entrance"], 1920, 1080, false),
        ScreenMatchRule::new("The Avenues Grand Plaza Entrance", &["grand plaza"], 1920, 1080, false),
        ScreenMatchRule::new(
            "The Avenues – Eye of Kuwait",
            &["eye of kuwait", "avenues eye of kuwait", "the avenues – eye of kuwait"],
            7560,
            2100,
            true,
        ),
        ScreenMatchRule::new("Al Salam Sync", &["al salam", "salam sync"], 3072, 576, true),
        ScreenMatchRule::new("Piccadilly", &["piccadilly single"], 2027, 720, true),
        ScreenMatchRule::new("Piccadilly Combined", &["piccadilly combined"], 6080, 720, true),
    ]
}

impl Default for ScreenPresetRules {
    fn default() -> Self {
        Self {
            enabled: true,
            better_res_id: default_better_res(),
            installation_id: default_installation(),
            ultra_wide_aspect: default_ultra_wide_aspect(),
            ultra_wide_names: default_ultra_wide_names(),
            listed: default_listed_screens(),
        }
    }
}

impl ScreenPresetRules {
    /// Preset ids that still participate in automatic per-screen assignment.
    pub fn is_auto_eligible(&self, preset_id: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let id = preset_id.trim();
        if id.is_empty() {
            return true;
        }
        let lower = id.to_ascii_lowercase();
        let better = self.better_res_id.to_ascii_lowercase();
        let inst = self.installation_id.to_ascii_lowercase();
        matches!(
            lower.as_str(),
            "auto"
                | "system.auto"
                | "h264"
                | "h.264"
                | "mp4"
                | "h264-mp4"
                | "system.h264-mp4"
                | "better-res"
                | "better res"
                | "system.better-res"
                | "installation"
                | "system.installation"
        ) || lower == better
            || lower == inst
    }

    /// `auto` / empty / `system.auto` — EffectCraft can send this instead of Match Source.
    pub fn is_auto_token(preset_id: &str) -> bool {
        matches!(preset_id.trim().to_ascii_lowercase().as_str(), "" | "auto" | "system.auto")
    }

    /// Pick Better Res or Installation when this item has a screen identity.
    ///
    /// `screen` is `sizes[].name` / Screen Suite. `source_name` is the composition or file name.
    /// `pixels` is the comp / source raster when EffectCraft did not send `sizes[]`.
    pub fn pick(&self, screen: Option<&ScreenSize>, source_name: &str, pixels: Option<(u32, u32)>) -> Option<PresetPick> {
        if !self.enabled {
            return None;
        }
        if let Some(sz) = screen {
            let w = if sz.width > 0 { Some(sz.width) } else { pixels.map(|p| p.0) };
            let h = if sz.height > 0 { Some(sz.height) } else { pixels.map(|p| p.1) };
            return Some(self.classify(&sz.name, w, h));
        }
        let (parsed_w, parsed_h) = parse_dims_from_text(source_name);
        let w = pixels.map(|p| p.0).filter(|n| *n > 0).or(parsed_w);
        let h = pixels.map(|p| p.1).filter(|n| *n > 0).or(parsed_h);
        if self.has_name_identity(source_name) || self.dims_are_identity(w, h) {
            return Some(self.classify(source_name, w, h));
        }
        None
    }

    fn classify(&self, name: &str, width: Option<u32>, height: Option<u32>) -> PresetPick {
        if self.is_ultra_wide(name, width, height) {
            return PresetPick { preset_id: self.better_res_id.clone(), reason: PresetReason::UltraWide };
        }
        if self.is_listed(name, width, height) {
            return PresetPick { preset_id: self.better_res_id.clone(), reason: PresetReason::ListedScreen };
        }
        PresetPick { preset_id: self.installation_id.clone(), reason: PresetReason::OtherScreen }
    }

    pub fn is_ultra_wide(&self, name: &str, width: Option<u32>, height: Option<u32>) -> bool {
        if name_matches_any(name, &self.ultra_wide_names) {
            return true;
        }
        let (Some(w), Some(h)) = (width, height) else {
            return false;
        };
        if h == 0 || w <= h {
            return false;
        }
        (w as f64 / h as f64) > self.ultra_wide_aspect
    }

    pub fn is_listed(&self, name: &str, width: Option<u32>, height: Option<u32>) -> bool {
        self.listed.iter().any(|rule| rule_matches(rule, name, width, height))
    }

    fn has_name_identity(&self, name: &str) -> bool {
        if name.trim().is_empty() {
            return false;
        }
        name_matches_any(name, &self.ultra_wide_names) || self.listed.iter().any(|r| name_matches_rule(name, r))
    }

    fn dims_are_identity(&self, width: Option<u32>, height: Option<u32>) -> bool {
        let (Some(w), Some(h)) = (width, height) else {
            return false;
        };
        if h > 0 && w > h && (w as f64 / h as f64) > self.ultra_wide_aspect {
            return true;
        }
        self.listed.iter().any(|r| r.match_dims && r.width == Some(w) && r.height == Some(h))
    }

    /// Rebuild `listed` from user-editable lines (`Name | alias, alias | 1920x1080`).
    pub fn set_listed_from_text(&mut self, text: &str) {
        self.listed = text.lines().filter_map(parse_rule_line).collect();
    }

    pub fn listed_as_text(&self) -> String {
        self.listed.iter().map(format_rule_line).collect::<Vec<_>>().join("\n")
    }

    pub fn ultra_wide_names_text(&self) -> String {
        self.ultra_wide_names.join("\n")
    }

    pub fn set_ultra_wide_names_from_text(&mut self, text: &str) {
        self.ultra_wide_names = text.lines().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
    }
}

/// Collapse punctuation so "The-Avenues_Gate.mp4" matches "The Avenues Gate".
pub fn normalize_screen_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn padded(s: &str) -> String {
    format!(" {s} ")
}

fn contains_phrase(haystack: &str, needle: &str) -> bool {
    let h = normalize_screen_name(haystack);
    let n = normalize_screen_name(needle);
    if n.is_empty() || h.is_empty() {
        return false;
    }
    padded(&h).contains(&padded(&n))
}

fn name_matches_any(haystack: &str, names: &[String]) -> bool {
    names.iter().any(|n| contains_phrase(haystack, n))
}

fn name_matches_rule(haystack: &str, rule: &ScreenMatchRule) -> bool {
    contains_phrase(haystack, &rule.name) || rule.aliases.iter().any(|a| contains_phrase(haystack, a))
}

fn rule_matches(rule: &ScreenMatchRule, name: &str, width: Option<u32>, height: Option<u32>) -> bool {
    if name_matches_rule(name, rule) {
        return true;
    }
    if rule.match_dims {
        if let (Some(w), Some(h), Some(rw), Some(rh)) = (width, height, rule.width, rule.height) {
            return w == rw && h == rh;
        }
    }
    false
}

fn parse_dims_from_text(s: &str) -> (Option<u32>, Option<u32>) {
    let t = s.replace('×', "x");
    let bytes = t.as_bytes();
    let mut i = 0;
    let mut last = None;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'x' || bytes[i] == b'X') {
                let mid = i;
                i += 1;
                let h0 = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > h0 {
                    if let (Ok(w), Ok(h)) = (t[start..mid].parse::<u32>(), t[h0..i].parse::<u32>()) {
                        if w > 0 && h > 0 {
                            last = Some((w, h));
                        }
                    }
                    continue;
                }
            }
        } else {
            i += 1;
        }
    }
    match last {
        Some((w, h)) => (Some(w), Some(h)),
        None => (None, None),
    }
}

fn format_rule_line(r: &ScreenMatchRule) -> String {
    let mut s = r.name.clone();
    if !r.aliases.is_empty() {
        s.push_str(" | ");
        s.push_str(&r.aliases.join(", "));
    }
    if let (Some(w), Some(h)) = (r.width, r.height) {
        s.push_str(&format!(" | {w}x{h}"));
    }
    s
}

fn parse_rule_line(line: &str) -> Option<ScreenMatchRule> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let parts: Vec<&str> = line.split('|').map(str::trim).filter(|s| !s.is_empty()).collect();
    let name = (*parts.first()?).to_string();
    let mut aliases = Vec::new();
    let mut width = None;
    let mut height = None;
    for p in parts.iter().skip(1) {
        if let Some((w, h)) = ScreenSize::parse_dims(p) {
            width = Some(w);
            height = Some(h);
        } else {
            for a in p.split(',') {
                let a = a.trim();
                if !a.is_empty() {
                    aliases.push(a.to_string());
                }
            }
        }
    }
    let match_dims = match (width, height) {
        (Some(1920), Some(1080)) | (Some(1080), Some(1920)) => false,
        (Some(_), Some(_)) => true,
        _ => false,
    };
    Some(ScreenMatchRule { name, aliases, width, height, match_dims })
}

/// Infer a [`ScreenSize`] from a file / composition name when EffectCraft did not send `sizes[]`.
pub fn infer_screen_from_text(text: &str, rules: &ScreenPresetRules) -> Option<ScreenSize> {
    let (w, h) = parse_dims_from_text(text);
    if rules.has_name_identity(text) || rules.dims_are_identity(w, h) {
        let name = {
            let n = normalize_screen_name(text);
            if n.is_empty() {
                "screen".into()
            } else {
                text.trim().to_string()
            }
        };
        return Some(ScreenSize::new(name, w.unwrap_or(0), h.unwrap_or(0)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> ScreenPresetRules {
        ScreenPresetRules::default()
    }

    fn pick_name(name: &str, w: u32, h: u32) -> PresetPick {
        let sz = ScreenSize::new(name, w, h);
        rules().pick(Some(&sz), "", None).expect("sized jobs always pick")
    }

    #[test]
    fn avenues_gate_is_ultra_wide_exception() {
        let p = pick_name("The Avenues Gate", 1920, 1080);
        assert_eq!(p.preset_id, BETTER_RES_ID);
        assert_eq!(p.reason, PresetReason::UltraWide);
        assert_eq!(p.reason.label(), "ultra wide");
        let p = pick_name("the-avenues-gate", 100, 100);
        assert_eq!(p.reason, PresetReason::UltraWide);
        let from_file = rules().pick(None, "Hero_The Avenues Gate_v2.mp4", None).expect("name in file");
        assert_eq!(from_file.reason, PresetReason::UltraWide);
        assert_eq!(from_file.preset_id, BETTER_RES_ID);
    }

    #[test]
    fn normal_1920x1080_1_7hd_gets_installation() {
        let p = pick_name("1.7HD", 1920, 1080);
        assert_eq!(p.preset_id, INSTALLATION_ID);
        assert_eq!(p.reason, PresetReason::OtherScreen);
        let p = pick_name("FHD Landscape", 1920, 1080);
        assert_eq!(p.preset_id, INSTALLATION_ID);
    }

    #[test]
    fn avenues_entrance_1920x1080_gets_better_res() {
        for name in [
            "The Avenues Grand Avenues Entrance",
            "The Avenues The Mall Entrance",
            "The Avenues Grand Plaza Entrance",
            "Grand Avenues",
            "the mall entrance",
            "Grand Plaza",
            "Avenues Entrance",
        ] {
            let p = pick_name(name, 1920, 1080);
            assert_eq!(p.preset_id, BETTER_RES_ID, "{name}");
            assert_eq!(p.reason, PresetReason::ListedScreen, "{name}");
        }
    }

    #[test]
    fn ring_road_and_nassar_are_listed() {
        let a = pick_name("1st Ring Road", 1008, 5184);
        assert_eq!(a.preset_id, BETTER_RES_ID);
        assert_eq!(a.reason, PresetReason::ListedScreen);
        let b = pick_name("Al Nassar Tower Vertical", 960, 5328);
        assert_eq!(b.preset_id, BETTER_RES_ID);
        let from_dims = rules().pick(None, "spot_1008x5184.mov", None).expect("unique dims");
        assert_eq!(from_dims.preset_id, BETTER_RES_ID);
        let nassar = rules().pick(None, "out_960x5328.png", None).expect("nassar dims");
        assert_eq!(nassar.preset_id, BETTER_RES_ID);
    }

    #[test]
    fn geometric_ultra_wide_landscape() {
        let examples = [("Eye of Kuwait", 7560, 2100), ("Kuwait Gate", 4460, 378), ("Piccadilly combined", 6080, 720), ("Thuraya", 3584, 608)];
        for (name, w, h) in examples {
            let p = pick_name(name, w, h);
            if (w as f64 / h as f64) > 5.0 {
                assert_eq!(p.reason, PresetReason::UltraWide, "{name} {w}x{h}");
                assert_eq!(p.preset_id, BETTER_RES_ID);
            }
        }
        let kuwait_gate = pick_name("Kuwait Gate", 4460, 378);
        assert_eq!(kuwait_gate.reason, PresetReason::UltraWide);
        let piccadilly = pick_name("Piccadilly combined", 6080, 720);
        assert_eq!(piccadilly.preset_id, BETTER_RES_ID);
        let thuraya = pick_name("Thuraya", 3584, 608);
        assert_eq!(thuraya.reason, PresetReason::UltraWide);
    }

    #[test]
    fn eye_of_kuwait_al_salam_piccadilly_are_listed_better_res() {
        let eye = pick_name("Eye of Kuwait", 7560, 2100);
        assert_eq!(eye.preset_id, BETTER_RES_ID);
        assert_eq!(eye.reason, PresetReason::ListedScreen);
        let avenues = pick_name("The Avenues – Eye of Kuwait", 7560, 2100);
        assert_eq!(avenues.preset_id, BETTER_RES_ID);
        let salam = pick_name("Al Salam Sync", 3072, 576);
        assert_eq!(salam.preset_id, BETTER_RES_ID);
        let pic = pick_name("Piccadilly", 2027, 720);
        assert_eq!(pic.preset_id, BETTER_RES_ID);
        assert_eq!(pic.reason, PresetReason::ListedScreen);
        let pair = pick_name("Piccadilly Combined", 6080, 720);
        assert_eq!(pair.preset_id, BETTER_RES_ID);
    }

    #[test]
    fn portrait_is_not_ultra_wide() {
        let p = pick_name("Some Portrait", 1080, 1920);
        assert_eq!(p.preset_id, INSTALLATION_ID);
        assert_ne!(p.reason, PresetReason::UltraWide);
    }

    #[test]
    fn filename_without_screen_does_not_auto() {
        assert!(rules().pick(None, "hero.mp4", None).is_none());
        assert!(rules().pick(None, "clip_1920x1080.mp4", None).is_none());
        assert_eq!(rules().pick(None, "Honor 400_EN.ai Comp", Some((1008, 5184))).unwrap().preset_id, BETTER_RES_ID);
        assert!(rules().pick(None, "Honor 400_EN.ai Comp", Some((1080, 1080))).is_none());
    }

    #[test]
    fn listed_text_roundtrip() {
        let mut r = ScreenPresetRules::default();
        let text = r.listed_as_text();
        r.set_listed_from_text(&text);
        assert!(r.is_listed("1st Ring Road", Some(1008), Some(5184)));
        r.set_listed_from_text("My LED Wall | led wall | 3840x800\n");
        assert!(r.is_listed("My LED Wall", Some(3840), Some(800)));
        assert!(r.is_listed("led wall", None, None));
    }

    #[test]
    fn disabled_rules_do_not_pick() {
        let mut r = ScreenPresetRules::default();
        r.enabled = false;
        assert!(r.pick(Some(&ScreenSize::new("The Avenues Gate", 100, 100)), "", None).is_none());
    }

    #[test]
    fn auto_eligible_ids() {
        let r = ScreenPresetRules::default();
        assert!(r.is_auto_eligible("system.h264-mp4"));
        assert!(r.is_auto_eligible("h264"));
        assert!(r.is_auto_eligible("auto"));
        assert!(r.is_auto_eligible("system.better-res"));
        assert!(ScreenPresetRules::is_auto_token("auto"));
        assert!(ScreenPresetRules::is_auto_token("system.auto"));
        assert!(!ScreenPresetRules::is_auto_token("system.h264-mp4"));
        assert!(!r.is_auto_eligible("system.hevc-mp4"));
        assert!(!r.is_auto_eligible("user.custom"));
    }
}
