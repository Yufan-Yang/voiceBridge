//! Window identity checks. Identity never relies on the title alone.

use super::model::TargetSlot;
use crate::platform::WindowInfo;

/// Strict identity: platform window id, process id and application identity
/// must all match the pinned target.
pub fn identity_matches(target: &TargetSlot, window: &WindowInfo) -> bool {
    target.platform_window_id == window.platform_window_id
        && target.process_id == window.process_id
        && target.executable_or_bundle_id == window.executable_or_bundle_id
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .collect()
}

fn title_score(target: &TargetSlot, window: &WindowInfo) -> usize {
    let wanted = tokens(&target.title_hint);
    let have = tokens(&window.title);
    let mut score = wanted.iter().filter(|t| have.contains(t)).count();
    if let Some(name) = &target.project_name {
        if !name.is_empty() && window.title.to_lowercase().contains(&name.to_lowercase()) {
            score += 3;
        }
    }
    score
}

/// Suggests the most likely replacement window for an offline target. The
/// application identity must match exactly; the title only ranks candidates.
/// The result is a suggestion only — the caller must ask the user.
pub fn suggest<'a>(
    target: &TargetSlot,
    windows: &'a [WindowInfo],
    already_bound: &[String],
) -> Option<&'a WindowInfo> {
    windows
        .iter()
        .filter(|w| {
            !target.executable_or_bundle_id.is_empty()
                && w.executable_or_bundle_id == target.executable_or_bundle_id
                && !already_bound.contains(&w.platform_window_id)
        })
        .map(|w| (title_score(target, w), w))
        .max_by_key(|(score, _)| *score)
        .map(|(_, w)| w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mock::MockDesktop;
    use crate::types::OutputKind;

    #[test]
    fn identity_requires_window_process_and_app() {
        let w = MockDesktop::window("10", 100, "/Apps/Code", "backend — main.rs");
        let t = TargetSlot::from_window(&w, 1, OutputKind::Prompt);
        assert!(identity_matches(&t, &w));

        let same_title_new_window =
            MockDesktop::window("11", 100, "/Apps/Code", "backend — main.rs");
        assert!(!identity_matches(&t, &same_title_new_window));
        let recycled_id = MockDesktop::window("10", 999, "/Apps/Code", "backend — main.rs");
        assert!(!identity_matches(&t, &recycled_id));
        let other_app = MockDesktop::window("10", 100, "/Apps/Other", "backend — main.rs");
        assert!(!identity_matches(&t, &other_app));
    }

    #[test]
    fn suggestion_requires_same_application() {
        let old = MockDesktop::window("10", 100, "/Apps/Code", "backend — main.rs");
        let t = TargetSlot::from_window(&old, 1, OutputKind::Prompt);
        let windows = vec![
            MockDesktop::window("20", 200, "/Apps/Browser", "backend — main.rs"),
            MockDesktop::window("21", 201, "/Apps/Code", "frontend — app.tsx"),
            MockDesktop::window("22", 201, "/Apps/Code", "backend — lib.rs"),
        ];
        let s = suggest(&t, &windows, &[]).unwrap();
        assert_eq!(s.platform_window_id, "22");
        // Windows already pinned elsewhere are never suggested.
        let s = suggest(&t, &windows, &["22".to_string()]).unwrap();
        assert_eq!(s.platform_window_id, "21");
        assert!(suggest(&t, &windows[..1], &[]).is_none());
    }
}
