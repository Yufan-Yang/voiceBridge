pub mod defaults;
pub mod model;
pub mod store;

pub use model::*;
pub use store::{ConfigStore, JsonFileStore, MemoryStore};

use crate::error::{AppError, ErrorCode, Result};

pub const SETTINGS_KEY: &str = "settings";
pub const TARGETS_KEY: &str = "targets";
pub const HISTORY_KEY: &str = "history";

/// Load settings. A missing, unreadable or invalid file yields secure defaults.
pub fn load_settings(store: &dyn ConfigStore) -> Settings {
    match store.load(SETTINGS_KEY) {
        Ok(Some(json)) => Settings::from_json_or_default(&json),
        _ => Settings::default(),
    }
}

pub fn save_settings(store: &dyn ConfigStore, settings: &Settings) -> Result<()> {
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| AppError::new(ErrorCode::ConfigFailed).with_details(e.to_string()))?;
    store.save(SETTINGS_KEY, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_corrupt_file_recovery() {
        let store = MemoryStore::default();
        assert_eq!(load_settings(&store), Settings::default());

        let mut s = Settings::default();
        s.behavior.completion_notice_ms = 2500;
        s.models.prompt_model_path = "/models/q.gguf".into();
        save_settings(&store, &s).unwrap();
        assert_eq!(load_settings(&store), s);

        store.save(SETTINGS_KEY, "{corrupt").unwrap();
        assert_eq!(load_settings(&store), Settings::default());
    }

    #[test]
    fn json_file_store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileStore::new(dir.path().to_path_buf());
        assert!(store.load("settings").unwrap().is_none());
        store.save("settings", "{}").unwrap();
        assert_eq!(store.load("settings").unwrap().as_deref(), Some("{}"));
        store.remove("settings").unwrap();
        assert!(store.load("settings").unwrap().is_none());
    }
}
