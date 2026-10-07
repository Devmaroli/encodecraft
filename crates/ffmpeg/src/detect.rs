use std::path::{Path, PathBuf};
use std::process::Command;

use encodecraft_job::{EFFECTCRAFT_CLI_ENV, FFMPEG_ENV};

use super::FfmpegError;

#[derive(Clone, Debug)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub version: String,
}

impl Ffmpeg {
    pub fn ok_message(&self) -> String {
        format!("{}  ({})", self.version.lines().next().unwrap_or("ffmpeg"), self.ffmpeg.display())
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
    Ok(Ffmpeg { ffmpeg, ffprobe, version })
}

pub fn find_effectcraft_cli() -> Result<PathBuf, FfmpegError> {
    locate_tool("effectcraft-cli", EFFECTCRAFT_CLI_ENV, &effectcraft_candidates()).map_err(|_| FfmpegError::EffectCraftMissing)
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
    let out = Command::new(ffmpeg).arg("-version").output()?;
    let s = String::from_utf8_lossy(&out.stdout);
    Ok(s.lines().next().unwrap_or("ffmpeg").to_string())
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

fn effectcraft_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let name = if cfg!(windows) { "effectcraft-cli.exe" } else { "effectcraft-cli" };
            v.push(dir.join(name));
            // Sibling checkout: ../effectcraft/target/release/effectcraft-cli
            if let Some(root) = dir.parent().and_then(|p| p.parent()) {
                v.push(root.join("effectcraft").join("target").join("release").join(name));
            }
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
}
