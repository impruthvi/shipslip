use shipslip::records::{Outcome, OutcomeKind, Smoke, StepState};
#[cfg(test)]
use shipslip::WatchStatus;

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

pub(crate) fn headline(outcome: Outcome) -> String {
    match (outcome.unfinished, outcome_label(outcome.kind)) {
        (false, Some(label)) => label,
        (false, None) => "Finished without an outcome".into(),
        (true, Some(label)) => format!("Unfinished (last recorded: {label})"),
        (true, None) => "Unfinished".into(),
    }
}

pub(crate) fn step_label(state: StepState) -> &'static str {
    match state {
        StepState::Ok => "✓ ok",
        StepState::Failed => "✗ failed",
        StepState::Unknown => "? unknown",
        StepState::NotStarted | StepState::Pending => "– not run",
        StepState::Running => "? running when saved",
        StepState::Skipped => "↷ skipped",
    }
}

pub(crate) fn smoke_text(smoke: &Smoke) -> String {
    match smoke {
        Smoke::NotConfigured => "not configured".into(),
        Smoke::Skipped => "skipped".into(),
        Smoke::Passed { status, latency_ms } => {
            format!("passed, HTTP {status} in {latency_ms} ms")
        }
        Smoke::Failed {
            status: Some(status),
            reason,
        } if reason.raw() == format!("HTTP {status}") => format!("failed, HTTP {status}"),
        Smoke::Failed {
            status: Some(status),
            reason,
        } => format!("failed, HTTP {status}: {}", reason.field()),
        Smoke::Failed {
            status: None,
            reason,
        } => format!("failed: {}", reason.field()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_headlines_preserve_each_outcome_and_unfinished_state() {
        use shipslip::records::{Outcome, OutcomeKind};
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
    fn receipt_step_and_watch_labels_preserve_every_state() {
        use shipslip::records::StepState;
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
    #[test]
    fn smoke_results_in_words() {
        use shipslip::SmokeResult;
        let failed = |status, reason: &str| SmokeResult::Failed {
            status,
            reason: reason.into(),
        };
        let cases = [
            (SmokeResult::NotConfigured, "not configured"),
            (SmokeResult::Skipped, "skipped"),
            (
                SmokeResult::Passed {
                    status: 200,
                    latency_ms: 42,
                },
                "passed, HTTP 200 in 42 ms",
            ),
            (failed(Some(500), "HTTP 500"), "failed, HTTP 500"),
            (
                failed(Some(502), "bad gateway"),
                "failed, HTTP 502: bad gateway",
            ),
            (
                failed(None, "timed out\u{1b}[2J"),
                "failed: timed out\\x1b[2J",
            ),
        ];
        for (smoke, words) in cases {
            assert_eq!(smoke_text(&shipslip::records::Smoke::from(&smoke)), words);
        }
    }
}
