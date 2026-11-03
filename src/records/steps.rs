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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_seen_at_ms: Option<u128>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub connection_gaps: Vec<ConnectionGap>,
    /// Only when both ends were seen.
    pub duration_ms: Option<u128>,
    /// The saved output, colour codes and all.
    pub output: Vec<Untrusted>,
}

impl StepRow {
    pub fn times(&self) -> StepTimes {
        StepTimes {
            result_seen_at_ms: self.result_seen_at_ms,
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
        gaps: &[crate::receipt::ConnectionGap],
    ) -> Self {
        Self {
            kind,
            command: command.into(),
            status: status.into(),
            exit_code,
            started_at_ms: times.started_at_ms,
            finished_at_ms: times.finished_at_ms,
            result_seen_at_ms: times.result_seen_at_ms,
            connection_gaps: gaps
                .iter()
                .map(|gap| ConnectionGap {
                    lost_at_ms: gap.lost_at_ms,
                    reattached_at_ms: gap.reattached_at_ms,
                    detail: gap.detail.clone().into(),
                })
                .collect(),
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
        gaps: &[crate::receipt::ConnectionGap],
    ) -> Self {
        Self::new(
            kind,
            command,
            status.clone().into(),
            exit_code,
            times,
            &[],
            gaps,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectionGap {
    pub lost_at_ms: u128,
    pub reattached_at_ms: Option<u128>,
    pub detail: Untrusted,
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
                &self.maintenance_down_connection_gaps,
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
                &step.connection_gaps,
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
                    &self.maintenance_up_connection_gaps,
                ));
            }
        }
        rows
    }
}
