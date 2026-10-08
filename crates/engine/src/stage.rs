//! Duration-weighted job progress: one bar from EffectCraft render through encode passes.
//!
//! Weights start from measured office-PC ratios (ProRes mezzanine, then x264 pass 1 / pass 2)
//! and follow an EMA of finished jobs so the bar and ETA cover the whole encode, not just FFmpeg.

use serde::{Deserialize, Serialize};

const EMA: f64 = 0.35;
const MIN_SAMPLE: f64 = 0.35;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageBook {
    #[serde(default)]
    pub effectcraft_vbr2: ProfileTimes,
    #[serde(default)]
    pub effectcraft_single: ProfileTimes,
    #[serde(default)]
    pub file_vbr2: ProfileTimes,
    #[serde(default)]
    pub file_single: ProfileTimes,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileTimes {
    #[serde(default)]
    pub render_sec: f64,
    #[serde(default)]
    pub pass1_sec: f64,
    #[serde(default)]
    pub pass2_sec: f64,
    #[serde(default)]
    pub encode_sec: f64,
}

impl Default for StageBook {
    fn default() -> Self {
        Self {
            // 6880×1032 / 10 s Better Res on the office PC: long ProRes HQ render, pass 1 ~32 fps, pass 2 ~21 fps.
            effectcraft_vbr2: ProfileTimes { render_sec: 20.0, pass1_sec: 8.0, pass2_sec: 12.0, encode_sec: 15.0 },
            effectcraft_single: ProfileTimes { render_sec: 20.0, pass1_sec: 8.0, pass2_sec: 12.0, encode_sec: 12.0 },
            file_vbr2: ProfileTimes { render_sec: 0.0, pass1_sec: 8.0, pass2_sec: 12.0, encode_sec: 15.0 },
            file_single: ProfileTimes { render_sec: 0.0, pass1_sec: 8.0, pass2_sec: 12.0, encode_sec: 12.0 },
        }
    }
}

impl Default for ProfileTimes {
    fn default() -> Self {
        Self { render_sec: 0.0, pass1_sec: 0.0, pass2_sec: 0.0, encode_sec: 0.0 }
    }
}

impl StageBook {
    fn fill_zeros(&mut self) {
        let d = Self::default();
        fill_profile(&mut self.effectcraft_vbr2, &d.effectcraft_vbr2);
        fill_profile(&mut self.effectcraft_single, &d.effectcraft_single);
        fill_profile(&mut self.file_vbr2, &d.file_vbr2);
        fill_profile(&mut self.file_single, &d.file_single);
    }

    fn profile(&self, has_render: bool, n_passes: usize) -> &ProfileTimes {
        match (has_render, n_passes >= 2) {
            (true, true) => &self.effectcraft_vbr2,
            (true, false) => &self.effectcraft_single,
            (false, true) => &self.file_vbr2,
            (false, false) => &self.file_single,
        }
    }

    fn profile_mut(&mut self, has_render: bool, n_passes: usize) -> &mut ProfileTimes {
        match (has_render, n_passes >= 2) {
            (true, true) => &mut self.effectcraft_vbr2,
            (true, false) => &mut self.effectcraft_single,
            (false, true) => &mut self.file_vbr2,
            (false, false) => &mut self.file_single,
        }
    }
}

fn fill_profile(p: &mut ProfileTimes, d: &ProfileTimes) {
    if p.render_sec <= 0.0 {
        p.render_sec = d.render_sec;
    }
    if p.pass1_sec <= 0.0 {
        p.pass1_sec = d.pass1_sec;
    }
    if p.pass2_sec <= 0.0 {
        p.pass2_sec = d.pass2_sec;
    }
    if p.encode_sec <= 0.0 {
        p.encode_sec = d.encode_sec;
    }
}

fn ema(old: f64, sample: f64) -> f64 {
    if old <= 0.0 {
        sample
    } else {
        EMA * sample + (1.0 - EMA) * old
    }
}

/// One overall bar: stage labels + weights that sum to 1.
#[derive(Clone, Debug)]
pub struct JobProgress {
    pub labels: Vec<String>,
    pub weights: Vec<f32>,
    pub stage_secs: Vec<f64>,
}

impl JobProgress {
    pub fn for_job(has_render: bool, n_passes: usize, book: &StageBook) -> Self {
        let p = book.profile(has_render, n_passes);
        let mut labels = Vec::new();
        let mut secs = Vec::new();
        if has_render {
            labels.push("Rendering comp".into());
            secs.push(p.render_sec.max(1.0));
        }
        if n_passes >= 2 {
            labels.push("Encoding pass 1/2".into());
            secs.push(p.pass1_sec.max(0.5));
            labels.push("Encoding pass 2/2".into());
            secs.push(p.pass2_sec.max(0.5));
        } else {
            labels.push("Encoding".into());
            secs.push(p.encode_sec.max(0.5));
        }
        let sum: f64 = secs.iter().sum::<f64>().max(0.001);
        let weights = secs.iter().map(|s| (*s / sum) as f32).collect();
        Self { labels, weights, stage_secs: secs }
    }

    pub fn overall(&self, stage: usize, stage_pct: f32) -> f32 {
        let done: f32 = self.weights.iter().take(stage).copied().sum();
        let w = self.weights.get(stage).copied().unwrap_or(0.0);
        let frac = (stage_pct / 100.0).clamp(0.0, 1.0);
        ((done + w * frac) * 100.0).clamp(0.0, 99.9)
    }

    pub fn expected_sec(&self, stage: usize) -> f64 {
        self.stage_secs.get(stage).copied().unwrap_or(1.0).max(0.5)
    }

    pub fn label(&self, stage: usize) -> &str {
        self.labels.get(stage).map(String::as_str).unwrap_or("Encoding")
    }
}

pub fn load() -> StageBook {
    let path = crate::paths::data_dir().join("stage-times.json");
    let mut book: StageBook = crate::fsutil::load_json_or_quarantine(&path).unwrap_or_default();
    book.fill_zeros();
    book
}

pub fn record(has_render: bool, n_passes: usize, samples: &[f64]) {
    if samples.is_empty() {
        return;
    }
    let mut book = load();
    let p = book.profile_mut(has_render, n_passes);
    let mut i = 0usize;
    if has_render {
        if let Some(&s) = samples.get(i) {
            if s >= MIN_SAMPLE {
                p.render_sec = ema(p.render_sec, s);
            }
        }
        i += 1;
    }
    if n_passes >= 2 {
        if let Some(&s) = samples.get(i) {
            if s >= MIN_SAMPLE {
                p.pass1_sec = ema(p.pass1_sec, s);
            }
        }
        i += 1;
        if let Some(&s) = samples.get(i) {
            if s >= MIN_SAMPLE {
                p.pass2_sec = ema(p.pass2_sec, s);
            }
        }
    } else if let Some(&s) = samples.get(i) {
        if s >= MIN_SAMPLE {
            p.encode_sec = ema(p.encode_sec, s);
        }
    }
    let path = crate::paths::data_dir().join("stage-times.json");
    let _ = save(&path, &book);
}

fn save(path: &std::path::Path, book: &StageBook) -> Result<(), crate::EngineError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::fsutil::atomic_write(path, &serde_json::to_vec_pretty(book)?)?;
    Ok(())
}

/// Wall-clock per stage; `enter` records the previous sample.
pub struct StageClock {
    started: std::time::Instant,
    samples: Vec<f64>,
    current: usize,
}

impl StageClock {
    pub fn new(n_stages: usize) -> Self {
        Self { started: std::time::Instant::now(), samples: vec![0.0; n_stages], current: 0 }
    }

    pub fn enter(&mut self, idx: usize) {
        if idx == self.current {
            return;
        }
        self.stamp();
        self.current = idx;
        self.started = std::time::Instant::now();
    }

    pub fn finish(&mut self) {
        self.stamp();
    }

    pub fn samples(&self) -> &[f64] {
        &self.samples
    }

    fn stamp(&mut self) {
        let dt = self.started.elapsed().as_secs_f64();
        if self.current < self.samples.len() {
            self.samples[self.current] = dt;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effectcraft_two_pass_bar_moves_during_render() {
        let jp = JobProgress::for_job(true, 2, &StageBook::default());
        assert_eq!(jp.labels, ["Rendering comp", "Encoding pass 1/2", "Encoding pass 2/2"]);
        let sum: f32 = jp.weights.iter().sum();
        assert!((sum - 1.0).abs() < 0.001, "{sum}");
        assert!(jp.overall(0, 0.0) < 1.0);
        let mid_render = jp.overall(0, 50.0);
        assert!(mid_render > 15.0 && mid_render < 40.0, "{mid_render}");
        let end_render = jp.overall(0, 100.0);
        let start_p1 = jp.overall(1, 0.0);
        assert!((end_render - start_p1).abs() < 0.2);
        let mid_p1 = jp.overall(1, 50.0);
        assert!(mid_p1 > end_render + 2.0, "{mid_p1} vs {end_render}");
        let almost = jp.overall(2, 99.0);
        assert!(almost > 90.0 && almost < 100.0, "{almost}");
        // Render is the longest default stage (~20 / 40 s).
        assert!(jp.weights[0] > jp.weights[1] && jp.weights[0] > jp.weights[2]);
    }

    #[test]
    fn file_two_pass_has_no_render_slice() {
        let jp = JobProgress::for_job(false, 2, &StageBook::default());
        assert_eq!(jp.labels[0], "Encoding pass 1/2");
        assert!(jp.overall(0, 50.0) > 10.0);
        assert!(jp.overall(1, 100.0) >= 99.0);
    }

    #[test]
    fn ema_moves_toward_sample() {
        let old = 20.0;
        let next = ema(old, 40.0);
        assert!(next > old && next < 40.0);
    }
}
