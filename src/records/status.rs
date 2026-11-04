use std::path::{Path, PathBuf};

use serde::{ser::SerializeStruct, Serialize, Serializer};

use super::{Badge, NextSteps, Outcome, ReceiptQueryError, Untrusted};
use crate::receipt::{Receipt, ReceiptKind, ReceiptStepStatus};
use crate::RunPlan;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub repos: Vec<RepoRow>,
    /// Files or directories that could not be read; the rest is still shown.
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoRow {
    pub repo_root: Untrusted,
    /// `None` for a checkout known only from the trust store.
    pub project: Option<Untrusted>,
    pub label: Untrusted,
    /// The checkout is no longer on disk.
    pub path_missing: bool,
    pub envs: Vec<EnvCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvCell {
    pub env: Untrusted,
    /// An approval record exists. It does not mean the current config is
    /// still trusted, which is never checked here.
    pub approved: bool,
    /// `ssh_alias:path` from the approval record.
    pub approved_target: Option<Untrusted>,
    pub code: CodeState,
    /// The newest readable run of any kind.
    pub last_attempt: Option<AttemptSummary>,
    /// The run that decided `code`, when one did. It can be older than
    /// `last_attempt`.
    pub code_run: Option<AttemptSummary>,
    pub stale_hint: Option<StaleHint>,
    /// `ssh_alias:path` of the newest readable run.
    pub target: Option<Untrusted>,
    /// Another checkout ran on the same `ssh_alias:path` after this cell's
    /// last run, so its code may have replaced this one.
    pub newer_elsewhere: Option<NewerElsewhere>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    Recorded,
    Incomplete,
    Observed,
    NotKnown,
    NoCode,
}

impl EnvCell {
    pub fn mark(&self) -> Mark {
        match self.code {
            CodeState::Recorded { complete: true, .. } => Mark::Recorded,
            CodeState::Recorded {
                complete: false, ..
            } => Mark::Incomplete,
            CodeState::Observed { .. } => Mark::Observed,
            CodeState::NotKnown { .. } => Mark::NotKnown,
            CodeState::NeverDeployed
            | CodeState::NoCodeChangeInLast { .. }
            | CodeState::TargetChanged { .. } => Mark::NoCode,
        }
    }
}

impl Serialize for EnvCell {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut cell = serializer.serialize_struct("EnvCell", 11)?;
        cell.serialize_field("env", &self.env)?;
        cell.serialize_field("approved", &self.approved)?;
        cell.serialize_field("approved_target", &self.approved_target)?;
        cell.serialize_field("mark", &self.mark())?;
        cell.serialize_field("code", &self.code)?;
        cell.serialize_field("last_attempt", &self.last_attempt)?;
        cell.serialize_field("code_run", &self.code_run)?;
        cell.serialize_field("stale_hint", &self.stale_hint)?;
        cell.serialize_field("target", &self.target)?;
        cell.serialize_field("newer_elsewhere", &self.newer_elsewhere)?;
        cell.serialize_field("needs_attention", &self.needs_attention())?;
        cell.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewerElsewhere {
    pub repo_root: Untrusted,
    pub env: Untrusted,
    pub run_id: Untrusted,
    pub started_at_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodeState {
    /// From the receipt's own facts.
    Recorded {
        sha: Untrusted,
        /// The run finished and succeeded.
        complete: bool,
        evidence: Evidence,
        run_id: Untrusted,
        at_ms: u128,
    },
    /// The server HEAD read after a failed or unknown step.
    Observed {
        sha: Untrusted,
        dirty: Option<bool>,
        /// The commit the run was deploying.
        expected: Untrusted,
        run_id: Untrusted,
        /// How that run ended, if it did.
        outcome: Option<Outcome>,
        at_ms: u128,
    },
    /// The code may have changed, but nothing recorded says to what.
    NotKnown {
        run_id: Option<Untrusted>,
        reason: NotKnownReason,
    },
    NeverDeployed,
    /// None of this many runs on the current target changed the code.
    NoCodeChangeInLast {
        records: usize,
    },
    /// Runs exist only for an older `ssh_alias:path`.
    TargetChanged {
        run_id: Untrusted,
        target: Untrusted,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// The fast-forward step succeeded.
    FastForward,
    /// A rerun or from-step run checked that the target was checked out.
    CheckedOutAtStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "status", rename_all = "snake_case")]
pub enum NotKnownReason {
    /// The fast-forward started but did not end `Ok`.
    StepZero(ReceiptStepStatus),
    /// A deploy receipt without a fast-forward step.
    NoStepZero,
    /// None of the cell's receipts could be read.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptSummary {
    pub run_id: Untrusted,
    pub started_at_ms: u128,
    pub kind: ReceiptKind,
    pub run_plan: RunPlan,
    pub badge: Badge,
    pub next_steps: NextSteps,
    /// The receipt file.
    pub path: PathBuf,
}

/// A receipt newer than the one that decided the cell could not be read, so
/// the cell may be out of date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StaleHint {
    /// One of this checkout's receipts.
    NewestUnreadable { path: PathBuf },
    /// A file in the same directory whose checkout cannot be told.
    UnattributedNewer { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub path: PathBuf,
    pub message: Untrusted,
    pub kind: ProblemKind,
    /// The cells this problem may make out of date. Empty when it cannot
    /// be tied to any, e.g. for the trust store.
    pub cells: Vec<CellRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    Trust,
    ProjectDir,
    Receipt,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CellRef {
    pub repo_root: Untrusted,
    pub env: Untrusted,
}

impl AttemptSummary {
    pub(crate) fn of(receipt: &Receipt, path: &Path) -> Self {
        Self {
            run_id: receipt.run_id.clone().into(),
            started_at_ms: receipt.started_at_ms,
            kind: receipt.kind,
            run_plan: receipt.run_plan,
            badge: receipt.badge(),
            next_steps: receipt.next_steps(),
            path: path.to_path_buf(),
        }
    }

    fn needs_attention(&self) -> bool {
        self.badge.outcome.needs_attention() || self.badge.flags.iter().any(|flag| flag.is_bad())
    }
}

impl EnvCell {
    /// The approved target, when the newest receipt was for another one.
    pub fn target_moved(&self) -> Option<&str> {
        let approved = self.approved_target.as_ref()?.raw();
        (self.target.as_ref()?.raw() != approved).then_some(approved)
    }

    /// Whether the cell shows something the user should look at: a run
    /// that did not succeed, code that is not known or not complete, a
    /// record that may be out of date, or new errors after a run.
    pub fn needs_attention(&self) -> bool {
        let code = match &self.code {
            CodeState::Recorded { complete, .. } => !complete,
            CodeState::Observed { .. } | CodeState::NotKnown { .. } => true,
            CodeState::NeverDeployed
            | CodeState::NoCodeChangeInLast { .. }
            | CodeState::TargetChanged { .. } => false,
        };
        code || self.stale_hint.is_some()
            || self.newer_elsewhere.is_some()
            || self.target_moved().is_some()
            || self
                .code_run
                .iter()
                .chain(&self.last_attempt)
                .any(AttemptSummary::needs_attention)
    }
}

/// How every front-end orders environments: production, staging, then the
/// rest by name.
pub fn env_order(a: &str, b: &str) -> std::cmp::Ordering {
    let rank = |env: &str| match env {
        "production" => 0,
        "staging" => 1,
        _ => 2,
    };
    rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
}

/// The environments this Mac knows from local receipts and approvals.
/// This read-only query never contacts a server. Times are absolute epoch
/// milliseconds; front-ends supply their own relative wording.
pub fn status(receipts_root: &Path, trust_path: &Path) -> Result<Status, ReceiptQueryError> {
    Ok(crate::receipt::overview(receipts_root, trust_path)?)
}

#[cfg(test)]
mod tests;
