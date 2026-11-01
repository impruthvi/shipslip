//! Typed evidence for the start of a log snapshot's window.

use super::super::Since;
use crate::records::Untrusted;

/// The evidence selected for a snapshot's window start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Anchor {
    CheckoutChange {
        commit: Untrusted,
    },
    RunInProgress {
        run_id: Untrusted,
        step: Option<usize>,
    },
    Run {
        run_id: Untrusted,
        plan: Untrusted,
        target_commit: Untrusted,
        check: RunCheck,
        /// What the server's last-run marker says, never a receipt outcome.
        server_marker_outcome: Option<Untrusted>,
        /// Without a recorded reflog, the text label keeps the outcome unknown.
        reflog_recorded: bool,
    },
    Since {
        since: Since,
    },
    Default {
        seconds: u64,
    },
}

/// Whether the checkout confirmed the run named by the server's marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunCheck {
    Verified,
    Unverified { reason: Untrusted },
}

/// An anchor candidate that could not be used. Missing candidates are kept
/// even when the existing text output has no warning for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorFallback {
    Run {
        run_id: Untrusted,
        reason: RunFallbackReason,
    },
    /// A run anchor could not be read, so its run ID is unavailable.
    LastRunMarker {
        reason: MarkerFallbackReason,
    },
    CheckoutChange {
        reason: CheckoutFallbackReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutFallbackReason {
    ReflogMissing,
    ReflogReadFailed { message: Untrusted },
    HeadReadFailed { message: Untrusted },
    InvalidStartTime { commit: Untrusted },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerFallbackReason {
    Missing,
    Corrupt,
    Unreadable { message: Untrusted },
}

impl From<&str> for MarkerFallbackReason {
    fn from(message: &str) -> Self {
        Self::Unreadable {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunFallbackReason {
    ReflogMissing,
    TargetNotReached { target_commit: Untrusted },
    CheckoutChanged,
    InvalidStartTime,
}

impl AnchorFallback {
    pub(super) fn warning(&self) -> Option<String> {
        Some(match self {
            Self::CheckoutChange {
                reason:
                    CheckoutFallbackReason::ReflogMissing
                    | CheckoutFallbackReason::InvalidStartTime { .. },
            } => return None,
            Self::CheckoutChange {
                reason:
                    CheckoutFallbackReason::ReflogReadFailed { message }
                    | CheckoutFallbackReason::HeadReadFailed { message },
            } => format!("anchor: reflog failed ({}) — using 24h", message.raw()),
            Self::LastRunMarker {
                reason: MarkerFallbackReason::Missing,
            } => return None,
            Self::LastRunMarker {
                reason: MarkerFallbackReason::Corrupt,
            } => "anchor: last-run marker is corrupt".into(),
            Self::LastRunMarker {
                reason: MarkerFallbackReason::Unreadable { message },
            } => format!("anchor: {}", message.raw()),
            Self::Run {
                run_id,
                reason: RunFallbackReason::ReflogMissing,
            } => format!(
                "run {} could not be verified: reflog is missing",
                run_id.raw()
            ),
            Self::Run {
                run_id,
                reason: RunFallbackReason::TargetNotReached { target_commit },
            } => format!(
                "run {} did not reach its target {}",
                run_id.raw(),
                &target_commit.raw()[..7]
            ),
            Self::Run {
                run_id,
                reason: RunFallbackReason::CheckoutChanged,
            } => format!("checkout changed after run {}", run_id.raw()),
            Self::Run {
                reason: RunFallbackReason::InvalidStartTime,
                ..
            } => "last-run marker has an invalid start time".into(),
        })
    }
}

impl Anchor {
    pub(super) fn label(&self) -> String {
        match self {
            Self::CheckoutChange { commit } => {
                format!("since checkout change {}", &commit.raw()[..7])
            }
            Self::RunInProgress { run_id, step } => format!(
                "deploy in progress (run {}{}); since run start",
                run_id.raw(),
                step.map(|step| format!(", step {step}"))
                    .unwrap_or_default()
            ),
            Self::Run {
                run_id,
                plan,
                target_commit,
                check,
                server_marker_outcome,
                reflog_recorded,
            } => {
                let detail = match check {
                    RunCheck::Unverified { reason } => format!("unverified: {}", reason.raw()),
                    RunCheck::Verified => if *reflog_recorded {
                        server_marker_outcome
                            .as_ref()
                            .map(|outcome| outcome.raw())
                            .unwrap_or("outcome unknown")
                    } else {
                        "outcome unknown"
                    }
                    .into(),
                };
                format!(
                    "since slip {} {} ({}, {detail})",
                    plan.raw(),
                    run_id.raw(),
                    &target_commit.raw()[..7]
                )
            }
            Self::Since { since } => since.label(),
            Self::Default { .. } => "since 24h (default)".into(),
        }
    }
}
