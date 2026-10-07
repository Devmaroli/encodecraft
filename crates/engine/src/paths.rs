use std::path::PathBuf;
use std::sync::Mutex;

use encodecraft_job::{APP_ID, HOME_ENV};

static TEST_HOME: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_home(path: PathBuf) {
    if let Ok(mut g) = TEST_HOME.lock() {
        *g = Some(path);
    }
}

pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var(HOME_ENV) {
        return PathBuf::from(p);
    }
    if let Ok(g) = TEST_HOME.lock() {
        if let Some(p) = g.clone() {
            return p;
        }
    }
    directories::ProjectDirs::from("dev", "EncodeCraft", "EncodeCraft").map(|p| p.data_dir().to_path_buf()).unwrap_or_else(|| std::env::temp_dir().join(APP_ID))
}

pub fn config_dir() -> PathBuf {
    data_dir()
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}
pub fn presets_path() -> PathBuf {
    config_dir().join("presets.json")
}
pub fn queue_path() -> PathBuf {
    config_dir().join("queue.json")
}
pub fn inbox_dir() -> PathBuf {
    data_dir().join("inbox")
}
pub fn work_dir() -> PathBuf {
    data_dir().join("work")
}
pub fn thumbs_dir() -> PathBuf {
    data_dir().join("thumbs")
}
pub fn lock_path() -> PathBuf {
    data_dir().join("encoder.lock")
}

pub fn ensure_dirs() -> std::io::Result<()> {
    for d in [data_dir(), inbox_dir(), work_dir(), thumbs_dir()] {
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}
