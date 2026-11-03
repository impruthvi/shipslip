use crate::records::{Flag, Outcome, OutcomeKind, StepState};
#[cfg(test)]
use crate::WatchStatus;

fn outcome_label(kind: OutcomeKind) -> Option<String> {
    Some(match kind {
        OutcomeKind::Succeeded => "Succeeded".into(),
        OutcomeKind::FailedAtStep {
            step,
            partial_update: true,
        } => format!("Failed at step {step} (partial update)"),
        OutcomeKind::FailedAtStep { step, .. } => format!("Failed at step {step}"),
        OutcomeKind::StoppedAfterStep { step } => format!("Stopped after step {step}"),
        OutcomeKind::CancelledBeforeChanges => "Cancelled before changes".into(),
        OutcomeKind::AbortedBeforeChanges => "Aborted before changes".into(),
        OutcomeKind::StoppedInMaintenance => "Stopped in maintenance mode, no steps ran".into(),
        OutcomeKind::Unknown { step } => format!("Unknown at step {step}"),
        OutcomeKind::NotRecorded => return None,
    })
}

pub(super) fn headline(outcome: Outcome) -> String {
    match (outcome.unfinished, outcome_label(outcome.kind)) {
        (false, Some(label)) => label,
        (false, None) => "Finished without an outcome".into(),
        (true, Some(label)) => format!("Unfinished (last recorded: {label})"),
        (true, None) => "Unfinished".into(),
    }
}

pub(super) fn flag_text(flag: Flag) -> String {
    let symbol = match flag {
        Flag::NewErrors { .. } | Flag::SmokeFailed { .. } => "⚠ ",
        Flag::AppLeftDown => "⛔ ",
        Flag::NoLogFileYet | Flag::LogWatchStoppedEarly | Flag::LogWatchSkippedInMaintenance => "",
        _ => "ⓘ ",
    };
    format!("{symbol}{}", flag.description())
}

pub(super) fn step_label(state: StepState) -> &'static str {
    match state {
        StepState::Ok => "✓ ok",
        StepState::Failed => "✗ failed",
        StepState::Unknown => "? unknown",
        StepState::NotStarted | StepState::Pending => "– not run",
        StepState::Running => "? running when saved",
        StepState::Skipped => "↷ skipped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_headlines_preserve_each_outcome_and_unfinished_state() {
        use crate::records::{Outcome, OutcomeKind};
        let cases = [
            (OutcomeKind::Succeeded, "Succeeded"),
            (
                OutcomeKind::FailedAtStep {
                    step: 2,
                    partial_update: true,
                },
                "Failed at step 2 (partial update)",
            ),
            (
                OutcomeKind::FailedAtStep {
                    step: 2,
                    partial_update: false,
                },
                "Failed at step 2",
            ),
            (
                OutcomeKind::StoppedAfterStep { step: 2 },
                "Stopped after step 2",
            ),
            (
                OutcomeKind::StoppedInMaintenance,
                "Stopped in maintenance mode, no steps ran",
            ),
            (OutcomeKind::Unknown { step: 2 }, "Unknown at step 2"),
            (
                OutcomeKind::CancelledBeforeChanges,
                "Cancelled before changes",
            ),
            (OutcomeKind::AbortedBeforeChanges, "Aborted before changes"),
        ];
        for (kind, expected) in cases {
            assert_eq!(
                headline(Outcome {
                    kind,
                    unfinished: false
                }),
                expected
            );
            assert_eq!(
                headline(Outcome {
                    kind,
                    unfinished: true
                }),
                format!("Unfinished (last recorded: {expected})")
            );
        }
        assert_eq!(
            headline(Outcome {
                kind: OutcomeKind::NotRecorded,
                unfinished: false
            }),
            "Finished without an outcome"
        );
        assert_eq!(
            headline(Outcome {
                kind: OutcomeKind::NotRecorded,
                unfinished: true
            }),
            "Unfinished"
        );
    }

    #[test]
    fn receipt_flags_preserve_the_terminal_symbols_and_counts() {
        use crate::records::Flag;
        for (flag, expected) in [
            (Flag::NewErrors { groups: 1 }, "⚠ 1 new error group"),
            (Flag::NewErrors { groups: 3 }, "⚠ 3 new error groups"),
            (
                Flag::LogWatchInterrupted,
                "ⓘ log watch interrupted · entries may be missing",
            ),
            (Flag::LogNotRecorded, "ⓘ log not recorded"),
            (Flag::SmokeFailed { status: Some(500) }, "⚠ smoke 500"),
            (Flag::SmokeFailed { status: None }, "⚠ smoke failed"),
            (Flag::SmokeNotRecorded, "ⓘ smoke not recorded"),
            (Flag::AppLeftDown, "⛔ app may be in maintenance mode"),
        ] {
            assert_eq!(flag_text(flag), expected);
        }
    }

    #[test]
    fn receipt_step_and_watch_labels_preserve_every_state() {
        use crate::records::StepState;
        for (state, expected) in [
            (StepState::Ok, "✓ ok"),
            (StepState::Failed, "✗ failed"),
            (StepState::Unknown, "? unknown"),
            (StepState::NotStarted, "– not run"),
            (StepState::Pending, "– not run"),
            (StepState::Running, "? running when saved"),
            (StepState::Skipped, "↷ skipped"),
        ] {
            assert_eq!(step_label(state), expected);
        }
        for (state, expected) in [
            (WatchStatus::Complete, "complete"),
            (
                WatchStatus::Partial,
                "log watch interrupted · entries may be missing",
            ),
            (WatchStatus::Unavailable, "log could not be read"),
            (WatchStatus::Cancelled, "log watch stopped early"),
            (WatchStatus::NotRun, "not run"),
            (WatchStatus::NoLogSeen, "no log file yet"),
        ] {
            assert_eq!(state.description(None), expected);
        }
    }
}
