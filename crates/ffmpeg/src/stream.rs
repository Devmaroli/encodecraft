//! EffectCraft streaming contract (docs/render-streaming-contract.md).
//!
//! Frame bytes never share a stream with JSON. Studio 2-pass H.264 is unchanged:
//! pass 1 reads the live pipe; pass 2 reads the cheap raw intermediate.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use encodecraft_job::JobSource;

use super::proc;
use super::FfmpegError;

/// Ultra-wide if the long side / short side > 5. Very large if the raster is 4K or bigger.
pub const HEAVY_ASPECT: f64 = 5.0;
pub const HEAVY_PIXELS: u64 = 3840 * 2160;

pub fn is_heavy_raster(width: u32, height: u32) -> bool {
    if width < 2 || height < 2 {
        return false;
    }
    let long = width.max(height) as f64;
    let short = width.min(height) as f64;
    if short > 0.0 && long / short > HEAVY_ASPECT {
        return true;
    }
    (width as u64).saturating_mul(height as u64) >= HEAVY_PIXELS
}

pub fn yuv420p_bytes_per_frame(width: u32, height: u32) -> u64 {
    let w = (width & !1).max(2) as u64;
    let h = (height & !1).max(2) as u64;
    let y = w.saturating_mul(h);
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    y.saturating_add(2u64.saturating_mul(cw.saturating_mul(ch)))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CliCaps {
    pub yuv420p: bool,
    pub serve: bool,
    pub gpu_flag: bool,
    pub sidecar: bool,
    pub audio_out: bool,
    pub fps_flag: bool,
}

impl CliCaps {
    pub fn streaming(&self) -> bool {
        self.yuv420p
    }
}

pub fn caps_from_help(render_help: &str, serve_help: &str) -> CliCaps {
    let r = render_help.to_ascii_lowercase();
    let s = serve_help.to_ascii_lowercase();
    CliCaps {
        yuv420p: r.contains("yuv420p") || r.contains("--format raw"),
        serve: s.contains("--control") || s.contains("listening") || serve_help.to_ascii_lowercase().contains("serve"),
        gpu_flag: r.contains("--gpu") || s.contains("--gpu"),
        sidecar: r.contains("--sidecar"),
        audio_out: r.contains("--audio-out") || r.contains("--audio_out"),
        fps_flag: r.contains("--fps"),
    }
}

struct CapsMemo {
    path: PathBuf,
    mtime: Option<SystemTime>,
    len: u64,
    caps: CliCaps,
}

static CAPS_MEMO: Mutex<Option<CapsMemo>> = Mutex::new(None);

fn cli_fingerprint(cli: &Path) -> (Option<SystemTime>, u64) {
    match std::fs::metadata(cli) {
        Ok(m) => (m.modified().ok(), m.len()),
        Err(_) => (None, 0),
    }
}

/// Probe `effectcraft-cli --help`. Cached by executable path + mtime (+ size) so a fresh job is not two ~1 s spawns.
pub fn probe_cli_caps(cli: &Path) -> CliCaps {
    let (mtime, len) = cli_fingerprint(cli);
    if let Ok(g) = CAPS_MEMO.lock() {
        if let Some(memo) = g.as_ref() {
            if memo.path == cli && memo.mtime == mtime && memo.len == len {
                return memo.caps;
            }
        }
    }
    let caps = probe_cli_caps_uncached(cli);
    if let Ok(mut g) = CAPS_MEMO.lock() {
        *g = Some(CapsMemo { path: cli.to_path_buf(), mtime, len, caps });
    }
    caps
}

pub fn probe_cli_caps_uncached(cli: &Path) -> CliCaps {
    let render = help_text(cli, &["render", "--help"]);
    let serve = help_text(cli, &["serve", "--help"]);
    let mut caps = caps_from_help(&render, &serve);
    if serve.is_empty() {
        let top = help_text(cli, &["--help"]);
        if top.to_ascii_lowercase().contains("serve") {
            caps.serve = true;
        }
    }
    caps
}

#[cfg(test)]
pub fn clear_cli_caps_cache() {
    if let Ok(mut g) = CAPS_MEMO.lock() {
        *g = None;
    }
}

fn help_text(cli: &Path, args: &[&str]) -> String {
    let out = proc::command(cli).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output();
    match out {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        }
        Err(_) => String::new(),
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StreamHeader {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub fps: f64,
    #[serde(default)]
    pub frames: u64,
    #[serde(default, rename = "pixFmt")]
    pub pix_fmt: String,
    #[serde(default, rename = "bytesPerFrame")]
    pub bytes_per_frame: u64,
    #[serde(default)]
    pub audio: bool,
    #[serde(default, rename = "sampleRate")]
    pub sample_rate: Option<u32>,
    #[serde(default, rename = "audioChannels")]
    pub audio_channels: Option<u8>,
    #[serde(default)]
    pub gpu: bool,
    #[serde(default)]
    pub comp: Option<String>,
    #[serde(default)]
    pub out: Option<String>,
    #[serde(default, rename = "audioOut")]
    pub audio_out: Option<String>,
}

impl StreamHeader {
    pub fn pix_fmt(&self) -> &str {
        if self.pix_fmt.is_empty() {
            "yuv420p"
        } else {
            self.pix_fmt.as_str()
        }
    }

    pub fn bytes_per_frame(&self) -> u64 {
        if self.bytes_per_frame > 0 {
            self.bytes_per_frame
        } else if self.pix_fmt() == "yuv420p" || self.pix_fmt() == "raw" {
            yuv420p_bytes_per_frame(self.width, self.height)
        } else if self.pix_fmt() == "rgb24" {
            (self.width as u64).saturating_mul(self.height as u64).saturating_mul(3)
        } else if self.pix_fmt() == "rgba" {
            (self.width as u64).saturating_mul(self.height as u64).saturating_mul(4)
        } else {
            yuv420p_bytes_per_frame(self.width, self.height)
        }
    }

    pub fn duration_sec(&self) -> Option<f64> {
        if self.fps > 0.0 && self.frames > 0 {
            Some(self.frames as f64 / self.fps)
        } else {
            None
        }
    }

    pub fn ffmpeg_size(&self) -> String {
        format!("{}x{}", self.width.max(2), self.height.max(2))
    }

    pub fn ffmpeg_fps(&self) -> f64 {
        if self.fps > 0.1 {
            self.fps
        } else {
            25.0
        }
    }
}

#[derive(Clone, Debug)]
pub enum StreamEvent {
    Header(StreamHeader),
    Frame { n: u64, total: u64, elapsed: Option<f64> },
    Gpu { used: bool, reason: Option<String> },
    Done(StreamHeader),
    Cancelled { reason: Option<String> },
    Error { message: String },
    Listening { host: String, port: u16, pid: Option<u32>, gpu: bool, idle_exit: Option<f64> },
    IdleExit { seconds: f64 },
    Other(serde_json::Value),
}

pub fn parse_stream_event(line: &str) -> Option<StreamEvent> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let event = v.get("event")?.as_str()?;
    match event {
        "header" => serde_json::from_value(v).ok().map(StreamEvent::Header),
        "frame" => {
            let n = json_u64(v.get("n").or_else(|| v.get("frame"))).unwrap_or(0);
            let total = json_u64(v.get("total").or_else(|| v.get("frames"))).unwrap_or(0);
            let elapsed = v.get("elapsed").and_then(|x| x.as_f64());
            Some(StreamEvent::Frame { n, total, elapsed })
        }
        "gpu" => Some(StreamEvent::Gpu {
            used: v.get("used").and_then(|x| x.as_bool()).unwrap_or(false),
            reason: v.get("reason").and_then(|x| x.as_str()).map(str::to_string),
        }),
        "done" => {
            let frames = json_u64(v.get("frames"));
            let mut h: StreamHeader = serde_json::from_value(v).unwrap_or_default();
            if h.frames == 0 {
                h.frames = frames.unwrap_or(0);
            }
            Some(StreamEvent::Done(h))
        }
        "cancelled" => Some(StreamEvent::Cancelled { reason: v.get("reason").and_then(|x| x.as_str()).map(str::to_string) }),
        "error" => Some(StreamEvent::Error { message: v.get("message").and_then(|x| x.as_str()).unwrap_or("effectcraft error").to_string() }),
        "listening" => Some(StreamEvent::Listening {
            host: v.get("host").and_then(|x| x.as_str()).unwrap_or("127.0.0.1").to_string(),
            port: json_u64(v.get("port")).unwrap_or(0) as u16,
            pid: json_u64(v.get("pid")).map(|n| n as u32),
            gpu: v.get("gpu").and_then(|x| x.as_bool()).unwrap_or(false),
            idle_exit: v.get("idleExit").or_else(|| v.get("idle_exit")).and_then(|x| x.as_f64()),
        }),
        "idle-exit" => Some(StreamEvent::IdleExit { seconds: v.get("seconds").and_then(|x| x.as_f64()).unwrap_or(0.0) }),
        _ => Some(StreamEvent::Other(v)),
    }
}

fn json_u64(v: Option<&serde_json::Value>) -> Option<u64> {
    let v = v?;
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|i| u64::try_from(i).ok()))
        .or_else(|| v.as_f64().and_then(|f| if f.is_finite() && f >= 0.0 { Some(f as u64) } else { None }))
}

#[derive(Clone, Debug)]
pub struct RawVideoInput {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub pix_fmt: String,
    pub from_stdin: bool,
}

impl RawVideoInput {
    pub fn from_header(h: &StreamHeader, from_stdin: bool) -> Self {
        Self { width: h.width.max(2), height: h.height.max(2), fps: h.ffmpeg_fps(), pix_fmt: h.pix_fmt().to_string(), from_stdin }
    }
}

/// Compact `--fps` value: `25`, `29.97`, `23.976`.
pub fn format_cli_fps(fps: f64) -> String {
    if !fps.is_finite() || fps <= 0.0 {
        return "25".into();
    }
    let r = fps.round();
    if (fps - r).abs() < 0.0005 {
        format!("{}", r as i64)
    } else {
        let s = format!("{fps:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn push_fps_arg(args: &mut Vec<String>, fps: Option<f64>) {
    if let Some(f) = fps.filter(|f| f.is_finite() && *f > 0.1) {
        args.push("--fps".into());
        args.push(format_cli_fps(f));
    }
}

/// Cold streaming argv: frames on stdout, JSON on stderr.
pub fn stream_render_args(
    cli: &Path,
    source: &JobSource,
    out: &Path,
    sidecar: &Path,
    audio_out: Option<&Path>,
    gpu: bool,
    fps: Option<f64>,
) -> Result<Vec<String>, FfmpegError> {
    match source {
        JobSource::EffectCraft { project, comp, work_area, .. } => {
            let mut args = vec![
                cli.to_string_lossy().into_owned(),
                "render".into(),
                "--project".into(),
                project.to_string_lossy().into_owned(),
                "--format".into(),
                "yuv420p".into(),
                "--out".into(),
                if stdout_token(out) { "-".into() } else { out.to_string_lossy().into_owned() },
                "--sidecar".into(),
                sidecar.to_string_lossy().into_owned(),
            ];
            if let Some(c) = comp {
                args.push("--comp".into());
                args.push(c.clone());
            }
            if *work_area {
                args.push("--work-area".into());
            }
            if let Some(a) = audio_out {
                args.push("--audio-out".into());
                args.push(a.to_string_lossy().into_owned());
            }
            if gpu {
                args.push("--gpu".into());
            }
            push_fps_arg(&mut args, fps);
            Ok(args)
        }
        JobSource::File { .. } => Err(FfmpegError::Message("source is already a file".into())),
    }
}

pub fn serve_args(cli: &Path, gpu: bool, idle_exit: u64) -> Vec<String> {
    let mut args = vec![cli.to_string_lossy().into_owned(), "serve".into(), "--control".into(), "0".into(), "--idle-exit".into(), idle_exit.to_string()];
    if gpu {
        args.push("--gpu".into());
    }
    args
}

pub fn stdout_token(path: &Path) -> bool {
    matches!(path.to_str(), Some("-") | Some("stdout") | Some("pipe:1") | Some("pipe:"))
}

/// Wait until `file` has at least `need` bytes, then read `need` bytes from `offset`.
pub fn read_growing_file(path: &Path, offset: u64, need: u64, stop: &dyn Fn() -> bool, timeout: Duration) -> Result<Vec<u8>, FfmpegError> {
    let start = Instant::now();
    loop {
        if stop() {
            return Err(FfmpegError::Message("stopped".into()));
        }
        match std::fs::metadata(path) {
            Ok(m) if m.len() >= offset.saturating_add(need) => break,
            _ => {
                if start.elapsed() > timeout {
                    return Err(FfmpegError::Message(format!("timed out waiting for frame bytes in {}", path.display())));
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        }
    }
    let mut f = std::fs::File::open(path)?;
    use std::io::Seek;
    f.seek(std::io::SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; need as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn read_exact_frame<R: Read>(r: &mut R, bytes: usize, stop: &dyn Fn() -> bool) -> Result<Option<Vec<u8>>, FfmpegError> {
    let mut buf = vec![0u8; bytes];
    let mut got = 0usize;
    while got < bytes {
        if stop() {
            return Err(FfmpegError::Message("stopped".into()));
        }
        match r.read(&mut buf[got..]) {
            Ok(0) => {
                if got == 0 {
                    return Ok(None);
                }
                return Err(FfmpegError::Message(format!("short frame ({got}/{bytes} bytes)")));
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FfmpegError::Io(e)),
        }
    }
    Ok(Some(buf))
}

pub fn load_sidecar(path: &Path) -> Option<StreamHeader> {
    let raw = std::fs::read_to_string(path).ok()?;
    let line = raw.lines().find(|l| l.trim().starts_with('{'))?;
    match parse_stream_event(line) {
        Some(StreamEvent::Header(h)) => Some(h),
        _ => serde_json::from_str(line).ok(),
    }
}

#[allow(dead_code)]
pub fn drain_json_lines<R: Read>(reader: R, mut on_line: impl FnMut(&str)) {
    let buf = BufReader::new(reader);
    for line in buf.lines().map_while(Result::ok) {
        on_line(&line);
    }
}

pub fn intermediate_yuv_path(work_dir: &Path, job_id: &str) -> PathBuf {
    work_dir.join(format!("{job_id}-stream.yuv"))
}

pub fn sidecar_path(work_dir: &Path, job_id: &str) -> PathBuf {
    work_dir.join(format!("{job_id}-header.json"))
}

pub fn audio_wav_path(work_dir: &Path, job_id: &str) -> PathBuf {
    work_dir.join(format!("{job_id}-audio.wav"))
}

/// Raw YUV, sidecar, WAV, and leftover ProRes mezzanine files in the work directory.
pub fn is_work_leftover(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with("-stream.yuv") || n.ends_with("-header.json") || n.ends_with("-audio.wav") || n.contains("-mezzanine.")
}

/// Delete this job's work files. Safe to call on success, failure, cancel, or panic unwind.
pub fn remove_job_work_files(work_dir: &Path, job_id: &str) {
    if job_id.is_empty() {
        return;
    }
    let prefix = format!("{job_id}-");
    if let Ok(rd) = std::fs::read_dir(work_dir) {
        for e in rd.flatten() {
            let name = e.file_name();
            let Some(s) = name.to_str() else { continue };
            if s.starts_with(&prefix) && is_work_leftover(s) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let _ = std::fs::remove_file(intermediate_yuv_path(work_dir, job_id));
    let _ = std::fs::remove_file(sidecar_path(work_dir, job_id));
    let _ = std::fs::remove_file(audio_wav_path(work_dir, job_id));
}

/// Startup sweep: drop leftover `*-stream.yuv`, `*-mezzanine.*`, sidecars, and WAVs.
pub fn sweep_work_dir(work_dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(work_dir) else {
        return 0;
    };
    let mut n = 0usize;
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(s) = name.to_str() else { continue };
        if is_work_leftover(s) && e.path().is_file() && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn yuv420_frame_size_matches_contract() {
        assert_eq!(yuv420p_bytes_per_frame(6880, 1032), 10_650_240);
        assert_eq!(yuv420p_bytes_per_frame(1920, 1080), 3_110_400);
        assert_eq!(yuv420p_bytes_per_frame(2, 2), 6);
    }

    #[test]
    fn heavy_raster_ultrawide_or_4k() {
        assert!(is_heavy_raster(6880, 1032));
        assert!(is_heavy_raster(3840, 2160));
        assert!(!is_heavy_raster(1920, 1080));
        assert!(!is_heavy_raster(1080, 1920));
        assert!(!is_heavy_raster(0, 0));
        assert!(is_heavy_raster(1032, 6880));
    }

    #[test]
    fn caps_detect_yuv_and_serve() {
        let render = "render --format yuv420p|prores  --sidecar FILE  --gpu  --audio-out FILE --fps N";
        let serve = "serve --control PORT  --idle-exit  --gpu";
        let c = caps_from_help(render, serve);
        assert!(c.streaming());
        assert!(c.serve);
        assert!(c.gpu_flag);
        assert!(c.sidecar);
        assert!(c.audio_out);
        assert!(c.fps_flag);
        let old = caps_from_help("render --format prores --out FILE", "");
        assert!(!old.streaming());
        assert!(!old.serve);
        assert!(!old.fps_flag);
        let yuv_no_fps = caps_from_help("render --format yuv420p --sidecar FILE", "serve --control PORT");
        assert!(yuv_no_fps.streaming());
        assert!(!yuv_no_fps.fps_flag);
    }

    #[test]
    fn parse_contract_events() {
        let h = parse_stream_event(
            r#"{"event":"header","version":1,"width":6880,"height":1032,"fps":25.0,"frames":250,"pixFmt":"yuv420p","bytesPerFrame":10650240,"audio":true}"#,
        );
        match h {
            Some(StreamEvent::Header(h)) => {
                assert_eq!(h.width, 6880);
                assert_eq!(h.bytes_per_frame(), 10_650_240);
                assert!(h.audio);
            }
            _ => panic!("header"),
        }
        match parse_stream_event(r#"{"event":"frame","n":1,"total":250,"elapsed":0.04}"#) {
            Some(StreamEvent::Frame { n, total, .. }) => assert_eq!((n, total), (1, 250)),
            _ => panic!("frame"),
        }
        match parse_stream_event(r#"{"event":"listening","host":"127.0.0.1","port":9878,"pid":1234,"gpu":true,"idleExit":120.0}"#) {
            Some(StreamEvent::Listening { port, gpu, .. }) => {
                assert_eq!(port, 9878);
                assert!(gpu);
            }
            _ => panic!("listening"),
        }
        match parse_stream_event(r#"{"event":"gpu","used":false,"reason":"no usable GPU adapter; rendering on the CPU"}"#) {
            Some(StreamEvent::Gpu { used, reason }) => {
                assert!(!used);
                assert!(reason.unwrap().contains("CPU"));
            }
            _ => panic!("gpu"),
        }
        match parse_stream_event(r#"{"event":"done","frames":250,"width":6880,"height":1032}"#) {
            Some(StreamEvent::Done(h)) => assert_eq!(h.frames, 250),
            _ => panic!("done"),
        }
        assert!(stdout_token(Path::new("-")));
        assert!(stdout_token(Path::new("stdout")));
        assert!(!stdout_token(Path::new("clip.yuv")));
        let mut lines = Vec::new();
        drain_json_lines(b"{\"event\":\"frame\",\"n\":1,\"total\":2}\nnot json\n{\"event\":\"done\",\"frames\":2}\n".as_slice(), |l| lines.push(l.to_string()));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn mock_cli_help_enables_streaming_caps() {
        let dir = std::env::temp_dir().join(format!("ec-mockcli-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let script = dir.join("effectcraft-cli");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case " $* " in
  *" render "*" --help "*|*" render --help "*)
    echo "Usage: render --format yuv420p|prores --sidecar FILE --gpu --audio-out FILE --fps N"
    exit 0
    ;;
  *" serve "*" --help "*|*" serve --help "*)
    echo "Usage: serve --control PORT --idle-exit --gpu"
    exit 0
    ;;
esac
echo "effectcraft-cli commands: render serve"
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let caps = probe_cli_caps(&script);
        assert!(caps.streaming(), "{caps:?}");
        assert!(caps.serve, "{caps:?}");
        assert!(caps.gpu_flag);
        assert!(caps.sidecar);
        assert!(caps.audio_out);
        assert!(caps.fps_flag);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_cli_fps_compacts() {
        assert_eq!(format_cli_fps(25.0), "25");
        assert_eq!(format_cli_fps(29.97), "29.97");
        assert_eq!(format_cli_fps(23.976), "23.976");
    }

    fn mock_effectcraft_source() -> JobSource {
        JobSource::EffectCraft {
            project: PathBuf::from("/p/karting.ecproj"),
            comp: Some("Karting".into()),
            mezzanine: "prores".into(),
            work_area: false,
            width: None,
            height: None,
        }
    }

    #[test]
    fn stream_render_args_include_output_fps() {
        let src = mock_effectcraft_source();
        let args = stream_render_args(Path::new("/bin/effectcraft-cli"), &src, Path::new("-"), Path::new("/tmp/h.json"), None, false, Some(25.0)).unwrap();
        assert!(args.windows(2).any(|w| w[0] == "--fps" && w[1] == "25"), "{args:?}");
        assert!(args.windows(2).any(|w| w[0] == "--comp" && w[1] == "Karting"));
        let no_fps = stream_render_args(Path::new("/bin/effectcraft-cli"), &src, Path::new("-"), Path::new("/tmp/h.json"), None, false, None).unwrap();
        assert!(!no_fps.iter().any(|a| a == "--fps"), "{no_fps:?}");
    }

    #[test]
    fn work_leftover_names_match_yuv_and_mezzanine() {
        assert!(is_work_leftover("abc-stream.yuv"));
        assert!(is_work_leftover("abc-mezzanine.mov"));
        assert!(is_work_leftover("abc-mezzanine.MOV"));
        assert!(is_work_leftover("abc-header.json"));
        assert!(is_work_leftover("abc-audio.wav"));
        assert!(!is_work_leftover("keep-me.txt"));
        assert!(!is_work_leftover("queue.json"));
    }

    #[test]
    fn sweep_and_remove_job_work_files() {
        let dir = std::env::temp_dir().join(format!(
            "ec-work-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("job1-stream.yuv"), b"yuv").unwrap();
        std::fs::write(dir.join("job1-header.json"), b"{}").unwrap();
        std::fs::write(dir.join("job1-audio.wav"), b"RIFF").unwrap();
        std::fs::write(dir.join("old-mezzanine.mov"), b"moov").unwrap();
        std::fs::write(dir.join("keep.txt"), b"ok").unwrap();
        remove_job_work_files(&dir, "job1");
        assert!(!dir.join("job1-stream.yuv").exists());
        assert!(!dir.join("job1-header.json").exists());
        assert!(!dir.join("job1-audio.wav").exists());
        assert!(dir.join("old-mezzanine.mov").exists(), "other jobs stay until sweep");
        assert!(dir.join("keep.txt").exists());
        let n = sweep_work_dir(&dir);
        assert!(n >= 1);
        assert!(!dir.join("old-mezzanine.mov").exists());
        assert!(dir.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caps_probe_cached_by_path_and_mtime() {
        clear_cli_caps_cache();
        let dir = std::env::temp_dir().join(format!("ec-capscache-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let script = dir.join("effectcraft-cli");
        let counter = dir.join("helps");
        std::fs::write(&counter, "0").unwrap();
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
echo x >> "{}"
case " $* " in
  *" render "*" --help "*|*" render --help "*)
    echo "Usage: render --format yuv420p --sidecar FILE --gpu --audio-out FILE --fps N"
    exit 0
    ;;
  *" serve "*" --help "*|*" serve --help "*)
    echo "Usage: serve --control PORT --idle-exit --gpu"
    exit 0
    ;;
esac
echo "effectcraft-cli commands: render serve"
"#,
                counter.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let a = probe_cli_caps(&script);
        assert!(a.streaming() && a.fps_flag, "{a:?}");
        let n1 = std::fs::read_to_string(&counter).unwrap().lines().filter(|l| !l.is_empty()).count();
        let b = probe_cli_caps(&script);
        assert_eq!(a, b);
        let n2 = std::fs::read_to_string(&counter).unwrap().lines().filter(|l| !l.is_empty()).count();
        assert_eq!(n1, n2, "second probe must not spawn --help again");
        assert!(n1 >= 2, "first probe runs render --help and serve --help, got {n1}");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
echo x >> "{}"
echo "Usage: render --format yuv420p --sidecar FILE --fps N extra-bytes-to-change-size"
echo "Usage: serve --control PORT"
"#,
                counter.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _ = probe_cli_caps(&script);
        let n3 = std::fs::read_to_string(&counter).unwrap().lines().filter(|l| !l.is_empty()).count();
        assert!(n3 > n2, "changed exe size/mtime must re-probe, {n3} vs {n2}");
        clear_cli_caps_cache();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
