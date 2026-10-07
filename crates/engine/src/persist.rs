use std::path::{Path, PathBuf};

use encodecraft_job::DEFAULT_CONTROL_PORT;
use encodecraft_presets::{default_output_dir, default_template};
use encodecraft_queue::EncodeQueue;
use serde::{Deserialize, Serialize};

use crate::watch::WatchFolder;
use crate::EngineError;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    #[serde(default = "default_naming")]
    pub naming: String,
    #[serde(default = "default_preset")]
    pub default_preset_id: String,
    #[serde(default)]
    pub ffmpeg_path: Option<PathBuf>,
    #[serde(default)]
    pub effectcraft_cli: Option<PathBuf>,
    #[serde(default = "default_port")]
    pub http_port: u16,
    #[serde(default)]
    pub auto_start_queue: bool,
    #[serde(default)]
    pub watch_folders: Vec<WatchFolder>,
    /// When false (default), existing output files get a `-2` suffix instead of being replaced.
    #[serde(default)]
    pub allow_overwrite: bool,
    /// Kill FFmpeg if it prints nothing for this many seconds.
    #[serde(default = "default_stall")]
    pub stall_timeout_sec: u64,
}

fn default_parallel() -> usize {
    2
}
fn default_naming() -> String {
    default_template().into()
}
fn default_preset() -> String {
    "system.h264-mp4".into()
}
fn default_port() -> u16 {
    DEFAULT_CONTROL_PORT
}
fn default_stall() -> u64 {
    120
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_parallel: 2,
            output_dir: default_output_dir(),
            naming: default_naming(),
            default_preset_id: default_preset(),
            ffmpeg_path: None,
            effectcraft_cli: None,
            http_port: DEFAULT_CONTROL_PORT,
            auto_start_queue: false,
            watch_folders: Vec::new(),
            allow_overwrite: false,
            stall_timeout_sec: 120,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        crate::fsutil::load_json_or_quarantine(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), EngineError> {
        crate::fsutil::atomic_write(path, &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

pub fn load_queue(path: &Path) -> EncodeQueue {
    match crate::fsutil::load_json_or_quarantine::<EncodeQueue>(path) {
        Some(mut q) => {
            for j in &mut q.jobs {
                if j.status.is_active() {
                    j.status = encodecraft_queue::JobStatus::Waiting;
                }
            }
            q
        }
        None => EncodeQueue::new(),
    }
}

pub fn save_queue(path: &Path, q: &EncodeQueue) -> Result<(), EngineError> {
    crate::fsutil::atomic_write(path, &serde_json::to_vec_pretty(&q.persist_view())?)?;
    Ok(())
}
