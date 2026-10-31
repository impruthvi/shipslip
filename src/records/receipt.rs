use std::path::Path;

use serde::Serialize;

use super::{Badge, Flag, NextSteps, Outcome, OutcomeKind, StepRow, Untrusted};
use crate::receipt::{self, Found, Receipt, ReceiptError, ReceiptKind};
use crate::{RunPlan, SmokeResult, WatchResult, WatchStatus};

/// The full, read-only record of one run on this Mac.
#[derive(Debug, Serialize)]
pub struct ReceiptRecord {
    pub path: Untrusted,
    pub run_id: Untrusted,
    pub kind: ReceiptKind,
    pub project: Untrusted,
    pub repo_root: Untrusted,
    pub target: Target,
    pub run_plan: RunPlan,
    pub started_at_ms: u128,
    pub finished_at_ms: Option<u128>,
    pub duration_ms: Option<u128>,
    pub started_by: Option<Untrusted>,
    pub started_by_name: Option<Untrusted>,
    pub from_sha: Untrusted,
    pub target_sha: Untrusted,
    pub server_head_at_end: Option<Untrusted>,
    pub tree_dirty: Option<bool>,
    pub commits: Vec<Untrusted>,
    pub more_commits: usize,
    pub recipe_hash: Untrusted,
    pub confirmed: bool,
    pub mutation_started: bool,
    pub outcome: Outcome,
    pub summary: Option<Untrusted>,
    pub flags: Vec<Flag>,
    pub steps: Vec<StepRow>,
    pub watch: Option<Watch>,
    pub smoke: Option<Smoke>,
    pub notes: Vec<Untrusted>,
    pub next_steps: NextSteps,
}

#[derive(Debug, Serialize)]
pub struct Target {
    pub env: Untrusted,
    pub production: bool,
    pub ssh_alias: Untrusted,
    pub path: Untrusted,
    pub branch: Untrusted,
    pub log_path: Untrusted,
}

/// Lookup failures carry raw text, just like successful records.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReceiptQueryError {
    Missing,
    Ambiguous { ids: Vec<String> },
    NewerVersion { version: u32 },
    Unreadable { message: Untrusted },
}

impl From<ReceiptError> for ReceiptQueryError {
    fn from(error: ReceiptError) -> Self {
        match error {
            ReceiptError::Version(version) => Self::NewerVersion { version },
            ReceiptError::Io { ref source, .. }
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Self::Missing
            }
            error => Self::Unreadable {
                message: error.to_string().into(),
            },
        }
    }
}

/// Finds a receipt by its case-insensitive run id prefix, within a project.
pub fn receipt(root: &Path, project: &str, id: &str) -> Result<ReceiptRecord, ReceiptQueryError> {
    match receipt::find(root, project, id)? {
        Found::Missing => Err(ReceiptQueryError::Missing),
        Found::Ambiguous(ids) => Err(ReceiptQueryError::Ambiguous { ids }),
        Found::One(path) => receipt_at(&path),
    }
}

/// Reads an exact receipt locator retained by a front-end. This avoids a
/// second prefix lookup when the desktop map already identified the file.
pub fn receipt_at(path: &Path) -> Result<ReceiptRecord, ReceiptQueryError> {
    receipt::read(path)
        .map(|receipt| ReceiptRecord::from_receipt(path, &receipt))
        .map_err(Into::into)
}

impl ReceiptRecord {
    pub(crate) fn from_receipt(path: &Path, receipt: &Receipt) -> Self {
        let badge = receipt.badge();
        Self {
            path: path.to_string_lossy().into_owned().into(),
            run_id: receipt.run_id.clone().into(),
            kind: receipt.kind,
            project: receipt.project.clone().into(),
            repo_root: receipt.repo_root.clone().into(),
            target: Target {
                env: receipt.target.env.clone().into(),
                production: receipt.target.production,
                ssh_alias: receipt.target.ssh_alias.clone().into(),
                path: receipt.target.path.clone().into(),
                branch: receipt.target.branch.clone().into(),
                log_path: receipt.target.log_path().into(),
            },
            run_plan: receipt.run_plan,
            started_at_ms: receipt.started_at_ms,
            finished_at_ms: receipt.finished_at_ms,
            duration_ms: receipt
                .finished_at_ms
                .map(|end| end.saturating_sub(receipt.started_at_ms)),
            started_by: receipt.started_by(),
            started_by_name: receipt.started_by_name(),
            from_sha: receipt.from_sha.clone().into(),
            target_sha: receipt.target_sha.clone().into(),
            server_head_at_end: receipt.server_head_at_end.clone().map(Into::into),
            tree_dirty: receipt.tree_dirty,
            commits: receipt.commits.iter().cloned().map(Into::into).collect(),
            more_commits: receipt.more_commits,
            recipe_hash: receipt.recipe_hash.clone().into(),
            confirmed: receipt.confirmed,
            mutation_started: receipt.mutation_started,
            outcome: badge.outcome,
            summary: receipt.summary(),
            flags: badge.flags,
            steps: receipt.step_rows(),
            watch: receipt.watch.as_ref().map(Watch::from),
            smoke: receipt.smoke.as_ref().map(Smoke::from),
            notes: receipt
                .last_message
                .iter()
                .chain(&receipt.warnings)
                .cloned()
                .map(Into::into)
                .collect(),
            next_steps: receipt.next_steps(),
        }
    }

    pub fn badge(&self) -> Badge {
        Badge {
            outcome: self.outcome,
            flags: self.flags.clone(),
        }
    }

    pub fn ended_in_failure(&self) -> bool {
        matches!(
            self.outcome.kind,
            OutcomeKind::FailedAtStep { .. }
                | OutcomeKind::StoppedAfterStep { .. }
                | OutcomeKind::Unknown { .. }
        )
    }

    pub fn plan_text(&self) -> String {
        match self.kind {
            ReceiptKind::Deploy => super::describe_plan(self.run_plan),
            ReceiptKind::Up => "bring the app up (`php artisan up`)".into(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Smoke {
    NotConfigured,
    Skipped,
    Passed {
        status: u16,
        latency_ms: u64,
    },
    Failed {
        status: Option<u16>,
        reason: Untrusted,
    },
}

impl From<&SmokeResult> for Smoke {
    fn from(result: &SmokeResult) -> Self {
        match result {
            SmokeResult::NotConfigured => Self::NotConfigured,
            SmokeResult::Skipped => Self::Skipped,
            SmokeResult::Passed { status, latency_ms } => Self::Passed {
                status: *status,
                latency_ms: *latency_ms,
            },
            SmokeResult::Failed { status, reason } => Self::Failed {
                status: *status,
                reason: reason.clone().into(),
            },
        }
    }
}

impl Smoke {
    pub fn text(&self) -> String {
        match self {
            Self::NotConfigured => "not configured".into(),
            Self::Skipped => "skipped".into(),
            Self::Passed { status, latency_ms } => {
                format!("passed, HTTP {status} in {latency_ms} ms")
            }
            Self::Failed {
                status: Some(status),
                reason,
            } if reason.raw() == format!("HTTP {status}") => format!("failed, HTTP {status}"),
            Self::Failed {
                status: Some(status),
                reason,
            } => format!("failed, HTTP {status}: {}", reason.field()),
            Self::Failed {
                status: None,
                reason,
            } => format!("failed: {}", reason.field()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Watch {
    pub status: WatchStatus,
    pub log_path: Option<Untrusted>,
    pub duration_ms: u64,
    pub baseline_signatures: usize,
    pub history_signatures: usize,
    pub observed_lines: u64,
    pub parsed_lines: u64,
    pub dropped_view_lines: u64,
    pub truncated_entries: u64,
    pub new_errors: Vec<ErrorGroup>,
    pub overflow_groups: u64,
    pub overflow_signatures: u64,
    pub warnings: Vec<Untrusted>,
}

impl From<&WatchResult> for Watch {
    fn from(value: &WatchResult) -> Self {
        Self {
            status: value.status,
            log_path: value.log_path.clone().map(Into::into),
            duration_ms: value.duration_ms,
            baseline_signatures: value.baseline_signatures,
            history_signatures: value.history_signatures,
            observed_lines: value.observed_lines,
            parsed_lines: value.parsed_lines,
            dropped_view_lines: value.dropped_view_lines,
            truncated_entries: value.truncated_entries,
            new_errors: value.new_errors.iter().map(Into::into).collect(),
            overflow_groups: value.overflow_groups,
            overflow_signatures: value.overflow_signatures,
            warnings: value
                .warnings
                .iter()
                .map(|text| text.clone().into())
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorGroup {
    pub exception: Untrusted,
    pub file: Option<Untrusted>,
    pub count: u64,
    pub variants: Vec<ErrorVariant>,
    pub overflow_variants: u64,
}

impl From<&crate::ErrorGroup> for ErrorGroup {
    fn from(value: &crate::ErrorGroup) -> Self {
        Self {
            exception: value.exception.clone().into(),
            file: value.file.clone().map(Into::into),
            count: value.count,
            variants: value.variants.iter().map(Into::into).collect(),
            overflow_variants: value.overflow_variants,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorVariant {
    pub signature: Untrusted,
    pub level: Untrusted,
    pub message: Untrusted,
    pub display_file_line: Option<Untrusted>,
    pub count: u64,
    pub phase: crate::LogPhase,
}

impl From<&crate::ErrorVariant> for ErrorVariant {
    fn from(value: &crate::ErrorVariant) -> Self {
        Self {
            signature: value.signature.clone().into(),
            level: value.level.clone().into(),
            message: value.message.clone().into(),
            display_file_line: value.display_file_line.clone().map(Into::into),
            count: value.count,
            phase: value.phase,
        }
    }
}
