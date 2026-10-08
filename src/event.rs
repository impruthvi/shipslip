use crate::observation::{LogPhase, SmokeResult, WatchResult};
use crate::preflight::AbortReason;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// Result of a single step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Ok,
    Failed,
    /// The step's result could not be determined (e.g. the connection was lost).
    Unknown,
    /// The connection failed before the step was sent.
    NotStarted,
}

/// A down command may have changed the app unless it definitely never started.
pub(crate) fn down_may_have_started(status: &StepStatus) -> bool {
    !matches!(status, StepStatus::NotStarted)
}

/// `php artisan down` before step 0, or `php artisan up` after the last step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaintenancePhase {
    Down,
    Up,
}

/// Why a deploy stopped between steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// Stop-after-step, or a detach between steps.
    Requested,
    /// Another run took over the deploy lock.
    LockLost,
    /// The server could not be reached to start or check the next step.
    ConnectFailed(String),
    /// The local receipt could not be saved before another step.
    JournalFailed(String),
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Requested => write!(f, "a stop was requested"),
            Self::LockLost => write!(f, "the deploy lock was taken over"),
            Self::ConnectFailed(reason) => write!(f, "could not reach the server: {reason}"),
            Self::JournalFailed(reason) => write!(f, "could not save the receipt: {reason}"),
        }
    }
}

/// Final result of a deploy run. Step indexes: 0 is the git fast-forward,
/// 1..=N are recipe steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeployOutcome {
    Succeeded,
    /// A step exited non-zero. `partial_update` is set when step 0 (the
    /// fast-forward) failed but the server's code or tree changed anyway.
    FailedAtStep {
        step: usize,
        partial_update: bool,
    },
    StoppedAfterStep {
        step: usize,
        reason: StopReason,
    },
    CancelledBeforeChanges,
    AbortedBeforeChanges(AbortReason),
    /// Maintenance mode may have been turned on, then the run stopped before
    /// any deploy step ran, so the app may still be in maintenance mode.
    StoppedInMaintenance(AbortReason),
    Unknown {
        step: usize,
        reason: String,
    },
}

impl DeployOutcome {
    /// One sentence, including any recorded reason.
    pub fn summary(&self) -> String {
        match self {
            Self::Succeeded => "Deploy run succeeded.".into(),
            Self::FailedAtStep {
                step,
                partial_update,
            } => format!(
                "Deploy failed at step {step}{}.",
                if *partial_update {
                    "; the server may have been partially updated"
                } else {
                    ""
                }
            ),
            Self::StoppedAfterStep { step, reason } => {
                format!("Deploy stopped after step {step}: {reason}.")
            }
            Self::CancelledBeforeChanges => "Deploy was cancelled before changes.".into(),
            Self::AbortedBeforeChanges(reason) => {
                format!("Deploy was aborted before changes: {reason}.")
            }
            Self::StoppedInMaintenance(reason) => format!(
                "Deploy stopped after maintenance mode was turned on; no deploy steps ran: {reason}."
            ),
            Self::Unknown { step, reason } => {
                format!("Outcome of step {step} is unknown: {reason}")
            }
        }
    }

    /// A short label without reasons, which may contain server text.
    pub fn label(&self) -> String {
        match self {
            Self::Succeeded => "Succeeded".into(),
            Self::FailedAtStep {
                step,
                partial_update: true,
            } => format!("Failed at step {step} (partial update)"),
            Self::FailedAtStep { step, .. } => format!("Failed at step {step}"),
            Self::StoppedAfterStep { step, .. } => format!("Stopped after step {step}"),
            Self::CancelledBeforeChanges => "Cancelled before changes".into(),
            Self::AbortedBeforeChanges(_) => "Aborted before changes".into(),
            Self::StoppedInMaintenance(_) => "Stopped in maintenance mode, no steps ran".into(),
            Self::Unknown { step, .. } => format!("Unknown at step {step}"),
        }
    }
}

/// Progress reported by [`crate::execute`], in order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeployEvent {
    StepStarted {
        index: usize,
        name: String,
    },
    Output {
        index: usize,
        line: String,
    },
    StepFinished {
        index: usize,
        status: StepStatus,
        exit_code: Option<i32>,
    },
    /// The front-end detached while step `index` was running. No `Finished`
    /// event follows; the step keeps running on the server.
    Detached {
        index: usize,
    },
    /// The server could not be reached again while step `index` was running.
    /// No `Finished` event follows; the step may still be running there.
    Interrupted {
        index: usize,
        reason: String,
    },
    /// A local receipt or lock operation failed outside a running step.
    RunError {
        reason: String,
    },
    /// A nonfatal ancillary operation failed; the deploy continues.
    Warning {
        reason: String,
    },
    MaintenanceStarted {
        phase: MaintenancePhase,
    },
    MaintenanceOutput {
        phase: MaintenancePhase,
        line: String,
    },
    MaintenanceFinished {
        phase: MaintenancePhase,
        status: StepStatus,
        exit_code: Option<i32>,
    },
    /// Maintenance down may have started and maintenance up did not succeed.
    /// The app may still be down. Sent before the final event.
    AppLeftDown,
    /// The server's checkout after step 0 failed; `None` where it could not
    /// be read.
    ServerState {
        head: Option<String>,
        tree_dirty: Option<bool>,
    },
    NewLogError {
        phase: LogPhase,
        message: String,
        file_line: Option<String>,
    },
    /// The post-deploy log watch started; it runs for about `window`.
    WatchStarted {
        window: Duration,
    },
    WatchFinished(WatchResult),
    SmokeFinished(SmokeResult),
    Finished(DeployOutcome),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_wording_is_stable() {
        let cases = [
            (
                DeployOutcome::Succeeded,
                "Deploy run succeeded.",
                "Succeeded",
            ),
            (
                DeployOutcome::FailedAtStep {
                    step: 2,
                    partial_update: false,
                },
                "Deploy failed at step 2.",
                "Failed at step 2",
            ),
            (
                DeployOutcome::FailedAtStep {
                    step: 0,
                    partial_update: true,
                },
                "Deploy failed at step 0; the server may have been partially updated.",
                "Failed at step 0 (partial update)",
            ),
            (
                DeployOutcome::StoppedAfterStep {
                    step: 1,
                    reason: StopReason::ConnectFailed("timed out".into()),
                },
                "Deploy stopped after step 1: could not reach the server: timed out.",
                "Stopped after step 1",
            ),
            (
                DeployOutcome::CancelledBeforeChanges,
                "Deploy was cancelled before changes.",
                "Cancelled before changes",
            ),
            (
                DeployOutcome::AbortedBeforeChanges(AbortReason::LockLost),
                "Deploy was aborted before changes: the deploy lock was taken over.",
                "Aborted before changes",
            ),
            (
                DeployOutcome::StoppedInMaintenance(AbortReason::Cancelled),
                "Deploy stopped after maintenance mode was turned on; no deploy steps ran: the run was cancelled.",
                "Stopped in maintenance mode, no steps ran",
            ),
            (
                DeployOutcome::Unknown {
                    step: 3,
                    reason: "connection lost".into(),
                },
                "Outcome of step 3 is unknown: connection lost",
                "Unknown at step 3",
            ),
        ];
        for (outcome, summary, label) in cases {
            assert_eq!(outcome.summary(), summary);
            assert_eq!(outcome.label(), label);
        }
    }
}
