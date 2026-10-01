//! Pure reducer for the utterance state machine.

use super::events::Event;
use crate::types::Phase;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Machine {
    pub phase: Phase,
    pub utterance_id: Option<String>,
    /// Target frozen when recording began. All later stages use this.
    pub frozen_target_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    /// The event belongs to an utterance that is no longer current.
    IgnoredStale,
    /// The event is not valid in the current phase.
    Rejected,
}

fn idle_like(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::Idle | Phase::Ready | Phase::Done | Phase::Error | Phase::Canceled
    )
}

pub fn reduce(m: &Machine, event: &Event) -> (Machine, Outcome) {
    let stay = |outcome| (m.clone(), outcome);
    let is_current = |id: &String| m.utterance_id.as_ref() == Some(id);
    let advance = |id: &String, from: Phase, to: Phase| {
        if !is_current(id) {
            stay(Outcome::IgnoredStale)
        } else if m.phase != from {
            stay(Outcome::Rejected)
        } else {
            (
                Machine {
                    phase: to,
                    ..m.clone()
                },
                Outcome::Applied,
            )
        }
    };

    match event {
        Event::PttPressed {
            utterance_id,
            frozen_target_id,
        } => {
            // Only one recording at a time; never interrupt an injection.
            if matches!(m.phase, Phase::Listening | Phase::Injecting) {
                return stay(Outcome::Rejected);
            }
            (
                Machine {
                    phase: Phase::Listening,
                    utterance_id: Some(utterance_id.clone()),
                    frozen_target_id: frozen_target_id.clone(),
                },
                Outcome::Applied,
            )
        }
        Event::PttReleased { utterance_id } => {
            advance(utterance_id, Phase::Listening, Phase::FinalizingAudio)
        }
        Event::AudioFinalized { utterance_id } => {
            advance(utterance_id, Phase::FinalizingAudio, Phase::Transcribing)
        }
        Event::Transcribed { utterance_id } => {
            advance(utterance_id, Phase::Transcribing, Phase::Normalizing)
        }
        Event::Normalized { utterance_id } => {
            advance(utterance_id, Phase::Normalizing, Phase::CompilingPrompt)
        }
        Event::Compiled { utterance_id } => {
            advance(utterance_id, Phase::CompilingPrompt, Phase::Ready)
        }
        Event::RecompileStarted {
            utterance_id,
            frozen_target_id,
        } => {
            if !idle_like(m.phase) {
                return stay(Outcome::Rejected);
            }
            (
                Machine {
                    phase: Phase::CompilingPrompt,
                    utterance_id: Some(utterance_id.clone()),
                    frozen_target_id: frozen_target_id.clone(),
                },
                Outcome::Applied,
            )
        }
        Event::InjectStarted {
            utterance_id,
            frozen_target_id,
        } => {
            if !idle_like(m.phase) {
                return stay(Outcome::Rejected);
            }
            (
                Machine {
                    phase: Phase::Injecting,
                    utterance_id: Some(utterance_id.clone()),
                    frozen_target_id: frozen_target_id.clone(),
                },
                Outcome::Applied,
            )
        }
        Event::InjectFinished { utterance_id } => {
            advance(utterance_id, Phase::Injecting, Phase::Done)
        }
        Event::Failed { utterance_id } => {
            if !is_current(utterance_id) {
                return stay(Outcome::IgnoredStale);
            }
            (
                Machine {
                    phase: Phase::Error,
                    ..m.clone()
                },
                Outcome::Applied,
            )
        }
        Event::Canceled => {
            if m.phase.is_processing() {
                // Dropping the utterance id makes every later result stale.
                (
                    Machine {
                        phase: Phase::Canceled,
                        utterance_id: None,
                        frozen_target_id: None,
                    },
                    Outcome::Applied,
                )
            } else if matches!(
                m.phase,
                Phase::Ready | Phase::Done | Phase::Error | Phase::Canceled
            ) {
                (Machine::default(), Outcome::Applied)
            } else {
                stay(Outcome::Rejected)
            }
        }
        Event::Reset => {
            if matches!(m.phase, Phase::Done | Phase::Error | Phase::Canceled) {
                (Machine::default(), Outcome::Applied)
            } else {
                stay(Outcome::Rejected)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(m: &Machine, id: &str, target: Option<&str>) -> (Machine, Outcome) {
        reduce(
            m,
            &Event::PttPressed {
                utterance_id: id.into(),
                frozen_target_id: target.map(String::from),
            },
        )
    }

    fn run(m: Machine, events: &[Event]) -> Machine {
        events.iter().fold(m, |m, e| {
            let (next, outcome) = reduce(&m, e);
            assert_eq!(outcome, Outcome::Applied, "{e:?} in {:?}", m.phase);
            next
        })
    }

    fn id(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn happy_path_walks_every_phase() {
        let (m, o) = press(&Machine::default(), "u1", Some("t1"));
        assert_eq!(o, Outcome::Applied);
        assert_eq!(m.phase, Phase::Listening);
        let m = run(
            m,
            &[
                Event::PttReleased {
                    utterance_id: id("u1"),
                },
                Event::AudioFinalized {
                    utterance_id: id("u1"),
                },
                Event::Transcribed {
                    utterance_id: id("u1"),
                },
                Event::Normalized {
                    utterance_id: id("u1"),
                },
                Event::Compiled {
                    utterance_id: id("u1"),
                },
            ],
        );
        assert_eq!(m.phase, Phase::Ready);
        let m = run(
            m,
            &[
                Event::InjectStarted {
                    utterance_id: id("u1"),
                    frozen_target_id: Some(id("t1")),
                },
                Event::InjectFinished {
                    utterance_id: id("u1"),
                },
                Event::Reset,
            ],
        );
        assert_eq!(m, Machine::default());
    }

    #[test]
    fn target_is_frozen_when_recording_begins() {
        let (m, _) = press(&Machine::default(), "u1", Some("target-a"));
        assert_eq!(m.frozen_target_id.as_deref(), Some("target-a"));
        // No processing event carries a target, so nothing can change it.
        let m = run(
            m,
            &[
                Event::PttReleased {
                    utterance_id: id("u1"),
                },
                Event::AudioFinalized {
                    utterance_id: id("u1"),
                },
                Event::Transcribed {
                    utterance_id: id("u1"),
                },
            ],
        );
        assert_eq!(m.frozen_target_id.as_deref(), Some("target-a"));
    }

    #[test]
    fn rapid_push_to_talk_does_not_restart_recording() {
        let (m, _) = press(&Machine::default(), "u1", None);
        let (m2, o) = press(&m, "u2", None);
        assert_eq!(o, Outcome::Rejected);
        assert_eq!(m2.utterance_id.as_deref(), Some("u1"));
    }

    #[test]
    fn stale_results_are_ignored() {
        let (m, _) = press(&Machine::default(), "old", None);
        let m = run(
            m,
            &[
                Event::PttReleased {
                    utterance_id: id("old"),
                },
                Event::AudioFinalized {
                    utterance_id: id("old"),
                },
            ],
        );
        // A new recording supersedes the old utterance while it transcribes.
        let (m, o) = press(&m, "new", None);
        assert_eq!(o, Outcome::Applied);
        for stale in [
            Event::Transcribed {
                utterance_id: id("old"),
            },
            Event::Compiled {
                utterance_id: id("old"),
            },
            Event::Failed {
                utterance_id: id("old"),
            },
            Event::InjectFinished {
                utterance_id: id("old"),
            },
        ] {
            let (after, o) = reduce(&m, &stale);
            assert_eq!(o, Outcome::IgnoredStale);
            assert_eq!(after, m);
        }
    }

    #[test]
    fn cancel_makes_later_results_stale() {
        let (m, _) = press(&Machine::default(), "u1", Some("t"));
        let m = run(
            m,
            &[
                Event::PttReleased {
                    utterance_id: id("u1"),
                },
                Event::Canceled,
            ],
        );
        assert_eq!(m.phase, Phase::Canceled);
        let (_, o) = reduce(
            &m,
            &Event::AudioFinalized {
                utterance_id: id("u1"),
            },
        );
        assert_eq!(o, Outcome::IgnoredStale);
    }

    #[test]
    fn injection_cannot_be_interrupted_or_started_mid_processing() {
        let (listening, _) = press(&Machine::default(), "u1", None);
        let inject = Event::InjectStarted {
            utterance_id: id("u0"),
            frozen_target_id: None,
        };
        assert_eq!(reduce(&listening, &inject).1, Outcome::Rejected);

        let (injecting, o) = reduce(&Machine::default(), &inject);
        assert_eq!(o, Outcome::Applied);
        assert_eq!(press(&injecting, "u2", None).1, Outcome::Rejected);
        assert_eq!(reduce(&injecting, &Event::Canceled).1, Outcome::Rejected);
    }

    #[test]
    fn out_of_order_events_are_rejected() {
        let (m, _) = press(&Machine::default(), "u1", None);
        let (_, o) = reduce(
            &m,
            &Event::Compiled {
                utterance_id: id("u1"),
            },
        );
        assert_eq!(o, Outcome::Rejected);
        assert_eq!(
            reduce(&Machine::default(), &Event::Reset).1,
            Outcome::Rejected
        );
    }
}
