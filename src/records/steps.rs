use serde::Serialize;

use super::Untrusted;
use crate::receipt::{Receipt, ReceiptKind, ReceiptStepStatus};
use crate::{StepStatus, StepTimes};

/// One row of what a run did, in run order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepRow {
    #[serde(flatten)]
    pub kind: StepKind,
    pub command: Untrusted,
    pub status: StepState,
    pub exit_code: Option<i32>,
    /// This Mac's clock when it sent the step.
    pub started_at_ms: Option<u128>,
    /// When this Mac saw the step end; `None` when it did not.
    pub finished_at_ms: Option<u128>,
    /// Only when both ends were seen.
    pub duration_ms: Option<u128>,
    /// The saved output, colour codes and all.
    pub output: Vec<Untrusted>,
}

impl StepRow {
    pub fn times(&self) -> StepTimes {
        StepTimes {
            started_at_ms: self.started_at_ms,
            finished_at_ms: self.finished_at_ms,
        }
    }

    fn new(
        kind: StepKind,
        command: &str,
        status: ReceiptStepStatus,
        exit_code: Option<i32>,
        times: StepTimes,
        output: &[String],
    ) -> Self {
        Self {
            kind,
            command: command.into(),
            status: status.into(),
            exit_code,
            started_at_ms: times.started_at_ms,
            finished_at_ms: times.finished_at_ms,
            duration_ms: times.duration_ms(),
            output: output.iter().map(|line| line.as_str().into()).collect(),
        }
    }

    fn maintenance(
        kind: StepKind,
        command: &str,
        status: &StepStatus,
        exit_code: Option<i32>,
        times: StepTimes,
    ) -> Self {
        Self::new(kind, command, status.clone().into(), exit_code, times, &[])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepKind {
    MaintenanceDown,
    /// Step 0 is the fast-forward; recipe steps follow.
    Step {
        index: usize,
    },
    MaintenanceUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Pending,
    Running,
    Ok,
    Failed,
    Unknown,
    NotStarted,
    Skipped,
}

impl From<ReceiptStepStatus> for StepState {
    fn from(status: ReceiptStepStatus) -> Self {
        match status {
            ReceiptStepStatus::Pending => Self::Pending,
            ReceiptStepStatus::Running => Self::Running,
            ReceiptStepStatus::Ok => Self::Ok,
            ReceiptStepStatus::Failed => Self::Failed,
            ReceiptStepStatus::Unknown => Self::Unknown,
            ReceiptStepStatus::NotStarted => Self::NotStarted,
            ReceiptStepStatus::Skipped => Self::Skipped,
        }
    }
}

impl StepState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "✓ ok",
            Self::Failed => "✗ failed",
            Self::Unknown => "? unknown",
            Self::NotStarted | Self::Pending => "– not run",
            Self::Running => "? running when saved",
            Self::Skipped => "↷ skipped",
        }
    }
}

impl Receipt {
    /// Maintenance down, each step, then maintenance up. An Up run's only
    /// step is `php artisan up`, so it has no maintenance-up row.
    pub fn step_rows(&self) -> Vec<StepRow> {
        let mut rows = Vec::new();
        if let Some(status) = &self.maintenance_down_status {
            rows.push(StepRow::maintenance(
                StepKind::MaintenanceDown,
                "php artisan down",
                status,
                self.maintenance_down_exit_code,
                self.maintenance_down_times(),
            ));
        }
        rows.extend(self.steps.iter().map(|step| {
            StepRow::new(
                StepKind::Step { index: step.index },
                &step.command,
                step.status,
                step.exit_code,
                step.times(),
                &step.output,
            )
        }));
        if self.kind == ReceiptKind::Deploy {
            if let Some(status) = &self.maintenance_up_status {
                rows.push(StepRow::maintenance(
                    StepKind::MaintenanceUp,
                    "php artisan up",
                    status,
                    self.maintenance_up_exit_code,
                    self.maintenance_up_times(),
                ));
            }
        }
        rows
    }
}
