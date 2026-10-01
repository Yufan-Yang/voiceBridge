//! Registry of pinned targets (slots 1–9) and the current selection.

use std::sync::{Arc, Mutex};

use super::matcher;
use super::model::*;
use crate::config::{ConfigStore, SLOT_COUNT, TARGETS_KEY};
use crate::error::{AppError, ErrorCode, Result};
use crate::platform::{WindowInfo, WindowManager};
use crate::types::OutputKind;

#[derive(Default)]
struct Inner {
    targets: Vec<TargetSlot>,
    selected: Option<String>,
    suggestions: Vec<RebindSuggestion>,
}

pub struct TargetRegistry {
    wm: Arc<dyn WindowManager>,
    store: Arc<dyn ConfigStore>,
    inner: Mutex<Inner>,
    /// Target ids in the order they were last spoken to (oldest first).
    recent: Mutex<Vec<String>>,
}

fn not_found() -> AppError {
    AppError::new(ErrorCode::TargetNotFound).with_details("no target with that id")
}

impl TargetRegistry {
    pub fn new(wm: Arc<dyn WindowManager>, store: Arc<dyn ConfigStore>) -> Self {
        Self {
            wm,
            store,
            inner: Mutex::new(Inner::default()),
            recent: Mutex::new(Vec::new()),
        }
    }

    /// Restores pinned targets from the previous run. They start offline and
    /// are only marked online again when the exact same window still exists;
    /// anything else needs an explicit rebind.
    pub fn load(wm: Arc<dyn WindowManager>, store: Arc<dyn ConfigStore>) -> Self {
        let reg = Self::new(wm, store);
        if let Ok(Some(json)) = reg.store.load(TARGETS_KEY) {
            if let Ok(mut targets) = serde_json::from_str::<Vec<TargetSlot>>(&json) {
                targets.retain(|t| (1..=SLOT_COUNT as u8).contains(&t.slot));
                targets.sort_by_key(|t| t.slot);
                targets.dedup_by_key(|t| t.slot);
                for t in &mut targets {
                    t.status = TargetStatus::Offline;
                }
                let mut g = reg.inner.lock().unwrap();
                g.selected = targets.first().map(|t| t.id.clone());
                g.targets = targets;
            }
        }
        reg.refresh_status();
        reg
    }

    fn persist(&self, targets: &[TargetSlot]) {
        if let Ok(json) = serde_json::to_string_pretty(targets) {
            let _ = self.store.save(TARGETS_KEY, &json);
        }
    }

    pub fn snapshot(&self) -> TargetsSnapshot {
        let g = self.inner.lock().unwrap();
        TargetsSnapshot {
            targets: g.targets.clone(),
            selected_id: g.selected.clone(),
            suggestions: g.suggestions.clone(),
        }
    }

    pub fn list(&self) -> Vec<TargetSlot> {
        self.inner.lock().unwrap().targets.clone()
    }

    pub fn get(&self, id: &str) -> Option<TargetSlot> {
        self.inner
            .lock()
            .unwrap()
            .targets
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }

    pub fn selected_id(&self) -> Option<String> {
        self.inner.lock().unwrap().selected.clone()
    }

    /// Pins `window` to `slot`, replacing whatever was in that slot. A window
    /// can only occupy one slot.
    pub fn bind_window(
        &self,
        window: &WindowInfo,
        slot: u8,
        default_output: OutputKind,
        auto_submit: bool,
    ) -> Result<TargetSlot> {
        if !(1..=SLOT_COUNT as u8).contains(&slot) {
            return Err(AppError::new(ErrorCode::InvalidInput).with_details("slot must be 1-9"));
        }
        let mut target = TargetSlot::from_window(window, slot, default_output);
        target.auto_submit = auto_submit;
        let mut g = self.inner.lock().unwrap();
        let replaced_selected = g.targets.iter().any(|t| {
            Some(&t.id) == g.selected.as_ref()
                && (t.slot == slot || t.platform_window_id == window.platform_window_id)
        });
        g.targets
            .retain(|t| t.slot != slot && t.platform_window_id != window.platform_window_id);
        g.targets.push(target.clone());
        g.targets.sort_by_key(|t| t.slot);
        if g.selected.is_none() || replaced_selected {
            g.selected = Some(target.id.clone());
        }
        g.suggestions.clear();
        self.persist(&g.targets);
        Ok(target)
    }

    /// Pins the current foreground window to `slot`.
    pub fn bind_active(
        &self,
        slot: u8,
        default_output: OutputKind,
        auto_submit: bool,
    ) -> Result<TargetSlot> {
        let window = self.wm.foreground_window()?.ok_or_else(|| {
            AppError::new(ErrorCode::TargetNotFound).with_details("no foreground window")
        })?;
        if window.process_id == std::process::id() {
            return Err(AppError::new(ErrorCode::InvalidInput)
                .with_message("Focus the coding window you want to pin first.")
                .with_details("foreground window belongs to VoiceBridge"));
        }
        self.bind_window(&window, slot, default_output, auto_submit)
    }

    pub fn bind_window_id(
        &self,
        platform_window_id: &str,
        slot: u8,
        default_output: OutputKind,
        auto_submit: bool,
    ) -> Result<TargetSlot> {
        let window = self.wm.window_info(platform_window_id)?.ok_or_else(|| {
            AppError::new(ErrorCode::TargetNotFound).with_details("window closed")
        })?;
        self.bind_window(&window, slot, default_output, auto_submit)
    }

    pub fn unbind(&self, id: &str) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        let before = g.targets.len();
        g.targets.retain(|t| t.id != id);
        if g.targets.len() == before {
            return Err(not_found());
        }
        g.suggestions.retain(|s| s.target_id != id);
        if g.selected.as_deref() == Some(id) {
            g.selected = g.targets.first().map(|t| t.id.clone());
        }
        self.persist(&g.targets);
        Ok(())
    }

    fn update<F: FnOnce(&mut TargetSlot)>(&self, id: &str, f: F) -> Result<TargetSlot> {
        let mut g = self.inner.lock().unwrap();
        let t = g
            .targets
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(not_found)?;
        f(t);
        let out = t.clone();
        self.persist(&g.targets);
        Ok(out)
    }

    pub fn rename(&self, id: &str, alias: &str) -> Result<TargetSlot> {
        let alias: String = alias.trim().chars().take(32).collect();
        if alias.is_empty() {
            return Err(AppError::new(ErrorCode::InvalidInput).with_details("empty alias"));
        }
        self.update(id, |t| t.alias = alias)
    }

    pub fn set_project_root(&self, id: &str, root: Option<String>) -> Result<TargetSlot> {
        let root = root.filter(|r| !r.trim().is_empty());
        let name = root.as_ref().and_then(|r| {
            std::path::Path::new(r)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        });
        self.update(id, |t| {
            t.project_root = root;
            t.project_name = name;
        })
    }

    pub fn set_output(&self, id: &str, output: OutputKind) -> Result<TargetSlot> {
        self.update(id, |t| t.preferred_output = output)
    }

    pub fn set_auto_submit(&self, id: &str, auto_submit: bool) -> Result<TargetSlot> {
        self.update(id, |t| t.auto_submit = auto_submit)
    }

    pub fn set_manual_terms(&self, id: &str, terms: Vec<String>) -> Result<TargetSlot> {
        let terms: Vec<String> = terms
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .take(200)
            .collect();
        self.update(id, |t| t.manual_terms = terms)
    }

    pub fn select(&self, id: &str) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if !g.targets.iter().any(|t| t.id == id) {
            return Err(not_found());
        }
        g.selected = Some(id.to_string());
        Ok(())
    }

    pub fn select_slot(&self, slot: u8) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        let id = g
            .targets
            .iter()
            .find(|t| t.slot == slot)
            .map(|t| t.id.clone())
            .ok_or_else(|| {
                AppError::new(ErrorCode::TargetNotFound)
                    .with_message(format!("No window is pinned to slot {slot}."))
            })?;
        g.selected = Some(id);
        Ok(())
    }

    /// Selects the next (`step = 1`) or previous (`step = -1`) target by slot.
    pub fn select_relative(&self, step: i32) -> Option<String> {
        let mut g = self.inner.lock().unwrap();
        if g.targets.is_empty() {
            return None;
        }
        let n = g.targets.len() as i32;
        let current = g
            .selected
            .as_ref()
            .and_then(|id| g.targets.iter().position(|t| &t.id == id))
            .map(|i| i as i32);
        let next = match current {
            Some(i) => (i + step).rem_euclid(n),
            None => 0,
        };
        let id = g.targets[next as usize].id.clone();
        g.selected = Some(id.clone());
        Some(id)
    }

    /// Re-checks which targets still exist. Returns true when anything changed.
    /// Offline targets get a rebind *suggestion*; nothing is rebound here.
    pub fn refresh_status(&self) -> bool {
        let targets = self.list();
        if targets.is_empty() {
            return false;
        }
        let mut statuses = Vec::with_capacity(targets.len());
        for t in &targets {
            let online = matches!(
                self.wm.window_info(&t.platform_window_id),
                Ok(Some(ref w)) if matcher::identity_matches(t, w)
            );
            statuses.push(online);
        }
        let mut suggestions = Vec::new();
        if statuses.iter().any(|online| !online) {
            if let Ok(windows) = self.wm.list_windows() {
                let mut taken: Vec<String> = targets
                    .iter()
                    .zip(&statuses)
                    .filter(|(_, online)| **online)
                    .map(|(t, _)| t.platform_window_id.clone())
                    .collect();
                for (t, online) in targets.iter().zip(&statuses) {
                    if *online {
                        continue;
                    }
                    if let Some(w) = matcher::suggest(t, &windows, &taken) {
                        taken.push(w.platform_window_id.clone());
                        suggestions.push(RebindSuggestion {
                            target_id: t.id.clone(),
                            window: w.clone(),
                        });
                    }
                }
            }
        }

        let mut g = self.inner.lock().unwrap();
        let mut changed = g.suggestions != suggestions;
        g.suggestions = suggestions;
        for (t, online) in targets.iter().zip(&statuses) {
            let status = if *online {
                TargetStatus::Online
            } else {
                TargetStatus::Offline
            };
            if let Some(cur) = g.targets.iter_mut().find(|c| c.id == t.id) {
                if cur.status != status {
                    cur.status = status;
                    changed = true;
                }
            }
        }
        changed
    }

    /// Applies a rebind the user explicitly confirmed.
    pub fn confirm_rebind(&self, target_id: &str, platform_window_id: &str) -> Result<TargetSlot> {
        let window = self.wm.window_info(platform_window_id)?.ok_or_else(|| {
            AppError::new(ErrorCode::TargetNotFound).with_details("window closed")
        })?;
        let mut g = self.inner.lock().unwrap();
        if g.targets
            .iter()
            .any(|t| t.id != target_id && t.platform_window_id == window.platform_window_id)
        {
            return Err(AppError::new(ErrorCode::InvalidInput)
                .with_message("That window is already pinned to another slot."));
        }
        let t = g
            .targets
            .iter_mut()
            .find(|t| t.id == target_id)
            .ok_or_else(not_found)?;
        t.platform_window_id = window.platform_window_id.clone();
        t.process_id = window.process_id;
        t.executable_or_bundle_id = window.executable_or_bundle_id.clone();
        t.title_hint = window.title.clone();
        t.status = TargetStatus::Online;
        let out = t.clone();
        g.suggestions.retain(|s| s.target_id != target_id);
        self.persist(&g.targets);
        Ok(out)
    }
}

impl TargetRegistry {
    /// Makes the current foreground window the selected target, so the next
    /// utterance goes to wherever the user is typing.
    ///
    /// - A window that is already a target is simply selected (its alias,
    ///   project and output settings are kept).
    /// - A new window takes a free slot; when all nine are used it replaces
    ///   the target that was spoken to longest ago.
    /// - When the foreground window is VoiceBridge itself, or there is none,
    ///   nothing changes and the previous target keeps being used.
    ///
    /// Returns the target that is now selected because of this call.
    pub fn track_foreground(
        &self,
        default_output: OutputKind,
        auto_submit: bool,
    ) -> Option<TargetSlot> {
        let window = self.wm.foreground_window().ok().flatten()?;
        if window.process_id == std::process::id() {
            return None;
        }
        let existing = self
            .list()
            .into_iter()
            .find(|t| matcher::identity_matches(t, &window));
        let target = match existing {
            Some(t) => {
                self.select(&t.id).ok()?;
                // Keep the title hint fresh; it is only a hint.
                self.update(&t.id, |t| {
                    t.title_hint = window.title.clone();
                    t.status = TargetStatus::Online;
                })
                .ok()?
            }
            None => {
                let slot = self.slot_for_new_target();
                let t = self
                    .bind_window(&window, slot, default_output, auto_submit)
                    .ok()?;
                self.select(&t.id).ok()?;
                t
            }
        };
        let mut recent = self.recent.lock().unwrap();
        recent.retain(|id| id != &target.id);
        recent.push(target.id.clone());
        Some(target)
    }

    /// First free slot, otherwise the slot of the least recently used target.
    fn slot_for_new_target(&self) -> u8 {
        let targets = self.list();
        if let Some(free) = (1..=SLOT_COUNT as u8).find(|s| !targets.iter().any(|t| t.slot == *s)) {
            return free;
        }
        let recent = self.recent.lock().unwrap();
        targets
            .iter()
            .min_by_key(|t| {
                recent
                    .iter()
                    .position(|id| id == &t.id)
                    .map_or(0, |i| i + 1)
            })
            .map_or(1, |t| t.slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MemoryStore;
    use crate::platform::mock::MockDesktop;

    fn setup() -> (MockDesktop, TargetRegistry, Arc<MemoryStore>) {
        let desk = MockDesktop::new();
        desk.add_window(MockDesktop::window("1", 10, "/Apps/Claude", "backend"));
        desk.add_window(MockDesktop::window("2", 20, "/Apps/Codex", "web"));
        desk.add_window(MockDesktop::window("3", 30, "/Apps/Cursor", "infra"));
        let store = Arc::new(MemoryStore::default());
        let reg = TargetRegistry::new(Arc::new(desk.clone()), store.clone());
        (desk, reg, store)
    }

    fn bind(reg: &TargetRegistry, window: &str, slot: u8) -> TargetSlot {
        reg.bind_window_id(window, slot, OutputKind::Prompt, false)
            .unwrap()
    }

    #[test]
    fn pins_three_distinct_windows_and_selects() {
        let (_d, reg, _s) = setup();
        let a = bind(&reg, "1", 1);
        let b = bind(&reg, "2", 2);
        let c = bind(&reg, "3", 3);
        assert_eq!(reg.list().len(), 3);
        assert_eq!(reg.selected_id(), Some(a.id.clone()));
        reg.select(&b.id).unwrap();
        assert_eq!(reg.selected_id(), Some(b.id.clone()));
        reg.select_slot(3).unwrap();
        assert_eq!(reg.selected_id(), Some(c.id.clone()));
        assert_eq!(reg.select_relative(1), Some(a.id.clone()));
        assert_eq!(reg.select_relative(-1), Some(c.id));
        assert!(reg.select_slot(7).is_err());
    }

    #[test]
    fn auto_submit_defaults_to_false() {
        let (_d, reg, _s) = setup();
        let t = bind(&reg, "1", 1);
        assert!(!t.auto_submit);
        let json = r#"{"id":"x","slot":1,"alias":"a","platform_window_id":"1","process_id":1,
            "executable_or_bundle_id":"e","title_hint":"t","project_root":null,"project_name":null}"#;
        let parsed: TargetSlot = serde_json::from_str(json).unwrap();
        assert!(!parsed.auto_submit);
        assert!(!crate::config::Settings::default().behavior.auto_submit);
    }

    #[test]
    fn foreground_binding_slot_limits_and_replacement() {
        let (desk, reg, _s) = setup();
        assert!(reg.bind_active(1, OutputKind::Prompt, false).is_err());
        desk.set_foreground("2");
        let t = reg.bind_active(4, OutputKind::Raw, false).unwrap();
        assert_eq!((t.slot, t.platform_window_id.as_str()), (4, "2"));
        assert!(reg
            .bind_window_id("1", 0, OutputKind::Prompt, false)
            .is_err());
        assert!(reg
            .bind_window_id("1", 10, OutputKind::Prompt, false)
            .is_err());
        // Re-pinning the same window moves it instead of duplicating it.
        bind(&reg, "2", 5);
        assert_eq!(reg.list().len(), 1);
        assert_eq!(reg.list()[0].slot, 5);
    }

    #[test]
    fn rename_unbind_and_settings() {
        let (_d, reg, _s) = setup();
        let t = bind(&reg, "1", 1);
        assert_eq!(reg.rename(&t.id, "  Backend ").unwrap().alias, "Backend");
        assert!(reg.rename(&t.id, "  ").is_err());
        let t2 = reg
            .set_project_root(&t.id, Some("/work/api-server".into()))
            .unwrap();
        assert_eq!(t2.project_name.as_deref(), Some("api-server"));
        assert_eq!(
            reg.set_output(&t.id, OutputKind::Raw)
                .unwrap()
                .preferred_output,
            OutputKind::Raw
        );
        reg.unbind(&t.id).unwrap();
        assert!(reg.list().is_empty());
        assert_eq!(reg.selected_id(), None);
    }

    #[test]
    fn closed_window_goes_offline_and_is_never_rebound_automatically() {
        let (desk, reg, _s) = setup();
        let t = bind(&reg, "1", 1);
        desk.remove_window("1");
        // A new window of the same app with the same title appears.
        desk.add_window(MockDesktop::window("99", 11, "/Apps/Claude", "backend"));
        assert!(reg.refresh_status());
        let snap = reg.snapshot();
        assert_eq!(snap.targets[0].status, TargetStatus::Offline);
        assert_eq!(
            snap.targets[0].platform_window_id, "1",
            "must not auto-rebind"
        );
        assert_eq!(snap.suggestions.len(), 1);
        assert_eq!(snap.suggestions[0].window.platform_window_id, "99");

        // Only an explicit confirmation rebinds.
        let rebound = reg.confirm_rebind(&t.id, "99").unwrap();
        assert_eq!(rebound.status, TargetStatus::Online);
        assert_eq!(rebound.process_id, 11);
        assert!(reg.snapshot().suggestions.is_empty());
    }

    #[test]
    fn targets_restored_after_restart_need_matching_window() {
        let (desk, reg, store) = setup();
        bind(&reg, "1", 1);
        bind(&reg, "2", 2);
        desk.remove_window("2");
        let restored = TargetRegistry::load(Arc::new(desk.clone()), store);
        let list = restored.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].status, TargetStatus::Online);
        assert_eq!(list[1].status, TargetStatus::Offline);
    }

    #[test]
    fn tracking_follows_the_foreground_window() {
        let (desk, reg, _s) = setup();
        // Nothing in front: nothing is tracked.
        assert!(reg.track_foreground(OutputKind::Prompt, false).is_none());
        assert!(reg.list().is_empty());

        desk.set_foreground("2");
        let codex = reg.track_foreground(OutputKind::Prompt, false).unwrap();
        assert_eq!(codex.platform_window_id, "2");
        assert_eq!(reg.selected_id(), Some(codex.id.clone()));
        assert!(!codex.auto_submit);

        // Speaking in another window switches to it and keeps the first one.
        desk.set_foreground("3");
        let cursor = reg.track_foreground(OutputKind::Prompt, false).unwrap();
        assert_eq!(reg.selected_id(), Some(cursor.id.clone()));
        assert_eq!(reg.list().len(), 2);

        // Returning to a known window reuses its target and its settings.
        reg.rename(&codex.id, "Web").unwrap();
        desk.set_foreground("2");
        let again = reg.track_foreground(OutputKind::Raw, true).unwrap();
        assert_eq!(again.id, codex.id);
        assert_eq!(again.alias, "Web");
        assert_eq!(again.preferred_output, OutputKind::Prompt);
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn tracking_ignores_own_windows_and_recycles_the_oldest_slot() {
        let (desk, reg, _s) = setup();
        desk.add_window(MockDesktop::window(
            "own",
            std::process::id(),
            "/Apps/VoiceBridge",
            "Settings",
        ));
        desk.set_foreground("1");
        let first = reg.track_foreground(OutputKind::Prompt, false).unwrap();
        desk.set_foreground("own");
        assert!(reg.track_foreground(OutputKind::Prompt, false).is_none());
        assert_eq!(
            reg.selected_id(),
            Some(first.id.clone()),
            "previous target keeps being used"
        );

        // Fill all nine slots, then speak in a tenth window.
        for i in 0..8 {
            let id = format!("w{i}");
            desk.add_window(MockDesktop::window(&id, 100 + i, "/Apps/Term", &id));
            desk.set_foreground(&id);
            reg.track_foreground(OutputKind::Prompt, false).unwrap();
        }
        assert_eq!(reg.list().len(), 9);
        desk.add_window(MockDesktop::window("tenth", 500, "/Apps/Term", "tenth"));
        desk.set_foreground("tenth");
        let tenth = reg.track_foreground(OutputKind::Prompt, false).unwrap();
        assert_eq!(reg.list().len(), 9);
        assert_eq!(
            tenth.slot, first.slot,
            "the least recently used target is replaced"
        );
        assert!(reg.get(&first.id).is_none());
    }
}
