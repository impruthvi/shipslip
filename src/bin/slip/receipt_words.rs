use shipslip::records::{Flag, Outcome, OutcomeKind, Smoke, StepState};

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

pub(crate) fn flag_text(flag: Flag) -> String {
    let symbol = match flag {
        Flag::NewErrors { .. } | Flag::SmokeFailed { .. } => "⚠ ",
        Flag::AppLeftDown => "⛔ ",
        Flag::NoLogFileYet | Flag::LogWatchStoppedEarly | Flag::LogWatchSkippedInMaintenance => "",
        _ => "ⓘ ",
    };
    format!("{symbol}{}", flag.description())
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
