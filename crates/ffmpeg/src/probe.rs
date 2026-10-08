use std::path::Path;

use serde::Deserialize;

use super::{Ffmpeg, FfmpegError};

#[derive(Clone, Debug, Default)]
pub struct MediaInfo {
    pub path: std::path::PathBuf,
    pub duration_sec: Option<f64>,
    pub format_name: String,
    pub bit_rate_bps: Option<u64>,
    pub size_bytes: Option<u64>,
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
    pub bit_rate_bps: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct AudioStream {
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bit_rate_bps: Option<u64>,
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

    /// Average video bitrate of the finished file in kbps.
    /// Prefers format size/duration (includes HRD filler) over stream `bit_rate`, which x264
    /// often reports as coded bits without filler NALs.
    pub fn video_average_kbps(&self) -> Option<f32> {
        let total = {
            let from_size = match (self.size_bytes.filter(|s| *s > 0), self.duration_sec.filter(|d| *d > 0.05)) {
                (Some(sz), Some(dur)) => Some((sz as f32) * 8.0 / dur as f32),
                _ => None,
            };
            from_size.or_else(|| self.bit_rate_bps.filter(|b| *b > 0).map(|b| b as f32))?
        };
        let audio = self.audio.as_ref().and_then(|a| a.bit_rate_bps).unwrap_or(0) as f32;
        let video = (total - audio).max(0.0);
        if video < 1.0 {
            None
        } else {
            Some(video / 1000.0)
        }
    }
}

pub fn probe(ff: &Ffmpeg, path: &Path) -> Result<MediaInfo, FfmpegError> {
    let out = super::proc::command(&ff.ffprobe)
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
        bit_rate_bps: raw.format.as_ref().and_then(|f| parse_u64(f.bit_rate.as_deref())),
        size_bytes: raw.format.as_ref().and_then(|f| parse_u64(f.size.as_deref())).or_else(|| std::fs::metadata(path).ok().map(|m| m.len())),
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
                    bit_rate_bps: parse_u64(s.bit_rate.as_deref()),
                });
            }
            Some("audio") if info.audio.is_none() => {
                info.audio = Some(AudioStream {
                    codec: s.codec_name.unwrap_or_default(),
                    sample_rate: s.sample_rate.as_ref().and_then(|x| x.parse().ok()),
                    channels: s.channels.map(|c| c.min(16) as u8),
                    bit_rate_bps: parse_u64(s.bit_rate.as_deref()),
                });
            }
            _ => {}
        }
    }
    Ok(info)
}

fn parse_u64(s: Option<&str>) -> Option<u64> {
    let s = s?.trim();
    if s.is_empty() || s == "N/A" {
        return None;
    }
    s.parse().ok()
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
    bit_rate: Option<String>,
    size: Option<String>,
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
    bit_rate: Option<String>,
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

    #[test]
    fn video_average_prefers_file_size_over_stream_bitrate() {
        let info = MediaInfo {
            duration_sec: Some(2.0),
            size_bytes: Some(500_000),
            bit_rate_bps: Some(25_000),
            video: Some(VideoStream {
                width: 1920,
                height: 1080,
                fps: Some(25.0),
                codec: "h264".into(),
                pix_fmt: "yuv420p".into(),
                bit_rate_bps: Some(25_000),
            }),
            ..MediaInfo::default()
        };
        let kbps = info.video_average_kbps().expect("kbps");
        assert!((kbps - 2000.0).abs() < 1.0, "{kbps}");
    }
}
