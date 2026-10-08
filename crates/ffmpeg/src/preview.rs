//! Low-res encode preview: scaled JPEG grab + letterbox geometry.
//!
//! Independent of the main encode graph so turning the panel off costs nothing.

use std::path::Path;
use std::process::{Command, Stdio};

use super::command::{argv_path, even};
use super::{Ffmpeg, FfmpegError};

/// Long side of the JPEG sent to the UI. Even, cheap to decode.
pub const PREVIEW_MAX_LONG: u32 = 480;

/// Fit `src_w×src_h` into a pane, preserving aspect. Caller centres the result.
pub fn letterbox_fit(pane_w: f32, pane_h: f32, src_w: u32, src_h: u32) -> (f32, f32) {
    if src_w == 0 || src_h == 0 || pane_w <= 1.0 || pane_h <= 1.0 {
        return (pane_w.max(0.0), pane_h.max(0.0));
    }
    let aspect = src_w as f32 / src_h as f32;
    let pane_aspect = pane_w / pane_h;
    if pane_aspect > aspect {
        let h = pane_h;
        let w = (h * aspect).max(1.0);
        (w.min(pane_w), h)
    } else {
        let w = pane_w;
        let h = (w / aspect).max(1.0);
        (w, h.min(pane_h))
    }
}

/// Even preview raster whose long side is at most `max_long`, same aspect as the output.
pub fn preview_pixel_size(src_w: u32, src_h: u32, max_long: u32) -> (u32, u32) {
    let w = src_w.max(1);
    let h = src_h.max(1);
    let long = w.max(h);
    if long <= max_long {
        return (even(w).max(2), even(h).max(2));
    }
    let scale = f64::from(max_long) / f64::from(long);
    let ow = even(((f64::from(w) * scale).round() as u32).max(2));
    let oh = even(((f64::from(h) * scale).round() as u32).max(2));
    (ow.max(2), oh.max(2))
}

/// Timecode at 25 fps: `HH:MM:SS:FF`.
pub fn timecode_25(frame: u64) -> String {
    const FPS: u64 = 25;
    let ff = frame % FPS;
    let total = frame / FPS;
    let ss = total % 60;
    let mm = (total / 60) % 60;
    let hh = total / 3600;
    format!("{hh:02}:{mm:02}:{ss:02}:{ff:02}")
}

pub fn frame_at_25(out_time_sec: f64, frame: u64) -> u64 {
    if frame > 0 {
        frame
    } else if out_time_sec.is_finite() && out_time_sec > 0.0 {
        (out_time_sec * 25.0).round().max(0.0) as u64
    } else {
        0
    }
}

/// Grab one JPEG of `input`. When `growing` is set, read near the end of a file still being written.
pub fn grab_preview_jpeg(
    ff: &Ffmpeg,
    dest: &Path,
    output_w: u32,
    output_h: u32,
    input: &Path,
    at_sec: Option<f64>,
    growing: bool,
) -> Result<(), FfmpegError> {
    if !input.exists() {
        return Err(FfmpegError::Message(format!("preview source missing: {}", input.display())));
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let (dw, dh) = preview_pixel_size(output_w.max(2), output_h.max(2), PREVIEW_MAX_LONG);
    let vf = if growing {
        format!("scale={dw}:{dh}:flags=fast_bilinear")
    } else {
        format!("scale={dw}:{dh}:force_original_aspect_ratio=decrease:flags=fast_bilinear,pad={dw}:{dh}:(ow-iw)/2:(oh-ih)/2:black")
    };
    let mut cmd = Command::new(&ff.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
    if growing {
        cmd.args(["-err_detect", "ignore_err", "-fflags", "+genpts+discardcorrupt", "-sseof", "-0.4"]);
    } else if let Some(sec) = at_sec {
        if sec > 0.04 {
            cmd.args(["-ss", &format!("{sec:.3}"), "-noaccurate_seek"]);
        }
    }
    cmd.args(["-i"])
        .arg(argv_path(input))
        .args(["-an", "-sn", "-frames:v", "1", "-vf", &vf, "-q:v", "7", "-threads", "1", "-y"])
        .arg(argv_path(dest))
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(FfmpegError::Probe(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    let len = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    if len < 32 {
        return Err(FfmpegError::Message("preview jpeg empty".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_tall_dooh() {
        let (w, h) = letterbox_fit(400.0, 240.0, 1008, 5184);
        assert!(h > w, "{w}x{h} should be a tall pillar");
        assert!((h - 240.0).abs() < 0.6);
        let aspect = w / h;
        assert!((aspect - 1008.0 / 5184.0).abs() < 0.02, "{aspect}");
    }

    #[test]
    fn letterbox_ultrawide_dooh() {
        let (w, h) = letterbox_fit(400.0, 240.0, 6080, 720);
        assert!(w > h, "{w}x{h} should be a wide letterbox");
        assert!((w - 400.0).abs() < 0.6);
        let aspect = w / h;
        assert!((aspect - 6080.0 / 720.0).abs() < 0.05, "{aspect}");
    }

    #[test]
    fn preview_raster_keeps_aspect() {
        let (w, h) = preview_pixel_size(1008, 5184, 480);
        assert_eq!(h, 480);
        assert!(w < h && w >= 2 && w % 2 == 0, "{w}x{h}");
        let (w, h) = preview_pixel_size(6080, 720, 480);
        assert_eq!(w, 480);
        assert!(h < w && h >= 2 && h % 2 == 0, "{w}x{h}");
    }

    #[test]
    fn timecode_25_fps() {
        assert_eq!(timecode_25(0), "00:00:00:00");
        assert_eq!(timecode_25(25), "00:00:01:00");
        assert_eq!(timecode_25(25 * 60 + 9), "00:01:00:09");
        assert_eq!(frame_at_25(2.0, 0), 50);
        assert_eq!(frame_at_25(2.0, 77), 77);
    }

    #[test]
    fn grab_jpeg_from_testsrc() {
        let Ok(ff) = crate::find_ffmpeg() else {
            eprintln!("skip: ffmpeg not on PATH");
            return;
        };
        let dir = std::env::temp_dir().join(format!("ec-prev-{}-{}", std::process::id(), unix_nanos()));
        let _ = std::fs::create_dir_all(&dir);
        let src = dir.join("src.mp4");
        let dest = dir.join("p.jpg");
        let status = Command::new(&ff.ffmpeg)
            .args(["-hide_banner", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25:duration=0.4", "-frames:v", "4", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status();
        if !status.map(|s| s.success()).unwrap_or(false) {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        grab_preview_jpeg(&ff, &dest, 320, 180, &src, Some(0.1), false).unwrap();
        assert!(dest.is_file());
        assert!(dest.metadata().map(|m| m.len() > 32).unwrap_or(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn unix_nanos() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    }
}
