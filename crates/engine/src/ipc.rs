//! Local HTTP + JSON-lines control server on **127.0.0.1 only**.
//!
//! Authentication: a per-install token in the user data dir (`ipc-token`), also readable via
//! `ENCODECRAFT_TOKEN`. HTTP clients send `X-EncodeCraft-Token`. JSON-lines clients put
//! `"token"` on each request.
//!
//! Browser-shaped requests are rejected (Host, Origin, CORS). There is no
//! `Access-Control-Allow-Origin` header.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use encodecraft_job::{ControlReply, ControlRequest, EnqueueReply, EnqueueRequest, DEFAULT_CONTROL_PORT, MAX_CONTROL_BODY, PRODUCT, TOKEN_HEADER};

use crate::{token, EngineError, Handle};

const MAX_HEADERS: usize = 64;
const MAX_HEADER_LINE: usize = 8192;
const MAX_CONNS: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

pub struct IpcServer {
    pub port: u16,
}

impl IpcServer {
    pub fn bind(port: u16, handle: Handle) -> Result<Self, EngineError> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        let actual = listener.local_addr()?.port();
        let live = std::sync::Arc::new(AtomicUsize::new(0));
        thread::Builder::new()
            .name("encodecraft-ipc".into())
            .spawn(move || {
                while handle.is_alive() {
                    match listener.accept() {
                        Ok((s, _)) => {
                            if live.load(Ordering::SeqCst) >= MAX_CONNS {
                                write_http_raw(&s, 503, r#"{"ok":false,"error":"busy"}"#);
                                continue;
                            }
                            live.fetch_add(1, Ordering::SeqCst);
                            let h = handle.clone();
                            let live = live.clone();
                            let _ = thread::Builder::new().name("encodecraft-ipc-conn".into()).spawn(move || {
                                handle_conn(s, h);
                                live.fetch_sub(1, Ordering::SeqCst);
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => {
                            thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(EngineError::Io)?;
        Ok(IpcServer { port: actual })
    }
}

fn handle_conn(stream: TcpStream, handle: Handle) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
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
    let expected = handle.lock(|s| s.ipc_token.clone());
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(n) if n > MAX_CONTROL_BODY => break,
            Ok(_) => {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                if t.len() > MAX_CONTROL_BODY {
                    break;
                }
                let reply = match serde_json::from_str::<ControlRequest>(t) {
                    Ok(req) => {
                        if !token::equals(req.token.as_deref().unwrap_or(""), &expected) {
                            ControlReply { id: req.id, ok: false, result: None, error: Some("unauthorized".into()) }
                        } else {
                            handle.execute_json(req)
                        }
                    }
                    Err(e) => ControlReply { id: 0, ok: false, result: None, error: Some(format!("bad json: {e}")) },
                };
                let s = reader.get_mut();
                if write_line(s, &reply).is_err() {
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
    if reader.read_line(&mut start).is_err() || start.len() > MAX_HEADER_LINE {
        return;
    }
    let mut host = String::new();
    let mut origin: Option<String> = None;
    let mut content_type = String::new();
    let mut content_length = 0usize;
    let mut req_token = String::new();
    let mut sec_fetch_site = String::new();
    let mut expect_continue = false;
    let mut n_headers = 0usize;
    loop {
        if n_headers >= MAX_HEADERS {
            let _ = write_http(&mut stream, 431, r#"{"ok":false,"error":"too many headers"}"#);
            return;
        }
        n_headers += 1;
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h.len() > MAX_HEADER_LINE {
            return;
        }
        if h == "\r\n" || h == "\n" || h.is_empty() {
            break;
        }
        let (name, value) = split_header(&h);
        match name.as_str() {
            "host" => host = value,
            "origin" => origin = Some(value),
            "content-type" => content_type = value,
            "content-length" => content_length = value.parse().unwrap_or(0),
            "expect" => expect_continue = value.to_ascii_lowercase().contains("100-continue"),
            "x-encodecraft-token" | "authorization" => {
                req_token = value.strip_prefix("Bearer ").unwrap_or(&value).trim().to_string();
            }
            "sec-fetch-site" => sec_fetch_site = value,
            _ => {}
        }
    }
    if content_length > MAX_CONTROL_BODY {
        let _ = write_http(&mut stream, 413, r#"{"ok":false,"error":"payload too large"}"#);
        return;
    }
    if expect_continue && write_continue(reader.get_mut()).is_err() {
        return;
    }
    let body = match read_exact_len(&mut reader, content_length) {
        Ok(b) => b,
        Err(_) => {
            let _ = write_http(&mut stream, 400, r#"{"ok":false,"error":"truncated body"}"#);
            return;
        }
    };
    let parts: Vec<&str> = start.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("");
    let path = parts.get(1).copied().unwrap_or("/").split('?').next().unwrap_or("/");

    if !host_ok(&host) {
        let _ = write_http(&mut stream, 403, r#"{"ok":false,"error":"bad host"}"#);
        return;
    }
    if let Some(ref o) = origin {
        if !origin_ok(o) {
            let _ = write_http(&mut stream, 403, r#"{"ok":false,"error":"browser origin rejected"}"#);
            return;
        }
    }
    if !sec_fetch_site.is_empty() && sec_fetch_site != "none" && sec_fetch_site != "same-origin" {
        let _ = write_http(&mut stream, 403, r#"{"ok":false,"error":"cross-site request rejected"}"#);
        return;
    }

    let expected = handle.lock(|s| s.ipc_token.clone());
    let is_health = method == "GET" && (path == "/health" || path == "/v1/health");
    if !is_health && !token::equals(&req_token, &expected) {
        let _ = write_http(&mut stream, 401, r#"{"ok":false,"error":"unauthorized"}"#);
        return;
    }

    if method == "POST" && !content_type.to_ascii_lowercase().starts_with("application/json") {
        let _ = write_http(&mut stream, 415, r#"{"ok":false,"error":"Content-Type must be application/json"}"#);
        return;
    }

    let (status, payload) = if is_health {
        (200, serde_json::json!({"ok": true, "product": PRODUCT}).to_string())
    } else if method == "POST" && (path == "/v1/enqueue" || path == "/enqueue") {
        match EnqueueRequest::from_json(&body) {
            Ok(req) => match handle.enqueue(req) {
                Ok(ids) => {
                    let n = ids.len();
                    let running = handle.lock(|s| s.queue.running);
                    handle.lock(|s| s.push_log(format!("Queued {n} job(s) from EffectCraft / HTTP")));
                    let message = if running {
                        format!("{n} job{} added", if n == 1 { "" } else { "s" })
                    } else {
                        format!("{n} job{} added · Start", if n == 1 { "" } else { "s" })
                    };
                    (200, serde_json::to_string(&EnqueueReply { ok: true, ids, error: None, message: Some(message) }).unwrap_or_else(|_| "{}".into()))
                }
                Err(e) => {
                    let msg = e.to_string();
                    handle.lock(|s| s.push_log(format!("Enqueue rejected (HTTP 400): {msg}")));
                    (400, serde_json::to_string(&EnqueueReply { ok: false, ids: vec![], error: Some(msg), message: None }).unwrap_or_else(|_| "{}".into()))
                }
            },
            Err(e) => {
                let msg = e.to_string();
                handle.lock(|s| s.push_log(format!("Enqueue rejected (HTTP 400): {msg}")));
                (400, serde_json::to_string(&EnqueueReply { ok: false, ids: vec![], error: Some(msg), message: None }).unwrap_or_else(|_| "{}".into()))
            }
        }
    } else if method == "POST" && (path == "/v1/control" || path == "/control") {
        match serde_json::from_slice::<ControlRequest>(&body) {
            Ok(req) => {
                let r = handle.execute_json(req);
                (200, serde_json::to_string(&r).unwrap_or_else(|_| "{}".into()))
            }
            Err(e) => (400, serde_json::json!({"ok": false, "error": e.to_string()}).to_string()),
        }
    } else if method == "POST" && (path == "/v1/queue/start" || path == "/queue/start") {
        handle.lock(|s| s.queue.start());
        (200, serde_json::json!({"ok": true, "running": true}).to_string())
    } else if method == "GET" && path == "/v1/queue" {
        match handle.dispatch("queue.list", &serde_json::json!({})) {
            Ok(v) => (200, v.to_string()),
            Err(e) => (500, serde_json::json!({"ok":false,"error": e.to_string()}).to_string()),
        }
    } else {
        (404, serde_json::json!({"ok":false,"error":"not found"}).to_string())
    };
    let _ = write_http(&mut stream, status, &payload);
}

fn split_header(line: &str) -> (String, String) {
    let line = line.trim_end_matches(['\r', '\n']);
    match line.split_once(':') {
        Some((n, v)) => (n.trim().to_ascii_lowercase(), v.trim().to_string()),
        None => (String::new(), String::new()),
    }
}

fn host_ok(host: &str) -> bool {
    let h = host.trim().trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    let name = h.split(':').next().unwrap_or("");
    matches!(name, "127.0.0.1" | "localhost" | "::1")
}

fn origin_ok(origin: &str) -> bool {
    let o = origin.trim().to_ascii_lowercase();
    if o.is_empty() || o == "null" {
        return false;
    }
    o.starts_with("http://127.0.0.1") || o.starts_with("http://localhost") || o.starts_with("http://[::1]")
}

fn write_continue(stream: &mut impl Write) -> std::io::Result<()> {
    stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    stream.flush()
}

/// Read until `len` bytes arrive (or EOF / timeout). `read_exact` already loops; this
/// also retries `Interrupted` so a Content-Length body from Invoke-RestMethod is not
/// treated as truncated when it lands in several reads.
fn read_exact_len<R: Read>(reader: &mut R, len: usize) -> std::io::Result<Vec<u8>> {
    let mut body = vec![0u8; len];
    let mut got = 0usize;
    while got < len {
        match reader.read(&mut body[got..]) {
            Ok(0) => {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "truncated body"));
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(body)
}

fn write_http(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn write_http_raw(stream: &TcpStream, status: u16, body: &str) {
    if let Ok(mut s) = stream.try_clone() {
        let _ = write_http(&mut s, status, body);
    }
}

/// POST an enqueue request to a running encoder. Returns the job ids.
pub fn post_enqueue(port: u16, req: &EnqueueRequest) -> Result<Vec<String>, EngineError> {
    let body = req.to_vec().map_err(|e| EngineError::Message(e.to_string()))?;
    let tok = token::current();
    let resp = http_post(port, "/v1/enqueue", &body, tok.as_deref())?;
    let reply: EnqueueReply = serde_json::from_str(&resp).map_err(|e| EngineError::Message(e.to_string()))?;
    if reply.ok {
        Ok(reply.ids)
    } else {
        Err(EngineError::Message(reply.error.unwrap_or_else(|| "enqueue failed".into())))
    }
}

pub fn post_start(port: u16) -> Result<(), EngineError> {
    let tok = token::current();
    let resp = http_post(port, "/v1/queue/start", b"{}", tok.as_deref())?;
    if resp.contains("\"ok\":true") || resp.contains("\"running\":true") {
        Ok(())
    } else {
        Err(EngineError::Message(resp))
    }
}

pub fn try_connect(port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], port))),
        Duration::from_millis(400),
    )
    .is_ok()
}

pub fn http_post(port: u16, path: &str, body: &[u8], tok: Option<&str>) -> Result<String, EngineError> {
    let mut s = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().map_err(|e| EngineError::Message(format!("{e}")))?, Duration::from_secs(2))?;
    let _ = s.set_read_timeout(Some(IO_TIMEOUT));
    let auth = tok.map(|t| format!("{TOKEN_HEADER}: {t}\r\n")).unwrap_or_default();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(req.as_bytes())?;
    s.write_all(body)?;
    s.flush()?;
    read_http_body(s)
}

fn read_http_body(s: TcpStream) -> Result<String, EngineError> {
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
            len = v.trim().parse().unwrap_or(0).min(MAX_CONTROL_BODY);
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
    use encodecraft_job::{JobSource, ScreenSize, TOKEN_HEADER};
    use std::io::{Read, Write};

    fn boot() -> (Handle, IpcServer, String, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::paths::lock_home();
        let home = std::env::temp_dir().join(format!(
            "encodecraft-ipc-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        crate::paths::set_home(home);
        let h = crate::Handle::open_core().unwrap();
        let tok = h.lock(|s| s.ipc_token.clone());
        let srv = IpcServer::bind(0, h.clone()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(80));
        (h, srv, tok, guard)
    }

    fn raw(port: u16, req: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut b = String::new();
        s.read_to_string(&mut b).unwrap();
        b
    }

    #[test]
    fn http_enqueue_and_health() {
        let (h, srv, _, _guard) = boot();
        assert!(try_connect(srv.port));
        let health = raw(srv.port, "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        assert!(health.contains("EncodeCraft") || health.contains("ok"));
        assert!(!health.to_ascii_lowercase().contains("access-control-allow-origin"));
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

    #[test]
    fn rejects_missing_token_and_bad_host_and_origin() {
        let (_h, srv, tok, _guard) = boot();
        let body = r#"{"schema":1,"source":{"kind":"file","path":"/tmp/a.mp4"},"preset_id":"system.h264-mp4"}"#;
        let no_tok = raw(
            srv.port,
            &format!(
                "POST /v1/enqueue HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(no_tok.contains("401") || no_tok.contains("unauthorized"));
        let bad_host = raw(
            srv.port,
            &format!(
                "POST /v1/enqueue HTTP/1.1\r\nHost: evil.example\r\nContent-Type: application/json\r\n{TOKEN_HEADER}: {tok}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(bad_host.contains("403") || bad_host.contains("bad host"));
        let csrf = raw(
            srv.port,
            &format!(
                "POST /v1/enqueue HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\nContent-Type: application/json\r\n{TOKEN_HEADER}: {tok}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(csrf.contains("403") || csrf.contains("origin"));
        let rebind = raw(
            srv.port,
            &format!(
                "POST /v1/enqueue HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://attacker.test\r\nSec-Fetch-Site: cross-site\r\nContent-Type: application/json\r\n{TOKEN_HEADER}: {tok}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(rebind.contains("403"));
    }

    #[test]
    fn rejects_future_schema_over_http() {
        let (_h, srv, tok, _guard) = boot();
        let body = r#"{"schema":99,"source":{"kind":"file","path":"/tmp/a.mp4"},"preset_id":"system.h264-mp4"}"#;
        let resp = raw(
            srv.port,
            &format!(
                "POST /v1/enqueue HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{TOKEN_HEADER}: {tok}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(resp.contains("schema") || resp.contains("400"));
        assert!(!resp.contains("invalid type"));
    }

    #[test]
    fn accepts_effectcraft_schema_string_over_http_and_logs_failures() {
        let (h, srv, tok, _guard) = boot();
        let body = r#"{"schema":"encodecraft.job/v1","source_version":1,"source":{"kind":"effectcraft","project":"C:\\work\\spot.ecproj","comp":"Main"},"preset_id":"system.h264-mp4","start_queue":true}"#;
        let resp = raw(
            srv.port,
            &format!(
                "POST /v1/enqueue HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{TOKEN_HEADER}: {tok}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(resp.contains("200") || resp.contains("\"ok\":true"), "{resp}");
        assert!(resp.contains("1 job added · Start") || resp.contains("added · Start"), "{resp}");
        h.lock(|s| {
            assert!(!s.queue.running);
            assert_eq!(s.queue.jobs.len(), 1);
            assert_eq!(s.queue.jobs[0].status, encodecraft_queue::JobStatus::Waiting);
            let name = s.queue.jobs[0].output.file_name().unwrap().to_string_lossy().into_owned();
            assert_eq!(name, "Main.mp4");
        });

        let bad = r#"{"schema":"encodecraft.job/v9","source":{"kind":"file","path":"a.mp4"},"preset_id":"x"}"#;
        let resp = raw(
            srv.port,
            &format!(
                "POST /v1/enqueue HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{TOKEN_HEADER}: {tok}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bad}",
                bad.len()
            ),
        );
        assert!(resp.contains("400"));
        assert!(!resp.contains("invalid type"));
        assert!(resp.contains("encodecraft.job/v9") || resp.contains("unknown job schema"));
        let log = h.lock(|s| s.log.join("\n"));
        assert!(log.contains("HTTP 400"), "{log}");
        assert!(log.contains("schema") || log.contains("encodecraft.job"), "{log}");
    }

    #[test]
    fn http_control_expect_100_continue_then_content_length_body() {
        let (_h, srv, tok, _guard) = boot();
        let body = r#"{"id":7,"method":"queue.list"}"#;
        let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
        let head = format!(
            "POST /v1/control HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{TOKEN_HEADER}: {tok}\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut r = BufReader::new(s);
        let mut status = String::new();
        r.read_line(&mut status).unwrap();
        assert!(status.contains("100"), "expected 100 Continue, got {status}");
        loop {
            let mut h = String::new();
            r.read_line(&mut h).unwrap();
            if h == "\r\n" || h == "\n" || h.is_empty() {
                break;
            }
        }
        {
            let stream = r.get_mut();
            stream.write_all(body.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
        let mut rest = String::new();
        r.read_to_string(&mut rest).unwrap();
        assert!(!rest.contains("truncated body"), "{rest}");
        assert!(rest.contains("200") || rest.contains("\"ok\":true"), "{rest}");
    }

    #[test]
    fn http_control_reads_content_length_across_partial_writes() {
        let (_h, srv, tok, _guard) = boot();
        let body = r#"{"id":8,"method":"queue.list"}"#;
        let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
        let head = format!(
            "POST /v1/control HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{TOKEN_HEADER}: {tok}\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut r = BufReader::new(s);
        let mut status = String::new();
        r.read_line(&mut status).unwrap();
        assert!(status.contains("100"), "{status}");
        loop {
            let mut h = String::new();
            r.read_line(&mut h).unwrap();
            if h == "\r\n" || h == "\n" || h.is_empty() {
                break;
            }
        }
        let bytes = body.as_bytes();
        let mid = bytes.len() / 2;
        {
            let stream = r.get_mut();
            stream.write_all(&bytes[..mid]).unwrap();
            stream.flush().unwrap();
        }
        std::thread::sleep(Duration::from_millis(40));
        {
            let stream = r.get_mut();
            stream.write_all(&bytes[mid..]).unwrap();
            stream.flush().unwrap();
        }
        let mut rest = String::new();
        r.read_to_string(&mut rest).unwrap();
        assert!(!rest.contains("truncated body"), "{rest}");
        assert!(rest.contains("200") || rest.contains("\"ok\":true"), "{rest}");
    }
}
