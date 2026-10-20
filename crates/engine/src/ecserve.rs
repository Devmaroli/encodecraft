//! Warm `effectcraft-cli serve` process. One render at a time; EncodeCraft sends `app.quit` on exit.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use encodecraft_ffmpeg::{hidden_command, parse_stream_event, serve_args, StreamEvent};
use serde_json::{json, Value};

use crate::EngineError;

const IDLE_EXIT_SECS: u64 = 120;

pub struct WarmServe {
    child: Mutex<Child>,
    pub port: u16,
    pub gpu_requested: bool,
    #[allow(dead_code)]
    pub gpu: bool,
    next_id: AtomicU64,
    pub rendering: AtomicBool,
}

impl WarmServe {
    pub fn spawn(cli: &Path, gpu: bool) -> Result<Self, EngineError> {
        let args = serve_args(cli, gpu, IDLE_EXIT_SECS);
        let (prog, rest) = args.split_first().ok_or_else(|| EngineError::Message("empty serve argv".into()))?;
        let mut child = hidden_command(prog).args(rest).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
        let stderr = child.stderr.take().ok_or_else(|| EngineError::Message("serve has no stderr".into()))?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut port = 0u16;
        let mut gpu_on = gpu;
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        while Instant::now() < deadline {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if let Some(StreamEvent::Listening { port: p, gpu: g, .. }) = parse_stream_event(line.trim()) {
                        port = p;
                        gpu_on = g;
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        std::thread::spawn(move || for _ in reader.lines() {});
        if port == 0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(EngineError::Message("effectcraft-cli serve did not print a listening port".into()));
        }
        Ok(Self { child: Mutex::new(child), port, gpu_requested: gpu, gpu: gpu_on, next_id: AtomicU64::new(1), rendering: AtomicBool::new(false) })
    }

    pub fn alive(&self) -> bool {
        let mut g = match self.child.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        matches!(g.try_wait(), Ok(None))
    }

    pub fn rpc(&self, method: &str, params: Value) -> Result<Value, EngineError> {
        rpc_at(self.port, &self.next_id, method, params, Duration::from_secs(30))
    }

    pub fn render_start(&self, params: Value, mut on_event: impl FnMut(StreamEvent)) -> Result<Value, EngineError> {
        self.rendering.store(true, Ordering::SeqCst);
        let result = render_start_at(self.port, &self.next_id, params, &mut on_event);
        self.rendering.store(false, Ordering::SeqCst);
        result
    }

    pub fn cancel(&self) {
        let _ = self.rpc("render.cancel", json!({}));
    }

    pub fn quit(&self) {
        let _ = self.rpc("app.quit", json!({}));
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn rpc_at(port: u16, next_id: &AtomicU64, method: &str, params: Value, read_timeout: Duration) -> Result<Value, EngineError> {
    let id = next_id.fetch_add(1, Ordering::Relaxed);
    let req = json!({"id": id, "method": method, "params": params});
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| EngineError::Message(format!("serve connect: {e}")))?;
    stream.set_read_timeout(Some(read_timeout)).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let line = serde_json::to_string(&req)?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            return Err(EngineError::Message("serve closed the control connection".into()));
        }
        let v: Value = serde_json::from_str(buf.trim()).map_err(|e| EngineError::Message(format!("serve json: {e}")))?;
        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
            if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("serve error");
            return Err(EngineError::Message(err.to_string()));
        }
    }
}

fn render_start_at(port: u16, next_id: &AtomicU64, params: Value, on_event: &mut dyn FnMut(StreamEvent)) -> Result<Value, EngineError> {
    let id = next_id.fetch_add(1, Ordering::Relaxed);
    let req = json!({"id": id, "method": "render.start", "params": params});
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| EngineError::Message(format!("serve connect: {e}")))?;
    stream.set_read_timeout(Some(Duration::from_secs(300))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let line = serde_json::to_string(&req)?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            return Err(EngineError::Message("serve closed during render.start".into()));
        }
        let trim = buf.trim();
        let v: Value = serde_json::from_str(trim).map_err(|e| EngineError::Message(format!("serve json: {e}")))?;
        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
            if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("render.start failed");
            return Err(EngineError::Message(err.to_string()));
        }
        if let Some(ev) = parse_stream_event(trim) {
            on_event(ev);
        }
    }
}

#[derive(Clone)]
pub struct ServePool {
    inner: Arc<Mutex<Option<Arc<WarmServe>>>>,
}

impl ServePool {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(None)) }
    }

    pub fn ensure(&self, cli: &Path, gpu: bool) -> Result<Arc<WarmServe>, EngineError> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = g.as_ref() {
            if s.alive() && s.gpu_requested == gpu {
                return Ok(Arc::clone(s));
            }
            if let Some(old) = g.take() {
                old.quit();
            }
        }
        let spawned = Arc::new(WarmServe::spawn(cli, gpu)?);
        *g = Some(Arc::clone(&spawned));
        Ok(spawned)
    }

    pub fn get(&self) -> Option<Arc<WarmServe>> {
        self.inner.lock().ok().and_then(|g| g.clone())
    }

    pub fn rendering(&self) -> bool {
        self.get().is_some_and(|s| s.rendering.load(Ordering::SeqCst))
    }

    pub fn quit(&self) {
        if let Ok(mut g) = self.inner.lock() {
            if let Some(s) = g.take() {
                s.quit();
            }
        }
    }
}

impl Default for ServePool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_payload_has_id_and_method() {
        let v = json!({"id": 1, "method": "project.open", "params": {"path": "/p.ecproj"}});
        assert_eq!(v["method"], "project.open");
        assert_eq!(v["params"]["path"], "/p.ecproj");
    }
}
