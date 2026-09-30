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

/// Final result of a deploy run. Step indexes: 0 is the git fast-forward,
/// 1..=N are recipe steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployOutcome {
    Succeeded,
    FailedAtStep(usize),
    StoppedAfterStep(usize),
    CancelledBeforeChanges,
    AbortedBeforeChanges(String),
    Unknown { step: usize, reason: String },
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
    Finished(DeployOutcome),
}
