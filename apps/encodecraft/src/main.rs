//! EncodeCraft desktop app.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use encodecraft_engine::{write_lock, Handle};
use encodecraft_job::PORT_ENV;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if let Err(e) = run() {
        eprintln!("EncodeCraft: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let handle = Handle::open()?;
    let port = std::env::var(PORT_ENV).ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| handle.lock(|s| s.settings.http_port));
    match handle.start_ipc(port) {
        Ok(srv) => {
            let _ = write_lock(srv.port);
            log::info!("control server on 127.0.0.1:{}", srv.port);
        }
        Err(e) => log::warn!("control server not started: {e} (another EncodeCraft may already be listening)"),
    }
    encodecraft_ui_egui::run(handle)?;
    encodecraft_engine::clear_lock();
    Ok(())
}
