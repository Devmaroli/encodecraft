use std::path::{Path, PathBuf};

use encodecraft_job::{EFFECTCRAFT_CLI_ENV, FFMPEG_ENV};

use super::proc;
use super::FfmpegError;

#[derive(Clone, Debug)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub version: String,
    /// True when this FFmpeg build lists `h264_nvenc`.
    pub nvenc: bool,
    /// True when an NVIDIA GPU is present (driver / nvidia-smi / NVENC DLL).
    pub nvidia: bool,
}

impl Ffmpeg {
    pub fn ok_message(&self) -> String {
        format!("{}  ({})", self.version.lines().next().unwrap_or("ffmpeg"), self.ffmpeg.display())
    }

    /// Toolbar / Settings GPU switch: FFmpeg has NVENC **and** an NVIDIA GPU is attached.
    pub fn gpu_encode_ready(&self) -> bool {
        self.nvenc && self.nvidia
    }
}

/// Master GPU toggle + per-preset allow. Off always means CPU x264.
pub fn use_gpu_encoder(gpu_master: bool, ffmpeg_nvenc: bool, nvidia: bool, allow_gpu: bool, h264: bool) -> bool {
    gpu_master && ffmpeg_nvenc && nvidia && allow_gpu && h264
}

/// `ENCODECRAFT_NVIDIA=0/1` wins; otherwise probe the machine.
pub fn nvidia_gpu_present() -> bool {
    match nvidia_from_env(std::env::var("ENCODECRAFT_NVIDIA").ok().as_deref()) {
        Some(v) => v,
        None => probe_nvidia_gpu(),
    }
}

pub fn nvidia_from_env(raw: Option<&str>) -> Option<bool> {
    let t = raw?.trim();
    if t.is_empty() {
        return None;
    }
    if t == "0" || t.eq_ignore_ascii_case("false") || t.eq_ignore_ascii_case("off") || t.eq_ignore_ascii_case("no") {
        return Some(false);
    }
    if t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("on") || t.eq_ignore_ascii_case("yes") {
        return Some(true);
    }
    None
}

pub fn gpu_unavailable_reason(nvenc: bool, nvidia: bool) -> Option<&'static str> {
    if !nvidia {
        Some("No NVIDIA GPU detected.")
    } else if !nvenc {
        Some("This FFmpeg build has no NVIDIA NVENC encoder.")
    } else {
        None
    }
}

pub fn find_ffmpeg() -> Result<Ffmpeg, FfmpegError> {
    find_ffmpeg_at(None)
}

/// `override_path` is Settings ▸ FFmpeg path (file or directory).
///
/// Search order: that override, then **the folder next to this executable** (so a Windows zip
/// with `ffmpeg.exe` beside `encodecraft.exe` just works), then `FFMPEG_PATH`, then `PATH`,
/// then common install locations.
pub fn find_ffmpeg_at(override_path: Option<&Path>) -> Result<Ffmpeg, FfmpegError> {
    if let Some(p) = override_path {
        if p.is_absolute() && is_exe(p) {
            return ffmpeg_from(p.to_path_buf());
        }
        let nested = p.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
        if p.is_absolute() && is_exe(&nested) {
            return ffmpeg_from(nested);
        }
    }
    for p in sidecar_ffmpeg() {
        if is_exe(&p) {
            return ffmpeg_from(p);
        }
    }
    let ffmpeg = locate_tool("ffmpeg", FFMPEG_ENV, &ffmpeg_candidates())?;
    ffmpeg_from(ffmpeg)
}

/// `ffmpeg` / `ffmpeg.exe` sitting next to the running binary, or in `ffmpeg/` or `bin/` beside it.
pub fn sidecar_ffmpeg() -> Vec<PathBuf> {
    sidecar_tool("ffmpeg")
}

fn sidecar_tool(name: &str) -> Vec<PathBuf> {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let mut v = Vec::new();
    let Ok(exe) = std::env::current_exe() else {
        return v;
    };
    let Some(dir) = exe.parent() else {
        return v;
    };
    v.push(dir.join(&file));
    v.push(dir.join("ffmpeg").join(&file));
    v.push(dir.join("bin").join(&file));
    v
}

fn ffmpeg_from(ffmpeg: PathBuf) -> Result<Ffmpeg, FfmpegError> {
    let ffmpeg = std::fs::canonicalize(&ffmpeg).unwrap_or(ffmpeg);
    let probe_name = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
    let ffprobe = ffmpeg
        .parent()
        .map(|d| d.join(probe_name))
        .filter(|p| p.is_file())
        .or_else(|| locate_tool("ffprobe", "FFPROBE_PATH", &ffprobe_candidates()).ok())
        .unwrap_or_else(|| ffmpeg.clone());
    let ffprobe = std::fs::canonicalize(&ffprobe).unwrap_or(ffprobe);
    let version = read_version(&ffmpeg).unwrap_or_else(|_| "ffmpeg".into());
    let nvenc = probe_nvenc(&ffmpeg);
    let nvidia = nvidia_gpu_present();
    Ok(Ffmpeg { ffmpeg, ffprobe, version, nvenc, nvidia })
}

/// Inputs for [`find_effectcraft_cli_with`] so tests can inject registry / PATH without HKCU.
#[derive(Clone, Debug, Default)]
pub struct EffectcraftLookup {
    pub env: Option<PathBuf>,
    pub current_exe: Option<PathBuf>,
    pub registry_install_dir: Option<PathBuf>,
    pub path_dirs: Vec<PathBuf>,
    pub extra: Vec<PathBuf>,
}

pub fn find_effectcraft_cli() -> Result<PathBuf, FfmpegError> {
    let current_exe = std::env::current_exe().ok();
    find_effectcraft_cli_with(&EffectcraftLookup {
        env: std::env::var_os(EFFECTCRAFT_CLI_ENV).map(PathBuf::from),
        registry_install_dir: effectcraft_registry_install_dir(),
        extra: leftover_effectcraft_candidates(current_exe.as_deref()),
        current_exe,
        path_dirs: std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default(),
    })
}

/// Search order: `EFFECTCRAFT_CLI`, next to this executable, HKCU InstallDir,
/// sibling `..\effectcraft\effectcraft-cli`, then PATH (then leftover extras).
pub fn find_effectcraft_cli_with(lookup: &EffectcraftLookup) -> Result<PathBuf, FfmpegError> {
    for c in effectcraft_search_candidates(lookup) {
        if c.is_absolute() && is_exe(&c) {
            return Ok(c);
        }
    }
    Err(FfmpegError::EffectCraftMissing)
}

pub fn effectcraft_search_candidates(lookup: &EffectcraftLookup) -> Vec<PathBuf> {
    let name = cli_name();
    let mut v = Vec::new();
    if let Some(env) = &lookup.env {
        if env.is_absolute() {
            v.push(env.clone());
            v.push(env.join(name));
        }
    }
    if let Some(dir) = lookup.current_exe.as_ref().and_then(|p| p.parent()) {
        v.push(dir.join(name));
    }
    if let Some(dir) = &lookup.registry_install_dir {
        if dir.is_absolute() {
            v.push(dir.join(name));
        }
    }
    if let Some(parent) = lookup.current_exe.as_ref().and_then(|p| p.parent()).and_then(|d| d.parent()) {
        v.push(parent.join("effectcraft").join(name));
    }
    for dir in &lookup.path_dirs {
        if dir.as_os_str().is_empty() || dir == Path::new(".") || !dir.is_absolute() {
            continue;
        }
        v.push(dir.join(name));
    }
    v.extend(lookup.extra.iter().cloned());
    v
}

fn cli_name() -> &'static str {
    if cfg!(windows) {
        "effectcraft-cli.exe"
    } else {
        "effectcraft-cli"
    }
}

#[cfg(windows)]
fn effectcraft_registry_install_dir() -> Option<PathBuf> {
    let out = proc::command("reg").args(["query", r"HKCU\Software\effectcraft", "/v", "InstallDir"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reg_install_dir(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(windows))]
fn effectcraft_registry_install_dir() -> Option<PathBuf> {
    None
}

#[cfg_attr(not(windows), allow(dead_code))]
fn parse_reg_install_dir(stdout: &str) -> Option<PathBuf> {
    for line in stdout.lines() {
        let line = line.trim();
        let lower = line.to_ascii_lowercase();
        if !lower.starts_with("installdir") {
            continue;
        }
        if let Some(idx) = lower.find("reg_sz") {
            let val = line[idx + "reg_sz".len()..].trim();
            if !val.is_empty() {
                return Some(PathBuf::from(val));
            }
        }
        if let Some(idx) = lower.find("reg_expand_sz") {
            let val = line[idx + "reg_expand_sz".len()..].trim();
            if !val.is_empty() {
                return Some(PathBuf::from(val));
            }
        }
    }
    None
}

fn leftover_effectcraft_candidates(current_exe: Option<&Path>) -> Vec<PathBuf> {
    let name = cli_name();
    let mut v = Vec::new();
    if let Some(dir) = current_exe.and_then(|p| p.parent()) {
        // Sibling checkout: encode from target/debug → ../../effectcraft/target/release
        if let Some(root) = dir.parent().and_then(|p| p.parent()) {
            v.push(root.join("effectcraft").join("target").join("release").join(name));
        }
    }
    if cfg!(windows) {
        v.push(PathBuf::from(r"C:\Program Files\EffectCraft\effectcraft-cli.exe"));
    } else {
        v.push(PathBuf::from("/usr/local/bin/effectcraft-cli"));
        v.push(PathBuf::from("/opt/homebrew/bin/effectcraft-cli"));
    }
    v
}

fn locate_tool(name: &str, env_key: &str, extra: &[PathBuf]) -> Result<PathBuf, FfmpegError> {
    if let Ok(p) = std::env::var(env_key) {
        let path = PathBuf::from(p);
        // Relative env values would resolve against the process cwd (a planting trick).
        if path.is_absolute() && is_exe(&path) {
            return Ok(path);
        }
        let nested = path.join(if cfg!(windows) { format!("{name}.exe") } else { name.into() });
        if path.is_absolute() && is_exe(&nested) {
            return Ok(nested);
        }
    }
    if let Some(p) = which(name) {
        return Ok(p);
    }
    for c in extra {
        if c.is_absolute() && is_exe(c) {
            return Ok(c.clone());
        }
    }
    Err(FfmpegError::Missing)
}

fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) && !name.ends_with(".exe") { format!("{name}.exe") } else { name.to_string() };
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        if dir.as_os_str().is_empty() || dir == Path::new(".") || !dir.is_absolute() {
            continue;
        }
        let p = dir.join(&exe);
        if is_exe(&p) {
            return Some(p);
        }
    }
    None
}

fn is_exe(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn read_version(ffmpeg: &Path) -> Result<String, FfmpegError> {
    let out = proc::command(ffmpeg).arg("-version").output()?;
    let s = String::from_utf8_lossy(&out.stdout);
    Ok(s.lines().next().unwrap_or("ffmpeg").to_string())
}

fn probe_nvenc(ffmpeg: &Path) -> bool {
    let out = proc::command(ffmpeg).args(["-hide_banner", "-encoders"]).output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).contains("h264_nvenc"),
        Err(_) => false,
    }
}

fn probe_nvidia_gpu() -> bool {
    for smi in nvidia_smi_candidates() {
        if let Ok(out) = proc::command(&smi).args(["-L"]).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
                if s.contains("gpu") || s.contains("nvidia") {
                    return true;
                }
            }
        }
    }
    #[cfg(windows)]
    {
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let sys32 = sys.join("System32");
        if sys32.join("nvEncodeAPI64.dll").is_file() || sys32.join("nvcuda.dll").is_file() {
            return true;
        }
    }
    #[cfg(unix)]
    {
        if Path::new("/dev/nvidia0").exists() || Path::new("/proc/driver/nvidia/version").is_file() {
            return true;
        }
    }
    false
}

fn nvidia_smi_candidates() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from(if cfg!(windows) { "nvidia-smi.exe" } else { "nvidia-smi" })];
    if cfg!(windows) {
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        v.push(sys.join("System32").join("nvidia-smi.exe"));
        let pf = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        v.push(pf.join(r"NVIDIA Corporation\NVSMI\nvidia-smi.exe"));
    }
    v
}

fn ffmpeg_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        let pf = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        let pfx86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        for root in [pf, pfx86] {
            v.push(root.join(r"ffmpeg\bin\ffmpeg.exe"));
            v.push(root.join(r"Gyan\ffmpeg\bin\ffmpeg.exe"));
        }
        v.push(PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"));
        if let Some(local) = local {
            v.push(local.join(r"Microsoft\WinGet\Links\ffmpeg.exe"));
        }
    } else {
        v.push(PathBuf::from("/usr/bin/ffmpeg"));
        v.push(PathBuf::from("/usr/local/bin/ffmpeg"));
        v.push(PathBuf::from("/opt/homebrew/bin/ffmpeg"));
        v.push(PathBuf::from("/snap/bin/ffmpeg"));
    }
    v
}

fn ffprobe_candidates() -> Vec<PathBuf> {
    ffmpeg_candidates()
        .into_iter()
        .map(|p| {
            let name = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
            p.parent().map(|d| d.join(name)).unwrap_or(p)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_is_actionable() {
        let msg = FfmpegError::Missing.to_string();
        assert!(msg.contains("winget"));
        assert!(msg.contains("FFMPEG_PATH"));
        assert!(msg.contains("same folder"));
    }

    #[test]
    fn ffmpeg_candidates_are_absolute() {
        for p in ffmpeg_candidates() {
            assert!(p.is_absolute() || p.starts_with("C:"), "{p:?}");
        }
    }

    #[test]
    fn nvidia_env_and_encoder_gate() {
        assert_eq!(nvidia_from_env(Some("1")), Some(true));
        assert_eq!(nvidia_from_env(Some("off")), Some(false));
        assert_eq!(nvidia_from_env(Some("maybe")), None);
        assert_eq!(nvidia_from_env(None), None);
        assert!(!use_gpu_encoder(false, true, true, true, true), "master off forces CPU");
        assert!(!use_gpu_encoder(true, true, true, false, true), "preset allow_gpu off");
        assert!(!use_gpu_encoder(true, true, false, true, true), "no NVIDIA GPU");
        assert!(!use_gpu_encoder(true, false, true, true, true), "ffmpeg without nvenc");
        assert!(!use_gpu_encoder(true, true, true, true, false), "not H.264");
        assert!(use_gpu_encoder(true, true, true, true, true));
        assert_eq!(gpu_unavailable_reason(true, false), Some("No NVIDIA GPU detected."));
        assert_eq!(gpu_unavailable_reason(false, true), Some("This FFmpeg build has no NVIDIA NVENC encoder."));
        assert!(gpu_unavailable_reason(true, true).is_none());
        let ready = Ffmpeg { ffmpeg: PathBuf::from("ffmpeg"), ffprobe: PathBuf::from("ffprobe"), version: "n".into(), nvenc: true, nvidia: false };
        assert!(!ready.gpu_encode_ready());
    }

    #[test]
    fn sidecar_names_match_platform() {
        for p in sidecar_ffmpeg() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if cfg!(windows) {
                assert_eq!(name, "ffmpeg.exe", "{p:?}");
            } else {
                assert_eq!(name, "ffmpeg", "{p:?}");
            }
        }
    }

    #[test]
    fn which_skips_dot_and_relative_path() {
        // Plant a dummy next to the test binary; discovery must not pick cwd.
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap();
        let plant = dir.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
        // Even if a file exists here, `which` only walks absolute PATH entries, never `.`.
        let found = super::which("ffmpeg");
        if let Some(p) = found {
            assert!(p.is_absolute());
            assert_ne!(p, plant, "must not use a binary planted next to the test exe via cwd");
        }
    }

    fn plant_cli(dir: &Path) -> PathBuf {
        let _ = std::fs::create_dir_all(dir);
        let p = dir.join(cli_name());
        std::fs::write(&p, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&p).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&p, perms).unwrap();
        }
        p
    }

    #[test]
    fn effectcraft_search_order_env_sidecar_registry_sibling_then_path() {
        let encode = PathBuf::from(r"/home/ada/.local/Programs/Craft/EncodeCraft/encodecraft");
        let registry = PathBuf::from(r"/home/ada/.local/Programs/Craft/effectcraft");
        let path_dir = PathBuf::from("/usr/bin");
        let lookup = EffectcraftLookup {
            env: Some(PathBuf::from("/opt/override/effectcraft-cli")),
            current_exe: Some(encode),
            registry_install_dir: Some(registry),
            path_dirs: vec![path_dir],
            extra: vec![],
        };
        let c = effectcraft_search_candidates(&lookup);
        let name = cli_name();
        assert_eq!(c[0], PathBuf::from("/opt/override/effectcraft-cli"));
        assert_eq!(c[1], PathBuf::from("/opt/override/effectcraft-cli").join(name));
        assert_eq!(c[2], PathBuf::from(r"/home/ada/.local/Programs/Craft/EncodeCraft").join(name));
        assert_eq!(c[3], PathBuf::from(r"/home/ada/.local/Programs/Craft/effectcraft").join(name));
        assert_eq!(c[4], PathBuf::from(r"/home/ada/.local/Programs/Craft/effectcraft").join(name));
        assert_eq!(c[5], PathBuf::from("/usr/bin").join(name));
        assert_eq!(c.len(), 6);
    }

    #[test]
    fn office_install_finds_registry_then_sibling_before_path() {
        let root = std::env::temp_dir().join(format!(
            "ec-craft-lookup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        let encode_dir = root.join("Craft").join("EncodeCraft");
        let sibling_dir = root.join("Craft").join("effectcraft");
        let registry_dir = root.join("registry-install");
        let path_dir = root.join("on-path");
        let _ = std::fs::create_dir_all(&encode_dir);
        let sidecar = plant_cli(&encode_dir);
        let sibling = plant_cli(&sibling_dir);
        let registry = plant_cli(&registry_dir);
        let on_path = plant_cli(&path_dir);
        let exe = encode_dir.join("encodecraft");

        let found = find_effectcraft_cli_with(&EffectcraftLookup {
            env: None,
            current_exe: Some(exe.clone()),
            registry_install_dir: Some(registry_dir.clone()),
            path_dirs: vec![path_dir.clone()],
            extra: vec![],
        })
        .expect("sidecar next to EncodeCraft wins");
        assert_eq!(found, sidecar);

        std::fs::remove_file(&sidecar).unwrap();
        let found = find_effectcraft_cli_with(&EffectcraftLookup {
            env: None,
            current_exe: Some(exe.clone()),
            registry_install_dir: Some(registry_dir.clone()),
            path_dirs: vec![path_dir.clone()],
            extra: vec![],
        })
        .expect("HKCU InstallDir wins after sidecar");
        assert_eq!(found, registry);

        std::fs::remove_file(&registry).unwrap();
        let found = find_effectcraft_cli_with(&EffectcraftLookup {
            env: None,
            current_exe: Some(exe.clone()),
            registry_install_dir: Some(registry_dir),
            path_dirs: vec![path_dir.clone()],
            extra: vec![],
        })
        .expect("sibling Craft\\\\effectcraft wins after registry");
        assert_eq!(found, sibling);

        std::fs::remove_file(&sibling).unwrap();
        let found = find_effectcraft_cli_with(&EffectcraftLookup {
            env: None,
            current_exe: Some(exe),
            registry_install_dir: None,
            path_dirs: vec![path_dir],
            extra: vec![],
        })
        .expect("PATH is last");
        assert_eq!(found, on_path);

        let missing = find_effectcraft_cli_with(&EffectcraftLookup::default());
        assert!(matches!(missing, Err(FfmpegError::EffectCraftMissing)));
        let msg = FfmpegError::EffectCraftMissing.to_string();
        assert!(msg.contains("EFFECTCRAFT_CLI"), "{msg}");
        assert!(msg.contains("InstallDir"), "{msg}");
        assert!(msg.contains("PATH"), "{msg}");
        assert!(msg.contains("sibling") || msg.contains("effectcraft"), "{msg}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_effectcraft_installdir_reg_query() {
        let stdout = "\nHKEY_CURRENT_USER\\Software\\effectcraft\n    InstallDir    REG_SZ    C:\\Users\\ASUS\\AppData\\Local\\Programs\\Craft\\effectcraft\n";
        assert_eq!(parse_reg_install_dir(stdout), Some(PathBuf::from(r"C:\Users\ASUS\AppData\Local\Programs\Craft\effectcraft")));
        assert!(parse_reg_install_dir("ERROR: The system was unable to find the specified registry key or value.").is_none());
    }
}
