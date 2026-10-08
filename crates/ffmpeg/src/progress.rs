//! Parse FFmpeg `-progress pipe:1` key=value lines.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EncodeProgress {
    pub percent: f32,
    pub out_time_sec: f64,
    pub speed: f32,
    pub fps: f32,
    pub frame: u64,
    pub done: bool,
    pub eta_sec: Option<f64>,
    /// Instantaneous video bitrate from `-progress`, in kbit/s.
    pub bitrate_kbps: Option<f32>,
}

impl EncodeProgress {
    pub fn speed_label(&self) -> String {
        if self.speed <= 0.0 {
            "—".into()
        } else {
            format!("{:.2}×", self.speed)
        }
    }

    pub fn eta_label(&self) -> String {
        match self.eta_sec {
            Some(s) if s.is_finite() && s >= 0.0 => crate::probe::format_duration(s),
            _ => "—".into(),
        }
    }

    pub fn percent_label(&self) -> String {
        if self.done {
            "100%".into()
        } else {
            format!("{:.1}%", self.percent.clamp(0.0, 99.9))
        }
    }
}

/// Update `acc` from one `key=value` progress line. Returns true if the snapshot changed.
pub fn parse_progress_line(line: &str, acc: &mut EncodeProgress, duration_sec: Option<f64>) -> bool {
    let line = line.trim();
    let Some((k, v)) = line.split_once('=') else {
        return false;
    };
    match k {
        "out_time_us" => {
            if let Ok(us) = v.parse::<f64>() {
                acc.out_time_sec = (us / 1_000_000.0).max(0.0);
                refresh_percent(acc, duration_sec);
                return true;
            }
        }
        "out_time_ms" => {
            if let Ok(ms) = v.parse::<f64>() {
                acc.out_time_sec = (ms / 1_000.0).max(0.0);
                refresh_percent(acc, duration_sec);
                return true;
            }
        }
        "out_time" => {
            if let Some(sec) = parse_hms(v) {
                acc.out_time_sec = sec;
                refresh_percent(acc, duration_sec);
                return true;
            }
        }
        "speed" => {
            let t = v.trim().trim_end_matches('x');
            if let Ok(s) = t.parse::<f32>() {
                acc.speed = s;
                refresh_percent(acc, duration_sec);
                return true;
            }
        }
        "fps" => {
            if let Ok(f) = v.parse::<f32>() {
                acc.fps = f;
                return true;
            }
        }
        "frame" => {
            if let Ok(f) = v.parse::<u64>() {
                acc.frame = f;
                return true;
            }
        }
        "bitrate" => {
            let t = v.trim();
            if t.eq_ignore_ascii_case("N/A") {
                return false;
            }
            let n = t.trim_end_matches("kbits/s").trim_end_matches("kbit/s").trim();
            if let Ok(b) = n.parse::<f32>() {
                acc.bitrate_kbps = Some(b.max(0.0));
                return true;
            }
        }
        "progress" => {
            if v == "end" {
                acc.done = true;
                acc.percent = 100.0;
                acc.eta_sec = Some(0.0);
                return true;
            }
        }
        _ => {}
    }
    false
}

fn refresh_percent(acc: &mut EncodeProgress, duration_sec: Option<f64>) {
    if let Some(d) = duration_sec {
        if d > 0.05 {
            acc.percent = ((acc.out_time_sec / d) as f32 * 100.0).clamp(0.0, 99.9);
            if acc.speed > 0.05 {
                let remain = (d - acc.out_time_sec).max(0.0);
                acc.eta_sec = Some(remain / acc.speed as f64);
            }
        }
    }
}

fn parse_hms(s: &str) -> Option<f64> {
    // HH:MM:SS.micro
    let mut parts = s.trim().split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_from_out_time() {
        let mut p = EncodeProgress::default();
        parse_progress_line("out_time_us=5000000", &mut p, Some(10.0));
        parse_progress_line("speed=2.0x", &mut p, Some(10.0));
        assert!((p.percent - 50.0).abs() < 0.2);
        assert_eq!(p.eta_sec.map(|s| s.round() as i32), Some(3)); // 5s remain / 2x
    }

    #[test]
    fn end_is_complete() {
        let mut p = EncodeProgress::default();
        parse_progress_line("progress=end", &mut p, Some(10.0));
        assert!(p.done);
        assert_eq!(p.percent, 100.0);
    }

    #[test]
    fn bitrate_kbits() {
        let mut p = EncodeProgress::default();
        parse_progress_line("bitrate=  7842.3kbits/s", &mut p, None);
        assert!((p.bitrate_kbps.unwrap_or(0.0) - 7842.3).abs() < 0.05);
        parse_progress_line("bitrate=N/A", &mut p, None);
        assert!((p.bitrate_kbps.unwrap_or(0.0) - 7842.3).abs() < 0.05);
    }
}
