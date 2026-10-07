//! Local HTTP + JSON-lines control server on 127.0.0.1.
//!
//! EffectCraft (and `encodecraft-cli enqueue`) POST `/v1/enqueue` with an [`EnqueueRequest`].
//! JSON-lines `{method, params}` is accepted on the same port so the protocol feels like
//! EffectCraft's control channel.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use encodecraft_job::{ControlReply, ControlRequest, EnqueueReply, EnqueueRequest, DEFAULT_CONTROL_PORT};
use serde_json::json;

use crate::{EngineError, Handle};

pub struct IpcServer {
    pub port: u16,
}

impl IpcServer {
    pub fn bind(port: u16, handle: Handle) -> Result<Self, EngineError> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(false)?;
        let actual = listener.local_addr()?.port();
        thread::Builder::new()
            .name("encodecraft-ipc".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    match conn {
                        Ok(s) => {
                            let h = handle.clone();
                            thread::spawn(move || handle_conn(s, h));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| EngineError::Io(e))?;
        Ok(IpcServer { port: actual })
    }
}

fn handle_conn(stream: TcpStream, handle: Handle) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut peek = [0u8; 8];
    let n = match stream.peek(&mut peek) {
        Ok(n) => n,
        Err(_) => return,
    };
    if n >= 4 && (peek.starts_with(b"POST") || peek.starts_with(b"GET ") || peek.starts_with(b"PUT ") || peek.starts_with(b"HEAD") || peek.starts_with(b"OPTI"))
    {
        handle_http(stream, handle);
        return;
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                let reply = match serde_json::from_str::<ControlRequest>(t) {
                    Ok(req) => handle.execute_json(req),
                    Err(e) => ControlReply { id: 0, ok: false, result: None, error: Some(format!("bad json: {e}")) },
                };
                let mut s = reader.get_mut();
                if write_line(&mut s, &reply).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn write_line(s: &mut TcpStream, reply: &ControlReply) -> std::io::Result<()> {
    let mut v = serde_json::to_vec(reply).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    v.push(b'\n');
    s.write_all(&v)?;
    s.flush()
}

fn handle_http(mut stream: TcpStream, handle: Handle) {
    let mut reader = BufReader::new(&mut stream);
    let mut start = String::new();
    if reader.read_line(&mut start).is_err() {
        return;
    }
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() {
            return;
        }
        if h == "\r\n" || h == "\n" || h.is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0).min(4 * 1024 * 1024);
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        let _ = write_http(&mut stream, 400, r#"{"ok":false,"error":"truncated body"}"#);
        return;
    }
    let parts: Vec<&str> = start.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("");
    let path = parts.get(1).copied().unwrap_or("/");

    let (status, payload) = if method == "GET" && (path == "/health" || path == "/v1/health") {
        (200, json!({"ok": true, "product": encodecraft_job::PRODUCT}).to_string())
    } else if method == "POST" && (path == "/v1/enqueue" || path == "/enqueue") {
        match EnqueueRequest::from_json(&body) {
            Ok(req) => match handle.enqueue(req) {
                Ok(ids) => (200, serde_json::to_string(&EnqueueReply { ok: true, ids, error: None }).unwrap_or_else(|_| "{}".into())),
                Err(e) => (400, serde_json::to_string(&EnqueueReply { ok: false, ids: vec![], error: Some(e.to_string()) }).unwrap_or_else(|_| "{}".into())),
            },
            Err(e) => (400, serde_json::to_string(&EnqueueReply { ok: false, ids: vec![], error: Some(e.to_string()) }).unwrap_or_else(|_| "{}".into())),
        }
    } else if method == "POST" && (path == "/v1/control" || path == "/control") {
        match serde_json::from_slice::<ControlRequest>(&body) {
            Ok(req) => {
                let r = handle.execute_json(req);
                (200, serde_json::to_string(&r).unwrap_or_else(|_| "{}".into()))
            }
            Err(e) => (400, json!({"ok": false, "error": e.to_string()}).to_string()),
        }
    } else if method == "GET" && path == "/v1/queue" {
        match handle.dispatch("queue.list", &json!({})) {
            Ok(v) => (200, v.to_string()),
            Err(e) => (500, json!({"ok":false,"error": e.to_string()}).to_string()),
        }
    } else {
        (404, json!({"ok":false,"error":"not found"}).to_string())
    };
    let _ = write_http(&mut stream, status, &payload);
}

fn write_http(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// POST an enqueue request to a running encoder. Returns the job ids.
pub fn post_enqueue(port: u16, req: &EnqueueRequest) -> Result<Vec<String>, EngineError> {
    let body = req.to_vec().map_err(|e| EngineError::Message(e.to_string()))?;
    let resp = http_post(port, "/v1/enqueue", &body)?;
    let reply: EnqueueReply = serde_json::from_str(&resp).map_err(|e| EngineError::Message(e.to_string()))?;
    if reply.ok {
        Ok(reply.ids)
    } else {
        Err(EngineError::Message(reply.error.unwrap_or_else(|| "enqueue failed".into())))
    }
}

pub fn try_connect(port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], port))),
        Duration::from_millis(400),
    )
    .is_ok()
}

pub fn http_post(port: u16, path: &str, body: &[u8]) -> Result<String, EngineError> {
    let mut s = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().map_err(|e| EngineError::Message(format!("{e}")))?, Duration::from_secs(2))?;
    let req =
        format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    s.write_all(req.as_bytes())?;
    s.write_all(body)?;
    s.flush()?;
    let mut r = BufReader::new(s);
    let mut status = String::new();
    r.read_line(&mut status)?;
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        r.read_line(&mut h)?;
        if h == "\r\n" || h == "\n" || h.is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0).min(8 * 1024 * 1024);
        }
    }
    let mut buf = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut buf)?;
    }
    let body = String::from_utf8_lossy(&buf).into_owned();
    if !status.contains("200") {
        return Err(EngineError::Message(body));
    }
    Ok(body)
}

pub fn default_port() -> u16 {
    std::env::var(encodecraft_job::PORT_ENV).ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_CONTROL_PORT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use encodecraft_job::{JobSource, ScreenSize};

    #[test]
    fn http_enqueue_and_health() {
        let home = std::env::temp_dir().join(format!("encodecraft-ipc-{}-{}", std::process::id(), encodecraft_queue::unix_ms()));
        crate::paths::set_home(home);
        let h = crate::Handle::open_core().unwrap();
        let srv = IpcServer::bind(0, h.clone()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert!(try_connect(srv.port));
        let health = super::http_post(srv.port, "/v1/health", b"").unwrap_or_else(|_| {
            // GET not POST — use a raw GET
            use std::io::{Read, Write};
            let mut s = std::net::TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
            s.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").unwrap();
            let mut b = String::new();
            s.read_to_string(&mut b).unwrap();
            b
        });
        assert!(health.contains("EncodeCraft") || health.contains("ok"));
        let req = EnqueueRequest {
            schema: 1,
            source: JobSource::file("/tmp/dooh.mp4"),
            preset_id: "system.h264-mp4".into(),
            preset_override: None,
            output_dir: None,
            naming: Some("{name}_{width}x{height}".into()),
            sizes: vec![ScreenSize::new("A", 1920, 1080), ScreenSize::new("B", 1080, 1920)],
            trim: None,
            scale: None,
            start_queue: false,
        };
        let ids = post_enqueue(srv.port, &req).unwrap();
        assert_eq!(ids.len(), 2);
        let n = h.lock(|s| s.queue.jobs.len());
        assert_eq!(n, 2);
    }
}
