use std::path::PathBuf;
use std::sync::Mutex;
#[cfg(test)]
use std::sync::MutexGuard;

use encodecraft_job::{APP_ID, HOME_ENV};

static TEST_HOME: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(test)]
static TEST_HOME_LOCK: Mutex<()> = Mutex::new(());

/// Hold this for the whole test so parallel tests don't share `TEST_HOME`.
#[cfg(test)]
pub fn lock_home() -> MutexGuard<'static, ()> {
    TEST_HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn set_home(path: PathBuf) {
    if let Ok(mut g) = TEST_HOME.lock() {
        *g = Some(path);
    }
}

pub fn data_dir() -> PathBuf {
    if let Ok(g) = TEST_HOME.lock() {
        if let Some(p) = g.clone() {
            return p;
        }
    }
    if let Ok(p) = std::env::var(HOME_ENV) {
        return PathBuf::from(p);
    }
    encodecraft_job::canonical_data_dir().unwrap_or_else(|| std::env::temp_dir().join(APP_ID))
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
