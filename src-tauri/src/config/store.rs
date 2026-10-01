//! Storage abstraction. The MVP uses one JSON document per key; the trait is
//! deliberately key/value shaped so a SQLite-backed store can replace it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{AppError, ErrorCode, Result};

pub trait ConfigStore: Send + Sync {
    fn load(&self, key: &str) -> Result<Option<String>>;
    fn save(&self, key: &str, value: &str) -> Result<()>;
    fn remove(&self, key: &str) -> Result<()>;
}

pub struct JsonFileStore {
    dir: PathBuf,
}

impl JsonFileStore {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self { dir }
    }

    fn path(&self, key: &str) -> PathBuf {
        let safe: String = key
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        self.dir.join(format!("{safe}.json"))
    }
}

fn io_err(e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::ConfigFailed).with_details(e.kind().to_string())
}

impl ConfigStore for JsonFileStore {
    fn load(&self, key: &str) -> Result<Option<String>> {
        match std::fs::read_to_string(self.path(key)) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(e)),
        }
    }

    fn save(&self, key: &str, value: &str) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(io_err)?;
        let path = self.path(key);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, value).map_err(io_err)?;
        restrict_permissions(&tmp);
        std::fs::rename(&tmp, &path).map_err(io_err)
    }

    fn remove(&self, key: &str) -> Result<()> {
        match std::fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }
}

/// Owner read/write only.
pub fn restrict_permissions(path: &std::path::Path) {
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

#[derive(Default)]
pub struct MemoryStore {
    data: Mutex<HashMap<String, String>>,
}

impl ConfigStore for MemoryStore {
    fn load(&self, key: &str) -> Result<Option<String>> {
        Ok(self.data.lock().unwrap().get(key).cloned())
    }

    fn save(&self, key: &str, value: &str) -> Result<()> {
        self.data
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<()> {
        self.data.lock().unwrap().remove(key);
        Ok(())
    }
}
