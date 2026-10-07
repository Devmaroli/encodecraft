//! Drop-in EffectCraft command: Composition ▸ Add to EncodeCraft Queue.
//!
//! Copy to `crates/engine/src/encoder_handoff.rs` and register `file.sendToEncodeCraft`.
//! Depends only on `encodecraft-job` plus std. Kept in this repo because we cannot push to
//! EffectCraft.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use encodecraft_job::{DEFAULT_CONTROL_PORT, EnqueueReply, EnqueueRequest, JobSource};

pub const COMMAND_ID: &str = "file.sendToEncodeCraft";
pub const MENU: &str = "Composition";
pub const LABEL: &str = "Add to EncodeCraft Queue";

pub struct SendParams {
    pub project: PathBuf,
    pub comp: Option<String>,
    pub preset_id: String,
    pub work_area: bool,
    pub start_queue: bool,
    pub port: u16,
}

impl Default for SendParams {
    fn default() -> Self {
        Self {
            project: PathBuf::new(),
            comp: None,
            preset_id: "system.h264-mp4".into(),
            work_area: false,
            start_queue: true,
            port: DEFAULT_CONTROL_PORT,
        }
    }
}

pub fn send(params: SendParams) -> Result<Vec<String>, String> {
    if params.project.as_os_str().is_empty() {
        return Err("save the project before sending it to EncodeCraft".into());
    }
    let req = EnqueueRequest {
        schema: encodecraft_job::SCHEMA,
        source: JobSource::EffectCraft {
            project: params.project,
            comp: params.comp,
            mezzanine: "prores".into(),
            work_area: params.work_area,
        },
        preset_id: params.preset_id,
        preset_override: None,
        output_dir: None,
        naming: None,
        sizes: Vec::new(),
        trim: None,
        scale: None,
        start_queue: params.start_queue,
    };
    match post(params.port, &req) {
        Ok(ids) => Ok(ids),
        Err(e) => {
            if let Some(path) = drop_inbox(&req) {
                let _ = spawn_encodecraft();
                Err(format!("{e}; wrote inbox job {}", path.display()))
            } else {
                Err(e)
            }
        }
    }
}

fn post(port: u16, req: &EnqueueRequest) -> Result<Vec<String>, String> {
    let body = req.to_vec().map_err(|e| e.to_string())?;
    let mut s = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().map_err(|e| e.to_string())?, Duration::from_secs(2)).map_err(|e| format!("EncodeCraft is not listening on 127.0.0.1:{port}: {e}"))?;
    let head = format!("POST /v1/enqueue HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    s.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    s.write_all(&body).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let json = text.split("\r\n\r\n").nth(1).or_else(|| text.split("\n\n").nth(1)).unwrap_or(&text);
    let reply: EnqueueReply = serde_json::from_str(json.trim()).map_err(|e| format!("bad reply: {e} ({json})"))?;
    if reply.ok { Ok(reply.ids) } else { Err(reply.error.unwrap_or_else(|| "enqueue failed".into())) }
}

fn drop_inbox(req: &EnqueueRequest) -> Option<PathBuf> {
    let dir = inbox_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("effectcraft-job.json");
    std::fs::write(&path, req.to_vec().ok()?).ok()?;
    Some(path)
}

fn inbox_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ENCODECRAFT_HOME") {
        return Some(PathBuf::from(p).join("inbox"));
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")?;
        return Some(PathBuf::from(base).join("EncodeCraft").join("EncodeCraft").join("inbox"));
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        return Some(PathBuf::from(home).join("Library/Application Support/dev.EncodeCraft.EncodeCraft/inbox"));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".local/share/encodecraft/inbox"))
    }
}

fn spawn_encodecraft() -> bool {
    let name = if cfg!(windows) { "encodecraft.exe" } else { "encodecraft" };
    std::process::Command::new(name).spawn().is_ok()
}

/// Example `Session::execute` arm (pseudo — adapt to EffectCraft's command table):
///
/// ```ignore
/// "file.sendToEncodeCraft" => {
///     let project = self.project_path().ok_or("save the project first")?;
///     let comp = params.get("comp").and_then(|v| v.as_str()).map(|s| s.to_string())
///         .or_else(|| self.active_comp_name());
///     let ids = encoder_handoff::send(encoder_handoff::SendParams {
///         project, comp, ..Default::default()
///     })?;
///     json!({ "ids": ids, "encoder": "EncodeCraft" })
/// }
/// ```
#[allow(dead_code)]
fn _docs(path: &Path) -> &Path {
    path
}
