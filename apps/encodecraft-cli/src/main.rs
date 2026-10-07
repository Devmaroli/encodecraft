//! EncodeCraft CLI.
//!
//! ```text
//! encodecraft-cli encode FILE --preset h264 [--out OUT] [--sizes 1920x1080,1080x1920]
//! encodecraft-cli enqueue FILE --preset h264 [--out DIR] [--sizes …] [--start]
//! encodecraft-cli presets
//! encodecraft-cli probe FILE
//! encodecraft-cli ffmpeg
//! encodecraft-cli serve [--port N]     # headless queue + HTTP (EffectCraft handoff)
//! encodecraft-cli print-cmd FILE --preset h264
//! ```

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use encodecraft_engine::{drop_inbox, fallback_naming, fallback_output, read_lock_port, write_lock, Handle};
use encodecraft_ffmpeg::{build_encode_plan, encode_path, find_ffmpeg, probe, EncodeStop, Ffmpeg, PlanInput};
use encodecraft_job::{EnqueueRequest, JobSource, ScaleMode, DEFAULT_CONTROL_PORT, PORT_ENV, PRODUCT};
use encodecraft_presets::{parse_size_list, PresetLibrary};
use encodecraft_queue::JobStatus;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return;
    }
    let cmd = args.remove(0);
    if let Err(e) = dispatch(&cmd, &args) {
        eprintln!("{PRODUCT}-cli: {e}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "\
{PRODUCT} command-line tool

  encode   FILE --preset ID [--out PATH] [--sizes W x H,… ] [--crf N] [--start 0] [--end S]
  enqueue  FILE|--project P.ecproj [--comp NAME] --preset ID [--out DIR] [--sizes …] [--start]
           Adds jobs to a running EncodeCraft (HTTP 127.0.0.1:{DEFAULT_CONTROL_PORT}).
           If the app is not running, the job is dropped in the inbox and the desktop
           app is launched when `encodecraft` is on PATH.
  presets              list builtin + user presets
  probe FILE           print ffprobe summary
  ffmpeg               locate FFmpeg
  print-cmd FILE --preset ID   print the FFmpeg argv (no encode)
  serve [--port N]     headless queue runner + HTTP (for EffectCraft / CI)

Tokens in --naming: {{name}} {{width}} {{height}} {{preset}} {{codec}} {{date}} {{screen}}
"
    );
}

fn dispatch(cmd: &str, args: &[String]) -> Result<(), String> {
    match cmd {
        "encode" => encode_cmd(args),
        "enqueue" => enqueue_cmd(args),
        "presets" => {
            let lib = PresetLibrary::load(&encodecraft_engine::data_dir().join("presets.json")).map_err(|e| e.to_string())?;
            for p in lib.all() {
                println!("{:<24} {}", p.id, p.name);
            }
            Ok(())
        }
        "probe" => {
            let file = pos(args, 0).ok_or("probe FILE")?;
            let ff = find_ffmpeg().map_err(|e| e.to_string())?;
            let info = probe(&ff, Path::new(file)).map_err(|e| e.to_string())?;
            println!("{}", info.summary());
            Ok(())
        }
        "ffmpeg" => {
            let ff = find_ffmpeg().map_err(|e| e.to_string())?;
            println!("{}", ff.ok_message());
            Ok(())
        }
        "print-cmd" => print_cmd(args),
        "serve" => serve_cmd(args),
        other => Err(format!("unknown command `{other}` (try --help)")),
    }
}

fn encode_cmd(args: &[String]) -> Result<(), String> {
    let file = pos(args, 0).ok_or("encode FILE")?;
    let preset_id = opt(args, "--preset").unwrap_or("system.h264-mp4");
    let lib = PresetLibrary::new();
    let mut preset = lib.get_or_parse(preset_id).map_err(|e| e.to_string())?;
    if let Some(sizes) = opt(args, "--sizes") {
        let parsed = parse_size_list(sizes).map_err(|e| e.to_string())?;
        let ff = find_ffmpeg().map_err(|e| e.to_string())?;
        let out_dir = opt(args, "--out").map(PathBuf::from).unwrap_or_else(fallback_output);
        for sz in parsed {
            let mut p = preset.clone();
            p.video.width = Some(sz.width);
            p.video.height = Some(sz.height);
            let dest = out_path(&out_dir, file, &p, sz.width, sz.height);
            run_one(&ff, Path::new(file), dest, &p, args)?;
        }
        return Ok(());
    }
    if let Some(c) = opt(args, "--crf") {
        if let Ok(v) = c.parse::<u8>() {
            preset.video.quality = encodecraft_presets::Quality::crf(v);
        }
    }
    let ff = find_ffmpeg().map_err(|e| e.to_string())?;
    let dest = opt(args, "--out").map(PathBuf::from).unwrap_or_else(|| {
        let stem = Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
        fallback_output().join(format!("{stem}.{}", preset.extension()))
    });
    run_one(&ff, Path::new(file), dest, &preset, args)
}

fn run_one(ff: &Ffmpeg, src: &Path, dest: PathBuf, preset: &encodecraft_presets::ExportPreset, args: &[String]) -> Result<(), String> {
    let trim = encodecraft_job::Trim {
        in_sec: opt(args, "--start").and_then(|s| s.parse().ok()).unwrap_or(0.0),
        out_sec: opt(args, "--end").and_then(|s| s.parse().ok()),
    };
    eprintln!("encoding {} → {}", src.display(), dest.display());
    encode_path(
        ff,
        src,
        dest.clone(),
        preset,
        trim,
        &EncodeStop::new(),
        |p| eprint!("\r{:>5}  {}  ETA {}", p.percent_label(), p.speed_label(), p.eta_label()),
        |l| eprintln!("{l}"),
    )
    .map_err(|e| e.to_string())?;
    eprintln!("\ndone {}", dest.display());
    Ok(())
}

fn print_cmd(args: &[String]) -> Result<(), String> {
    let file = pos(args, 0).ok_or("print-cmd FILE")?;
    let preset_id = opt(args, "--preset").unwrap_or("system.h264-mp4");
    let lib = PresetLibrary::new();
    let preset = lib.get_or_parse(preset_id).map_err(|e| e.to_string())?;
    let dest = opt(args, "--out").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("out.{}", preset.extension())));
    let plan = build_encode_plan(PlanInput { source: Path::new(file), output: dest, preset: &preset, trim: encodecraft_job::Trim::default(), media: None });
    let ff = find_ffmpeg().map(|f| f.ffmpeg).unwrap_or_else(|_| PathBuf::from("ffmpeg"));
    println!("{}", plan.display_cmdline(&ff));
    Ok(())
}

fn enqueue_cmd(args: &[String]) -> Result<(), String> {
    let req = build_request(args)?;
    let port = opt(args, "--port").and_then(|s| s.parse().ok()).or_else(read_lock_port).unwrap_or(DEFAULT_CONTROL_PORT);
    if encodecraft_engine::try_connect(port) {
        let ids = encodecraft_engine::post_enqueue(port, &req).map_err(|e| e.to_string())?;
        println!("queued {}", ids.join(" "));
        return Ok(());
    }
    let path = drop_inbox(&req).map_err(|e| e.to_string())?;
    println!("app not running; dropped {}", path.display());
    if flag(args, "--no-launch") {
        return Ok(());
    }
    launch_app();
    Ok(())
}

fn build_request(args: &[String]) -> Result<EnqueueRequest, String> {
    let project = opt(args, "--project");
    let source = if let Some(p) = project {
        JobSource::EffectCraft {
            project: PathBuf::from(p),
            comp: opt(args, "--comp").map(str::to_string),
            mezzanine: opt(args, "--mezzanine").unwrap_or("prores").to_string(),
            work_area: flag(args, "--work-area"),
        }
    } else {
        let file = pos(args, 0).ok_or("enqueue FILE  (or --project F.ecproj)")?;
        JobSource::file(file)
    };
    let sizes = match opt(args, "--sizes") {
        Some(s) => parse_size_list(s).map_err(|e| e.to_string())?,
        None => Vec::new(),
    };
    Ok(EnqueueRequest {
        schema: 1,
        source,
        preset_id: opt(args, "--preset").unwrap_or("system.h264-mp4").to_string(),
        preset_override: None,
        output_dir: opt(args, "--out").map(PathBuf::from),
        naming: opt(args, "--naming").map(str::to_string),
        sizes,
        trim: None,
        scale: opt(args, "--scale").and_then(ScaleMode::parse_name),
        start_queue: flag(args, "--start"),
    })
}

fn serve_cmd(args: &[String]) -> Result<(), String> {
    let port: u16 =
        opt(args, "--port").and_then(|s| s.parse().ok()).or_else(|| std::env::var(PORT_ENV).ok().and_then(|s| s.parse().ok())).unwrap_or(DEFAULT_CONTROL_PORT);
    let handle = Handle::open().map_err(|e| e.to_string())?;
    handle.lock(|s| {
        if flag(args, "--start") {
            s.queue.running = true;
        }
        s.settings.http_port = port;
    });
    let srv = handle.start_ipc(port).map_err(|e| e.to_string())?;
    let _ = write_lock(srv.port);
    eprintln!("{PRODUCT} headless on 127.0.0.1:{}  (POST /v1/enqueue)", srv.port);
    if let Some(secs) = opt(args, "--timeout").and_then(|s| s.parse::<u64>().ok()) {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            thread::sleep(Duration::from_millis(250));
            let idle = handle.lock(|s| !s.queue.running || s.queue.jobs.iter().all(|j| !j.status.is_active() && j.status != JobStatus::Waiting));
            if idle && handle.lock(|s| s.queue.jobs.iter().any(|j| j.status == JobStatus::Done || j.status == JobStatus::Failed)) {
                // keep serving until timeout unless --exit-when-idle
                if flag(args, "--exit-when-idle") {
                    break;
                }
            }
        }
    } else {
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
    handle.persist();
    encodecraft_engine::clear_lock();
    Ok(())
}

fn launch_app() {
    let name = if cfg!(windows) { "encodecraft.exe" } else { "encodecraft" };
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(name));
        }
    }
    candidates.push(PathBuf::from(name));
    for c in candidates {
        if std::process::Command::new(&c).spawn().is_ok() {
            eprintln!("launched {}", c.display());
            return;
        }
    }
    eprintln!("could not launch {name}; start EncodeCraft yourself — the inbox job will be picked up");
}

fn out_path(dir: &Path, src: &str, preset: &encodecraft_presets::ExportPreset, w: u32, h: u32) -> PathBuf {
    let stem = Path::new(src).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let naming = fallback_naming();
    let ctx = encodecraft_presets::NamingContext::from_preset(&stem, preset, w, h, "cli", 1, "");
    encodecraft_presets::output_path(dir, &naming, &ctx, preset.format.is_sequence())
}

fn pos<'a>(args: &'a [String], i: usize) -> Option<&'a str> {
    args.iter().filter(|a| !a.starts_with('-')).nth(i).map(String::as_str)
}

fn opt<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
    args.windows(2).find(|w| w[0] == key).map(|w| w[1].as_str())
}

fn flag(args: &[String], key: &str) -> bool {
    args.iter().any(|a| a == key)
}
