use std::io;
use std::path::Path;

use encodecraft_job::{TOKEN_ENV, TOKEN_FILE};

use crate::fsutil;
use crate::paths;

pub fn load_or_create() -> Result<String, io::Error> {
    if let Ok(t) = std::env::var(TOKEN_ENV) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let dir = paths::data_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(TOKEN_FILE);
    if let Ok(s) = std::fs::read_to_string(&path) {
        let t = s.trim().to_string();
        if t.len() >= 16 {
            return Ok(t);
        }
    }
    let token = generate()?;
    fsutil::atomic_write(&path, token.as_bytes())?;
    restrict_mode(&path);
    Ok(token)
}

pub fn current() -> Option<String> {
    load_or_create().ok()
}

fn generate() -> Result<String, io::Error> {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(hex(&buf))
}

fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 0xf) as usize] as char);
    }
    s
}

fn restrict_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Constant-time-ish compare for the local token (length is not secret).
pub fn equals(a: &str, b: &str) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_hex_64() {
        let t = generate().unwrap();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!equals(&t, "0".repeat(64).as_str()));
        assert!(equals(&t, &t));
    }
}
