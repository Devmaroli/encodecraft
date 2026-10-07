use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use super::{Ffmpeg, FfmpegError};

#[derive(Clone, Debug, Default)]
pub struct MediaInfo {
    pub path: std::path::PathBuf,
    pub duration_sec: Option<f64>,
    pub format_name: String,
    pub video: Option<VideoStream>,
    pub audio: Option<AudioStream>,
}

#[derive(Clone, Debug)]
pub struct VideoStream {
    pub width: u32,
    pub height: u32,
    pub fps: Option<f64>,
    pub codec: String,
    pub pix_fmt: String,
}

#[derive(Clone, Debug)]
pub struct AudioStream {
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
}

impl MediaInfo {
    pub fn width(&self) -> u32 {
        self.video.as_ref().map(|v| v.width).unwrap_or(0)
    }
    pub fn height(&self) -> u32 {
        self.video.as_ref().map(|v| v.height).unwrap_or(0)
    }
    pub fn summary(&self) -> String {
        let mut bits = Vec::new();
        if let Some(v) = &self.video {
            bits.push(format!("{}×{}", v.width, v.height));
            if let Some(fps) = v.fps {
                bits.push(format!("{fps:.3} fps"));
            }
            if !v.codec.is_empty() {
                bits.push(v.codec.clone());
            }
        }
        if let Some(a) = &self.audio {
            bits.push(a.codec.clone());
        }
        if let Some(d) = self.duration_sec {
            bits.push(format_duration(d));
        }
        if bits.is_empty() {
            self.format_name.clone()
        } else {
            bits.join(" · ")
        }
    }
}

pub fn probe(ff: &Ffmpeg, path: &Path) -> Result<MediaInfo, FfmpegError> {
    let out = Command::new(&ff.ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(super::command::argv_path(path))
        .stdin(std::process::Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(FfmpegError::Probe(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    let raw: ProbeJson = serde_json::from_slice(&out.stdout)?;
    let mut info = MediaInfo {
        path: path.to_path_buf(),
        format_name: raw.format.as_ref().and_then(|f| f.format_name.clone()).unwrap_or_default(),
        ..MediaInfo::default()
    };
    if let Some(d) = raw.format.as_ref().and_then(|f| f.duration.as_ref()).and_then(|s| s.parse::<f64>().ok()) {
        info.duration_sec = Some(d);
    }
    for s in raw.streams {
        match s.codec_type.as_deref() {
            Some("video") if info.video.is_none() => {
                info.video = Some(VideoStream {
                    width: s.width.unwrap_or(0),
                    height: s.height.unwrap_or(0),
                    fps: parse_rate(s.avg_frame_rate.as_deref()).or_else(|| parse_rate(s.r_frame_rate.as_deref())),
                    codec: s.codec_name.unwrap_or_default(),
                    pix_fmt: s.pix_fmt.unwrap_or_default(),
                });
            }
            Some("audio") if info.audio.is_none() => {
                info.audio = Some(AudioStream {
                    codec: s.codec_name.unwrap_or_default(),
                    sample_rate: s.sample_rate.as_ref().and_then(|x| x.parse().ok()),
                    channels: s.channels.map(|c| c.min(16) as u8),
                });
            }
            _ => {}
        }
    }
    Ok(info)
}

fn parse_rate(s: Option<&str>) -> Option<f64> {
    let s = s?;
    if s == "0/0" || s.is_empty() {
        return None;
    }
    if let Some((a, b)) = s.split_once('/') {
        let n: f64 = a.parse().ok()?;
        let d: f64 = b.parse().ok()?;
        if d == 0.0 {
            return None;
        }
        return Some(n / d);
    }
    s.parse().ok()
}

pub fn format_duration(sec: f64) -> String {
    if !sec.is_finite() || sec < 0.0 {
        return "--:--".into();
    }
    let t = sec.round() as i64;
    let h = t / 3600;
    let m = (t % 3600) / 60;
    let s = t % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[derive(Deserialize)]
struct ProbeJson {
    format: Option<FormatObj>,
    #[serde(default)]
    streams: Vec<StreamObj>,
}

#[derive(Deserialize)]
struct FormatObj {
    duration: Option<String>,
    format_name: Option<String>,
}

#[derive(Deserialize)]
struct StreamObj {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    pix_fmt: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ntsc() {
        assert!((parse_rate(Some("30000/1001")).unwrap() - 29.97).abs() < 0.01);
        assert_eq!(parse_rate(Some("0/0")), None);
    }

    #[test]
    fn duration_fmt() {
        assert_eq!(format_duration(75.2), "1:15");
        assert_eq!(format_duration(3661.0), "1:01:01");
    }
}
