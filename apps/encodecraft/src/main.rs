//! EncodeCraft desktop app.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
// Release builds on Windows are a GUI app: double-click must not open a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use encodecraft_engine::{write_lock, Handle};
use encodecraft_job::PORT_ENV;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if let Err(e) = run() {
        let msg = format!("EncodeCraft could not start:\n{e}");
        eprintln!("{msg}");
        let _ = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("EncodeCraft").set_description(&msg).show();
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
    encodecraft_ui_egui::run(handle.clone())?;
    handle.shutdown();
    encodecraft_engine::clear_lock();
    Ok(())
}
