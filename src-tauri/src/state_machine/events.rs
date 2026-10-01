/// Events that drive the utterance state machine. Every event produced by
/// asynchronous work carries the `utterance_id` it belongs to so stale
/// results can be rejected.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Push-to-Talk pressed. The selected target is frozen here.
    PttPressed {
        utterance_id: String,
        frozen_target_id: Option<String>,
    },
    PttReleased {
        utterance_id: String,
    },
    AudioFinalized {
        utterance_id: String,
    },
    Transcribed {
        utterance_id: String,
    },
    Normalized {
        utterance_id: String,
    },
    Compiled {
        utterance_id: String,
    },
    /// Re-run prompt compilation for an already finished utterance.
    RecompileStarted {
        utterance_id: String,
        frozen_target_id: Option<String>,
    },
    InjectStarted {
        utterance_id: String,
        frozen_target_id: Option<String>,
    },
    InjectFinished {
        utterance_id: String,
    },
    Failed {
        utterance_id: String,
    },
    /// User cancel / dismiss.
    Canceled,
    /// Return to idle after a completion, error or cancel notice.
    Reset,
}
