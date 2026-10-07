use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Write `bytes` to `path` via a sibling `.tmp` then rename (replace on Windows).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = tmp_sibling(path);
    std::fs::write(&tmp, bytes)?;
    replace_file(&tmp, path)
}

pub fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = std::fs::remove_file(to);
            std::fs::rename(from, to)
        }
    }
}

fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "file".into());
    name.push(".tmp");
    match path.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// If `bytes` are not valid JSON for `T`, move the file aside and return `None`.
pub fn load_json_or_quarantine<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(_) => {
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let bad = path.with_extension(format!("json.corrupt-{ts}"));
            let _ = std::fs::rename(path, bad);
            None
        }
    }
}

pub fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

/// Two size+mtime samples `apart` apart must match (copy-in-progress guard).
pub fn is_stable(path: &Path, apart: Duration) -> bool {
    let Ok(a) = stat(path) else {
        return false;
    };
    std::thread::sleep(apart);
    let Ok(b) = stat(path) else {
        return false;
    };
    a == b
}

fn stat(path: &Path) -> io::Result<(u64, u64)> {
    let m = std::fs::metadata(path)?;
    let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
    Ok((m.len(), mtime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ec-fs-{}-{}", std::process::id(), encodecraft_queue::unix_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.json");
        atomic_write(&p, br#"{"a":1}"#).unwrap();
        atomic_write(&p, br#"{"a":2}"#).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"{"a":2}"#);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quarantine_bad_json() {
        let dir = std::env::temp_dir().join(format!("ec-fsq-{}-{}", std::process::id(), encodecraft_queue::unix_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("q.json");
        std::fs::write(&p, b"not-json{").unwrap();
        let got: Option<serde_json::Value> = load_json_or_quarantine(&p);
        assert!(got.is_none());
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
