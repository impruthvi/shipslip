//! Words for the release map, shared by `slip status` and the desktop app.
//!
//! A cell never says "deployed" or "running": it says what the receipts on
//! this Mac recorded or observed. Every string is escaped for display.

use std::fmt::Write;

use super::{CodeState, EnvCell, EnvComparison, Evidence, NotKnownReason, Qualifier};
use crate::git::Comparison;
use crate::logs::escape_field;
use crate::receipt::{Flag, ReceiptStepStatus, MIN_ID_LEN};
use crate::DeployOutcome;

/// The shape that tells cell states apart without color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// The run recorded the code and finished.
    Recorded,
    /// The code moved but the run did not finish.
    Incomplete,
    /// The server HEAD was read after a failed or unknown step.
    Observed,
    NotKnown,
    /// No code recorded for this cell.
    NoCode,
}

/// A cell's code fact in parts, so a front-end can lay it out on several
/// lines. [`sentence`] joins the same parts for one-line output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeText {
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

pub fn code_text(cell: &EnvCell) -> CodeText {
    let text = |mark, sha: Option<&String>, at_ms, words: &str, evidence: Vec<String>| CodeText {
        mark,
        sha: sha.map(|sha| escape_field(sha)),
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
            let mark = if *complete {
                Mark::Recorded
            } else {
                Mark::Incomplete
            };
            text(mark, Some(sha), Some(*at_ms), "", facts)
        }
        CodeState::Observed {
            sha,
            dirty,
            expected,
            run_id,
            outcome,
            at_ms,
        } => {
            let mut after = format!("after run {}", short_id(run_id));
            match outcome {
                Some(DeployOutcome::FailedAtStep { step, .. }) => {
                    let _ = write!(after, " failed at step {step}");
                }
                Some(DeployOutcome::StoppedAfterStep { step, .. }) => {
                    let _ = write!(after, " stopped after step {step}");
                }
                Some(DeployOutcome::Unknown { step, .. }) => {
                    let _ = write!(after, " lost track of step {step}");
                }
                _ => {}
            }
            let mut facts = vec![after];
            if !sha.eq_ignore_ascii_case(expected) {
                facts.push(format!("expected {}", short_sha(expected, 12)));
            }
            if *dirty == Some(true) {
                facts.push("tree dirty".into());
            }
            text(Mark::Observed, Some(sha), Some(*at_ms), "observed", facts)
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
                facts.push(format!("after run {}", short_id(run_id)));
            }
            facts.push(why.into());
            text(Mark::NotKnown, None, None, "server code not known", facts)
        }
        CodeState::NeverDeployed => {
            let facts = if cell.approved {
                vec!["approved before".into()]
            } else {
                Vec::new()
            };
            text(
                Mark::NoCode,
                None,
                None,
                "no deploy receipt on this Mac",
                facts,
            )
        }
        CodeState::NoCodeChangeInLast { records: 1 } => text(
            Mark::NoCode,
            None,
            None,
            "no code change in the last run",
            Vec::new(),
        ),
        CodeState::NoCodeChangeInLast { records } => text(
            Mark::NoCode,
            None,
            None,
            &format!("no code change in the last {records} runs"),
            Vec::new(),
        ),
        CodeState::TargetChanged { run_id, target } => text(
            Mark::NoCode,
            None,
            None,
            &format!("no code recorded for {target}"),
            vec![format!("target changed in run {}", short_id(run_id))],
        ),
    }
}

/// The cell's code fact on one line, as `slip status` prints it.
pub fn sentence(cell: &EnvCell, now_ms: u128) -> String {
    let text = code_text(cell);
    let sha = text
        .sha
        .as_deref()
        .map(|sha| short_sha(sha, 12))
        .unwrap_or_default();
    let age = text
        .at_ms
        .map(|at_ms| age(now_ms, at_ms))
        .unwrap_or_default();
    let facts = &text.evidence;
    match &cell.code {
        CodeState::Recorded { .. } => {
            let mut line = format!("code at {sha}");
            for fact in facts {
                let _ = write!(line, ", {fact}");
            }
            format!("{line} (recorded {age})")
        }
        // The first fact says which run and how it ended.
        CodeState::Observed { .. } => {
            let mut line = format!("observed {sha} {}", facts[0]);
            for fact in &facts[1..] {
                let _ = write!(line, ", {fact}");
            }
            format!("{line} ({age})")
        }
        // The last fact is the reason; an "after run" fact may precede it.
        CodeState::NotKnown { .. } => match facts.as_slice() {
            [after, why] => format!("{} {after} ({why})", text.words),
            [why] => format!("{} ({why})", text.words),
            _ => text.words,
        },
        CodeState::TargetChanged { .. } => format!("{}; {}", facts[0], text.words),
        CodeState::NeverDeployed | CodeState::NoCodeChangeInLast { .. } => match facts.first() {
            Some(approved) => format!("{approved}, {}", text.words),
            None => text.words,
        },
    }
}

/// How env `a` relates to env `b`, with what the count alone would hide.
pub fn comparison_text(a: &str, b: &str, comparison: &EnvComparison) -> String {
    let commits = |n: usize| if n == 1 { "commit" } else { "commits" };
    let mut line = match &comparison.result {
        Comparison::Same => format!("{a} and {b} are on the same commit"),
        Comparison::Ahead(n) => format!("{a} is {n} {} ahead of {b}", commits(*n)),
        Comparison::Behind(n) => format!("{a} is {n} {} behind {b}", commits(*n)),
        Comparison::Diverged { a: left, b: right } => {
            format!("{a} and {b} have diverged: {left} only on {a}, {right} only on {b}")
        }
        Comparison::Unavailable(reason) => format!("cannot compare: {reason}"),
    };
    if !comparison.notes.is_empty() {
        let notes: Vec<String> = comparison
            .notes
            .iter()
            .map(|(env, qualifier)| match qualifier {
                Qualifier::DeployIncomplete => format!("{env} deploy incomplete"),
                Qualifier::TreeDirty => format!("{env} tree dirty"),
            })
            .collect();
        let _ = write!(line, " ({})", notes.join("; "));
    }
    escape_field(&line)
}

/// A warning when the approved config deploys somewhere other than the
/// newest receipt's target, so the cell's code may not be on it.
pub fn target_moved_text(cell: &EnvCell) -> Option<String> {
    let approved = cell.target_moved()?;
    Some(escape_field(&format!(
        "approved config now deploys to {approved}, not this target"
    )))
}

impl Flag {
    /// The flag in words for a cell, without the leading symbol.
    pub fn cell_text(&self) -> String {
        match self {
            Self::NewErrors(1) => "1 new error group after this run".into(),
            Self::NewErrors(count) => format!("{count} new error groups after this run"),
            Self::LogNotFullyObserved => "log not fully observed".into(),
            Self::LogNotRecorded => "log not recorded".into(),
            Self::SmokeFailed(Some(status)) => format!("smoke {status}"),
            Self::SmokeFailed(None) => "smoke failed".into(),
            Self::SmokeNotRecorded => "smoke not recorded".into(),
            Self::AppLeftDown => "app may be in maintenance mode".into(),
        }
    }
}

/// The first `len` characters of a commit.
pub fn short_sha(sha: &str, len: usize) -> &str {
    sha.get(..len).unwrap_or(sha)
}

pub fn short_id(run_id: &str) -> &str {
    run_id.get(..MIN_ID_LEN).unwrap_or(run_id)
}

/// How long ago, in the largest whole unit.
pub fn age(now_ms: u128, then_ms: u128) -> String {
    let minutes = now_ms.saturating_sub(then_ms) / 60_000;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes}m ago"),
        60..=2879 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}
