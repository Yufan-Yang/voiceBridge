use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::platform::WindowInfo;
use crate::types::OutputKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AdapterKind {
    #[default]
    GenericClipboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TargetStatus {
    Online,
    #[default]
    Offline,
}

/// A pinned top-level coding window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TargetSlot {
    pub id: String,
    pub slot: u8,
    pub alias: String,

    pub platform_window_id: String,
    pub process_id: u32,
    pub executable_or_bundle_id: String,
    pub title_hint: String,

    pub project_root: Option<String>,
    pub project_name: Option<String>,

    #[serde(default)]
    pub adapter: AdapterKind,
    #[serde(default)]
    pub preferred_output: OutputKind,
    /// Press Enter after pasting. Always defaults to false.
    #[serde(default)]
    pub auto_submit: bool,
    #[serde(default)]
    pub status: TargetStatus,
    /// Created by follow-focus and never customised. Such a target is
    /// dropped automatically once its window is closed.
    #[serde(default = "default_auto_tracked")]
    pub auto_tracked: bool,
    /// Vocabulary terms added by the user for this target.
    #[serde(default)]
    pub manual_terms: Vec<String>,
}

fn default_auto_tracked() -> bool {
    true
}

impl TargetSlot {
    /// Creates a target for `window`. `auto_submit` is always off for a new
    /// target unless the caller explicitly opts in.
    pub fn from_window(window: &WindowInfo, slot: u8, preferred_output: OutputKind) -> Self {
        let alias: String = if window.app_name.is_empty() {
            format!("Window {slot}")
        } else {
            window.app_name.chars().take(18).collect()
        };
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            slot,
            alias,
            platform_window_id: window.platform_window_id.clone(),
            process_id: window.process_id,
            executable_or_bundle_id: window.executable_or_bundle_id.clone(),
            title_hint: window.title.clone(),
            project_root: None,
            project_name: None,
            adapter: AdapterKind::GenericClipboard,
            preferred_output,
            auto_submit: false,
            status: TargetStatus::Online,
            auto_tracked: false,
            manual_terms: Vec::new(),
        }
    }
}

/// A window that probably is a closed target reopened. Never applied
/// automatically: the user must confirm the rebind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RebindSuggestion {
    pub target_id: String,
    pub window: WindowInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TargetsSnapshot {
    pub targets: Vec<TargetSlot>,
    pub selected_id: Option<String>,
    pub suggestions: Vec<RebindSuggestion>,
}
