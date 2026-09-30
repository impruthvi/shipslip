use crate::preflight::AbortReason;

/// Result of a single step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    Ok,
    Failed,
    /// The step's result could not be determined (e.g. the connection was lost).
    Unknown,
    /// The connection failed before the step was sent.
    NotStarted,
}

/// Why a deploy stopped between steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// Stop-after-step, or a detach between steps.
    Requested,
    /// Another run took over the deploy lock.
    LockLost,
    /// The server could not be reached to start or check the next step.
    ConnectFailed(String),
}

/// Final result of a deploy run. Step indexes: 0 is the git fast-forward,
/// 1..=N are recipe steps.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    Unknown {
        step: usize,
        reason: String,
    },
}

/// Progress reported by [`crate::execute`], in order.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The server's checkout after step 0 failed; `None` where it could not
    /// be read.
    ServerState {
        head: Option<String>,
        tree_dirty: Option<bool>,
    },
    Finished(DeployOutcome),
}
