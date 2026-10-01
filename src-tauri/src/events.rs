//! Backend → frontend events.

use std::sync::Mutex;

use crate::error::AppError;
use crate::target::TargetsSnapshot;
use crate::types::*;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum AppEvent {
    StateChanged(AppSnapshot),
    RecordingLevel(f32),
    TargetsChanged(TargetsSnapshot),
    ProviderHealthChanged(ProvidersHealth),
    UtteranceUpdated(UtteranceResult),
    InjectionResult(InjectionOutcome),
    PermissionRequired(PermissionRequired),
    FatalError(AppError),
}

impl AppEvent {
    /// Event name on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            AppEvent::StateChanged(_) => "state_changed",
            AppEvent::RecordingLevel(_) => "recording_level",
            AppEvent::TargetsChanged(_) => "targets_changed",
            AppEvent::ProviderHealthChanged(_) => "provider_health_changed",
            AppEvent::UtteranceUpdated(_) => "utterance_updated",
            AppEvent::InjectionResult(_) => "injection_result",
            AppEvent::PermissionRequired(_) => "permission_required",
            AppEvent::FatalError(_) => "fatal_error",
        }
    }
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: AppEvent);
}

/// Discards everything.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: AppEvent) {}
}

/// Records events for tests.
#[derive(Default)]
pub struct CollectingSink {
    pub events: Mutex<Vec<AppEvent>>,
}

impl EventSink for CollectingSink {
    fn emit(&self, event: AppEvent) {
        self.events.lock().unwrap().push(event);
    }
}
