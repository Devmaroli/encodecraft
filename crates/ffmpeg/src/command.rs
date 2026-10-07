//! Build an FFmpeg argv from a preset + source. Execution uses this argv as-is (no shell).

use std::path::{Path, PathBuf};

use encodecraft_job::{ScaleMode, Trim};
use encodecraft_presets::{ExportPreset, FrameRate, OutputFormat, Quality};

use super::probe::MediaInfo;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodePlan {
    pub args: Vec<String>,
    pub output: PathBuf,
}

impl EncodePlan {
    pub fn display_cmdline(&self, ffmpeg: &Path) -> String {
        let mut bits = vec![quote(&ffmpeg.to_string_lossy())];
        for a in &self.args {
            bits.push(quote(a));
        }
        bits.join(" ")
    }
}

fn quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@+".contains(c)) {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\\\""))
    }
}

/// Round down to even (H.264/HEVC yuv420p requirement).
pub fn even(n: u32) -> u32 {
    n & !1
}

pub struct PlanInput<'a> {
    pub source: &'a Path,
    pub output: PathBuf,
    pub preset: &'a ExportPreset,
    pub trim: Trim,
    pub media: Option<&'a MediaInfo>,
}

pub fn build_encode_plan(input: PlanInput<'_>) -> EncodePlan {
    let p = input.preset;
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into(), "-nostdin".into(), "-progress".into(), "pipe:1".into(), "-nostats".into()];

    if input.trim.in_sec > 0.001 {
        args.push("-ss".into());
        args.push(format_secs(input.trim.in_sec));
    }
    args.push("-i".into());
    args.push(input.source.to_string_lossy().into_owned());
    if let Some(dur) = input.trim.duration_sec(input.media.and_then(|m| m.duration_sec)) {
        args.push("-t".into());
        args.push(format_secs(dur));
    }

    let src_w = input.media.map(|m| m.width()).filter(|w| *w > 0);
    let src_h = input.media.map(|m| m.height()).filter(|h| *h > 0);
    let dst_w = p.video.width.or(src_w).unwrap_or(1920);
    let dst_h = p.video.height.or(src_h).unwrap_or(1080);

    if p.format.has_video() {
        let mut vf = Vec::new();
        if let FrameRate::Custom(fps) = p.video.frame_rate {
            if fps > 0.0 {
                vf.push(format!("fps={fps}"));
            }
        }
        if p.format != OutputFormat::Gif {
            if let Some(filter) = scale_filter(p.video.scale, dst_w, dst_h, p.format) {
                vf.push(filter);
            }
        }
        match p.format {
            OutputFormat::H264Mp4 => h264(&mut args, p, &vf),
            OutputFormat::HevcMp4 => hevc(&mut args, p, &vf),
            OutputFormat::ProResMov => prores(&mut args, p, &vf),
            OutputFormat::WebmVp9 => vp9(&mut args, p, &vf),
            OutputFormat::Gif => gif(&mut args, p, dst_w, dst_h),
            OutputFormat::PngSequence => sequence(&mut args, p, &vf, false),
            OutputFormat::JpegSequence => sequence(&mut args, p, &vf, true),
            OutputFormat::Aac | OutputFormat::Wav | OutputFormat::Mp3 => {}
        }
    } else {
        args.push("-vn".into());
    }

    if p.format.ffmpeg_audio_codec().is_some() && p.audio.enabled && p.format.has_video() {
        audio_encode(&mut args, p);
    } else if p.format.is_audio_only() {
        audio_encode(&mut args, p);
    } else {
        args.push("-an".into());
    }

    if p.format == OutputFormat::H264Mp4 || p.format == OutputFormat::HevcMp4 {
        args.push("-movflags".into());
        args.push("+faststart".into());
    }

    args.push(input.output.to_string_lossy().into_owned());
    EncodePlan { args, output: input.output }
}

fn push_vf(args: &mut Vec<String>, vf: &[String]) {
    if vf.is_empty() {
        return;
    }
    args.push("-vf".into());
    args.push(vf.join(","));
}

fn scale_filter(mode: ScaleMode, w: u32, h: u32, format: OutputFormat) -> Option<String> {
    let (w, h) = if matches!(format, OutputFormat::H264Mp4 | OutputFormat::HevcMp4 | OutputFormat::WebmVp9) {
        (even(w.max(2)), even(h.max(2)))
    } else {
        (w.max(1), h.max(1))
    };
    match mode {
        ScaleMode::Stretch => Some(format!("scale={w}:{h}")),
        ScaleMode::Fit => Some(format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black")),
        ScaleMode::Fill => Some(format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}")),
        ScaleMode::Scale => Some(format!("scale={w}:{h}:force_original_aspect_ratio=decrease")),
    }
}

fn quality_args(q: Quality, crf_flag: &str) -> Vec<String> {
    match q {
        Quality::Crf { value } => vec![crf_flag.into(), value.to_string()],
        Quality::Bitrate { kbps } => vec!["-b:v".into(), format!("{kbps}k")],
        Quality::Quality { value } => vec!["-q:v".into(), jpeg_q(value).to_string()],
    }
}

fn jpeg_q(quality_1_100: u8) -> u8 {
    // FFmpeg mjpeg q:v 2 is high quality, 31 is low. Map 100→2, 1→31.
    let q = quality_1_100.clamp(1, 100) as i32;
    let v = 31 - ((q - 1) * 29) / 99;
    v.clamp(2, 31) as u8
}

fn h264(args: &mut Vec<String>, p: &ExportPreset, vf: &[String]) {
    args.extend(["-c:v".into(), "libx264".into(), "-preset".into(), "medium".into(), "-pix_fmt".into(), "yuv420p".into()]);
    match p.video.quality {
        Quality::Crf { value } => args.extend(["-crf".into(), value.to_string()]),
        Quality::Bitrate { kbps } => args.extend(["-b:v".into(), format!("{kbps}k")]),
        Quality::Quality { value } => args.extend(["-crf".into(), ((100 - value as u16) * 28 / 100).min(28).to_string()]),
    }
    push_vf(args, vf);
}

fn hevc(args: &mut Vec<String>, p: &ExportPreset, vf: &[String]) {
    args.extend(["-c:v".into(), "libx265".into(), "-tag:v".into(), "hvc1".into(), "-pix_fmt".into(), "yuv420p".into()]);
    args.extend(quality_args(p.video.quality, "-crf"));
    args.extend(["-x265-params".into(), "log-level=error".into()]);
    push_vf(args, vf);
}

fn prores(args: &mut Vec<String>, p: &ExportPreset, vf: &[String]) {
    let profile = p.video.prores_profile.unwrap_or(2).min(5);
    args.extend(["-c:v".into(), "prores_ks".into(), "-profile:v".into(), profile.to_string(), "-pix_fmt".into(), "yuv422p10le".into()]);
    push_vf(args, vf);
}

fn vp9(args: &mut Vec<String>, p: &ExportPreset, vf: &[String]) {
    args.extend(["-c:v".into(), "libvpx-vp9".into(), "-row-mt".into(), "1".into(), "-pix_fmt".into(), "yuv420p".into()]);
    match p.video.quality {
        Quality::Crf { value } => args.extend(["-b:v".into(), "0".into(), "-crf".into(), value.to_string()]),
        Quality::Bitrate { kbps } => args.extend(["-b:v".into(), format!("{kbps}k")]),
        Quality::Quality { value } => args.extend(["-b:v".into(), "0".into(), "-crf".into(), (63u16.saturating_sub(value as u16 * 63 / 100)).to_string()]),
    }
    push_vf(args, vf);
}

fn gif(args: &mut Vec<String>, p: &ExportPreset, w: u32, h: u32) {
    let fps = match p.video.frame_rate {
        FrameRate::Custom(f) if f > 0.0 => f,
        _ => 15.0,
    };
    let scale = match p.video.scale {
        ScaleMode::Stretch => format!("scale={w}:{h}:flags=lanczos"),
        _ => format!("scale={w}:{h}:force_original_aspect_ratio=decrease:flags=lanczos"),
    };
    let vf = format!("fps={fps},{scale},split[s0][s1];[s0]palettegen=max_colors=256[p];[s1][p]paletteuse=dither=bayer");
    args.extend(["-vf".into(), vf, "-loop".into(), "0".into()]);
}

fn sequence(args: &mut Vec<String>, p: &ExportPreset, vf: &[String], jpeg: bool) {
    if jpeg {
        args.extend(["-c:v".into(), "mjpeg".into()]);
        if let Quality::Quality { value } = p.video.quality {
            args.extend(["-q:v".into(), jpeg_q(value).to_string()]);
        } else {
            args.extend(["-q:v".into(), "3".into()]);
        }
    } else {
        args.extend(["-c:v".into(), "png".into()]);
    }
    push_vf(args, vf);
    args.extend(["-start_number".into(), "1".into()]);
}

fn audio_encode(args: &mut Vec<String>, p: &ExportPreset) {
    let Some(codec) = p.format.ffmpeg_audio_codec() else {
        args.push("-an".into());
        return;
    };
    args.extend(["-c:a".into(), codec.into()]);
    if codec != "pcm_s16le" && p.audio.bitrate_kbps > 0 {
        args.extend(["-b:a".into(), format!("{}k", p.audio.bitrate_kbps)]);
    }
    if let Some(sr) = p.audio.sample_rate {
        args.extend(["-ar".into(), sr.to_string()]);
    }
    if let Some(ch) = p.audio.channels {
        args.extend(["-ac".into(), ch.to_string()]);
    }
}

fn format_secs(s: f64) -> String {
    format!("{s:.3}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use encodecraft_presets::{PresetLibrary, VideoSettings};

    fn plan_for(id: &str, w: Option<u32>, h: Option<u32>, scale: ScaleMode) -> EncodePlan {
        let lib = PresetLibrary::new();
        let mut p = lib.get(id).unwrap();
        p.video.width = w;
        p.video.height = h;
        p.video.scale = scale;
        build_encode_plan(PlanInput {
            source: Path::new("/clips/ad.mov"),
            output: PathBuf::from("/out/ad.mp4"),
            preset: &p,
            trim: Trim { in_sec: 1.5, out_sec: Some(9.0) },
            media: None,
        })
    }

    fn join(p: &EncodePlan) -> String {
        p.args.join(" ")
    }

    #[test]
    fn h264_uses_libx264_crf_and_faststart() {
        let p = plan_for("system.h264-mp4", Some(1920), Some(1080), ScaleMode::Fit);
        let s = join(&p);
        assert!(s.contains("-c:v libx264"));
        assert!(s.contains("-crf 23"));
        assert!(s.contains("-movflags +faststart"));
        assert!(s.contains("-ss 1.500"));
        assert!(s.contains("-t 7.500"));
        assert!(s.contains("force_original_aspect_ratio=decrease"));
        assert!(s.contains("pad=1920:1080"));
        assert!(s.ends_with("/out/ad.mp4") || s.contains("/out/ad.mp4"));
    }

    #[test]
    fn fill_uses_crop() {
        let p = plan_for("system.h264-mp4", Some(1080), Some(1920), ScaleMode::Fill);
        assert!(join(&p).contains("force_original_aspect_ratio=increase,crop=1080:1920"));
    }

    #[test]
    fn stretch_is_exact_scale() {
        let p = plan_for("system.h264-mp4", Some(1200), Some(960), ScaleMode::Stretch);
        assert!(join(&p).contains("scale=1200:960") || join(&p).contains("scale=1200:960"));
        assert!(!join(&p).contains("force_original_aspect_ratio"));
    }

    #[test]
    fn hevc_tags_hvc1() {
        let s = join(&plan_for("system.hevc-mp4", Some(1920), Some(1080), ScaleMode::Fit));
        assert!(s.contains("libx265"));
        assert!(s.contains("-tag:v hvc1"));
    }

    #[test]
    fn prores_profile_and_mov_codec() {
        let s = join(&plan_for("system.prores-mov", None, None, ScaleMode::Fit));
        assert!(s.contains("prores_ks"));
        assert!(s.contains("-profile:v 2"));
        assert!(s.contains("pcm_s16le"));
    }

    #[test]
    fn webm_vp9_crf_mode_sets_b_v_zero() {
        let s = join(&plan_for("system.webm-vp9", Some(1280), Some(720), ScaleMode::Scale));
        assert!(s.contains("libvpx-vp9"));
        assert!(s.contains("-b:v 0"));
        assert!(s.contains("libopus"));
    }

    #[test]
    fn gif_uses_palette() {
        let s = join(&plan_for("system.gif", Some(480), Some(270), ScaleMode::Scale));
        assert!(s.contains("palettegen"));
        assert!(s.contains("-an"));
    }

    #[test]
    fn png_sequence_pattern_is_caller_path() {
        let lib = PresetLibrary::new();
        let p = lib.get("system.png-sequence").unwrap();
        let plan = build_encode_plan(PlanInput {
            source: Path::new("in.mp4"),
            output: PathBuf::from("/out/clip/frame_%06d.png"),
            preset: &p,
            trim: Trim::default(),
            media: None,
        });
        assert!(join(&plan).contains("frame_%06d.png"));
        assert!(join(&plan).contains("-c:v png"));
    }

    #[test]
    fn audio_only_drops_video() {
        let s = join(&plan_for("system.mp3", None, None, ScaleMode::Fit));
        assert!(s.contains("-vn"));
        assert!(s.contains("libmp3lame"));
    }

    #[test]
    fn bitrate_mode_emits_b_v() {
        let lib = PresetLibrary::new();
        let mut p = lib.get("system.h264-mp4").unwrap();
        p.video = VideoSettings { quality: Quality::bitrate(5000), width: Some(1920), height: Some(1080), ..p.video };
        let plan =
            build_encode_plan(PlanInput { source: Path::new("in.mp4"), output: PathBuf::from("out.mp4"), preset: &p, trim: Trim::default(), media: None });
        assert!(join(&plan).contains("-b:v 5000k"));
        assert!(!join(&plan).contains("-crf"));
    }

    #[test]
    fn even_dims() {
        assert_eq!(even(1920), 1920);
        assert_eq!(even(1081), 1080);
    }
}
