//! Release-map wording owned by this front-end. Receipt text is escaped here.

use shipslip::logs::escape_field;
use shipslip::receipt::{ReceiptStepStatus, MIN_ID_LEN};
use shipslip::records::{CodeState, EnvCell, Evidence, Mark, NotKnownReason, OutcomeKind};
use std::fmt::Write;

/// The code fact in parts for the release map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodeText {
    pub mark: Mark,
    /// Full commit, when the cell has one.
    pub sha: Option<String>,
    /// When the fact was recorded.
    pub at_ms: Option<u128>,
    /// The state in a few words; empty for recorded code, whose outcome
    /// comes from the run's badge.
    pub words: String,
    /// Facts that qualify the state, e.g. "deploy incomplete".
    pub evidence: Vec<String>,
}

pub(super) fn code_text(cell: &EnvCell) -> CodeText {
    let text = |sha: Option<&shipslip::records::Untrusted>,
                at_ms,
                words: &str,
                evidence: Vec<String>| CodeText {
        mark: cell.mark(),
        sha: sha.map(|sha| sha.field()),
        at_ms,
        words: escape_field(words),
        evidence: evidence.iter().map(|item| escape_field(item)).collect(),
    };
    match &cell.code {
        CodeState::Recorded {
            sha,
            complete,
            evidence,
            at_ms,
            ..
        } => {
            let mut facts = Vec::new();
            if !complete {
                facts.push("deploy incomplete".into());
            }
            if *evidence == Evidence::CheckedOutAtStart {
                facts.push("checked out at start".into());
            }
            text(Some(sha), Some(*at_ms), "", facts)
        }
        CodeState::Observed {
            sha,
            dirty,
            expected,
            run_id,
            outcome,
            at_ms,
        } => {
            let mut after = format!("after run {}", short_id(run_id.raw()));
            match outcome.map(|outcome| outcome.kind) {
                Some(OutcomeKind::FailedAtStep { step, .. }) => {
                    let _ = write!(after, " failed at step {step}");
                }
                Some(OutcomeKind::StoppedAfterStep { step, .. }) => {
                    let _ = write!(after, " stopped after step {step}");
                }
                Some(OutcomeKind::Unknown { step, .. }) => {
                    let _ = write!(after, " lost track of step {step}");
                }
                _ => {}
            }
            let mut facts = vec![after];
            if !sha.raw().eq_ignore_ascii_case(expected.raw()) {
                facts.push(format!("expected {}", short_sha(expected.raw(), 12)));
            }
            if *dirty == Some(true) {
                facts.push("tree dirty".into());
            }
            text(Some(sha), Some(*at_ms), "observed", facts)
        }
        CodeState::NotKnown { run_id, reason } => {
            let why = match reason {
                NotKnownReason::StepZero(ReceiptStepStatus::Failed) => "fast-forward failed",
                NotKnownReason::StepZero(ReceiptStepStatus::Running) => {
                    "fast-forward was running when saved"
                }
                NotKnownReason::StepZero(_) => "fast-forward result unknown",
                NotKnownReason::NoStepZero => "no fast-forward step recorded",
                NotKnownReason::Unreadable => "its receipts could not be read",
            };
            let mut facts = Vec::new();
            if let Some(run_id) = run_id {
                facts.push(format!("after run {}", short_id(run_id.raw())));
            }
            facts.push(why.into());
            text(None, None, "server code not known", facts)
        }
        CodeState::NeverDeployed => {
            let facts = if cell.approved {
                vec!["approved before".into()]
            } else {
                Vec::new()
            };
            text(None, None, "no deploy receipt on this Mac", facts)
        }
        CodeState::NoCodeChangeInLast { records: 1 } => {
            text(None, None, "no code change in the last run", Vec::new())
        }
        CodeState::NoCodeChangeInLast { records } => text(
            None,
            None,
            &format!("no code change in the last {records} runs"),
            Vec::new(),
        ),
        CodeState::TargetChanged { run_id, target } => text(
            None,
            None,
            &format!("no code recorded for {}", target.raw()),
            vec![format!("target changed in run {}", short_id(run_id.raw()))],
        ),
    }
}

/// A warning when the approved config deploys somewhere other than the
/// newest receipt's target, so the cell's code may not be on it.
pub(super) fn target_moved_text(cell: &EnvCell) -> Option<String> {
    let approved = cell.target_moved()?;
    Some(escape_field(&format!(
        "approved config now deploys to {approved}, not this target"
    )))
}

/// A warning when a receipt newer than the one behind the cell could not be
/// read. Front-ends show the file from [`EnvCell::stale_hint`] with it.
pub(super) fn stale_hint_text(cell: &EnvCell) -> Option<String> {
    cell.stale_hint
        .as_ref()
        .map(|_| "a newer receipt could not be read; this may be out of date".into())
}

/// A warning when another checkout ran on this cell's server after it.
pub(super) fn newer_elsewhere_text(cell: &EnvCell, now_ms: u128) -> Option<String> {
    let other = cell.newer_elsewhere.as_ref()?;
    Some(escape_field(&format!(
        "a newer run ({}, {}) from {} ({}) used this server; this may be out of date",
        short_id(other.run_id.raw()),
        age(now_ms, other.started_at_ms),
        other.repo_root.raw(),
        other.env.raw()
    )))
}

/// The first `len` characters of a commit.
pub(super) fn short_sha(sha: &str, len: usize) -> &str {
    sha.get(..len).unwrap_or(sha)
}

pub(super) fn short_id(run_id: &str) -> &str {
    run_id.get(..MIN_ID_LEN).unwrap_or(run_id)
}

/// How long ago, in the largest whole unit.
pub(super) fn age(now_ms: u128, then_ms: u128) -> String {
    let minutes = now_ms.saturating_sub(then_ms) / 60_000;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes}m ago"),
        60..=2879 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}
