//! Utterance history. In memory by default; written to disk only when the
//! user explicitly enables persistent history.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::config::{ConfigStore, HISTORY_KEY};
use crate::types::UtteranceResult;

const CAPACITY: usize = 50;

pub struct HistoryStore {
    items: Mutex<VecDeque<UtteranceResult>>,
    store: Arc<dyn ConfigStore>,
}

impl HistoryStore {
    pub fn new(store: Arc<dyn ConfigStore>) -> Self {
        Self {
            items: Mutex::new(VecDeque::new()),
            store,
        }
    }

    /// Loads persisted history (only called when persistence is enabled).
    pub fn load_persisted(&self) {
        if let Ok(Some(json)) = self.store.load(HISTORY_KEY) {
            if let Ok(items) = serde_json::from_str::<VecDeque<UtteranceResult>>(&json) {
                *self.items.lock().unwrap() = items;
            }
        }
    }

    /// Inserts or updates an utterance. `persist` mirrors the user's
    /// "save history" setting at the time of the call.
    pub fn upsert(&self, result: &UtteranceResult, persist: bool) {
        let mut items = self.items.lock().unwrap();
        if let Some(existing) = items.iter_mut().find(|r| r.id == result.id) {
            *existing = result.clone();
        } else {
            items.push_back(result.clone());
            while items.len() > CAPACITY {
                items.pop_front();
            }
        }
        if persist {
            if let Ok(json) = serde_json::to_string(&*items) {
                let _ = self.store.save(HISTORY_KEY, &json);
            }
        }
    }

    pub fn list(&self) -> Vec<UtteranceResult> {
        self.items.lock().unwrap().iter().cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<UtteranceResult> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }

    /// Clears memory and any persisted copy.
    pub fn clear(&self) {
        self.items.lock().unwrap().clear();
        let _ = self.store.remove(HISTORY_KEY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MemoryStore;
    use crate::types::UtteranceStatus;

    fn result(id: &str) -> UtteranceResult {
        UtteranceResult {
            id: id.into(),
            created_at: "now".into(),
            frozen_target_id: String::new(),
            raw_transcript: "raw".into(),
            normalized_transcript: "Raw.".into(),
            compiled_prompt: "Raw.".into(),
            uncertain_identifiers: vec![],
            needs_confirmation: false,
            status: UtteranceStatus::Compiled,
            audio_ms: 0,
            transcribe_ms: 0,
            compile_ms: 0,
        }
    }

    #[test]
    fn history_stays_in_memory_unless_persistence_is_enabled() {
        let store = Arc::new(MemoryStore::default());
        let h = HistoryStore::new(store.clone());
        h.upsert(&result("a"), false);
        assert_eq!(h.list().len(), 1);
        assert!(
            store.load(HISTORY_KEY).unwrap().is_none(),
            "nothing on disk by default"
        );

        h.upsert(&result("b"), true);
        assert!(store.load(HISTORY_KEY).unwrap().is_some());
        let reloaded = HistoryStore::new(store.clone());
        reloaded.load_persisted();
        assert_eq!(reloaded.list().len(), 2);

        h.clear();
        assert!(h.list().is_empty());
        assert!(store.load(HISTORY_KEY).unwrap().is_none());
    }

    #[test]
    fn capacity_is_bounded_and_updates_replace() {
        let h = HistoryStore::new(Arc::new(MemoryStore::default()));
        for i in 0..(CAPACITY + 10) {
            h.upsert(&result(&i.to_string()), false);
        }
        assert_eq!(h.list().len(), CAPACITY);
        let mut r = result("59");
        r.status = UtteranceStatus::Injected;
        h.upsert(&r, false);
        assert_eq!(h.list().len(), CAPACITY);
        assert_eq!(h.get("59").unwrap().status, UtteranceStatus::Injected);
    }
}
