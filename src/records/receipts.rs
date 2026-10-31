use std::path::Path;

use serde::Serialize;

use super::{Flag, Outcome, ReceiptQueryError, StepRow, Untrusted};
use crate::receipt::{self, Listed, Receipt, ReceiptKind};
use crate::RunPlan;

#[derive(Debug, Serialize)]
pub struct ReceiptList {
    pub rows: Vec<ReceiptListRow>,
    pub older: usize,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReceiptListRow {
    Receipt {
        id: Untrusted,
        path: Untrusted,
        receipt: Box<ReceiptSummary>,
    },
    Newer {
        id: Untrusted,
        env: Untrusted,
        started_at_ms: Option<u128>,
        version: u32,
        path: Untrusted,
    },
    Unreadable {
        id: Untrusted,
        env: Untrusted,
        started_at_ms: Option<u128>,
        path: Untrusted,
        reason: Untrusted,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiptSummary {
    pub run_id: Untrusted,
    pub env: Untrusted,
    pub repo_root: Untrusted,
    pub started_at_ms: u128,
    pub kind: ReceiptKind,
    pub run_plan: RunPlan,
    pub from_sha: Untrusted,
    pub target_sha: Untrusted,
    pub outcome: Outcome,
    pub flags: Vec<Flag>,
    pub app_left_down: bool,
    pub recipe_steps: usize,
    pub steps: Vec<StepRow>,
}

impl From<&Receipt> for ReceiptSummary {
    fn from(receipt: &Receipt) -> Self {
        let badge = receipt.badge();
        Self {
            run_id: receipt.run_id.clone().into(),
            env: receipt.target.env.clone().into(),
            repo_root: receipt.repo_root.clone().into(),
            started_at_ms: receipt.started_at_ms,
            kind: receipt.kind,
            run_plan: receipt.run_plan,
            from_sha: receipt.from_sha.clone().into(),
            target_sha: receipt.target_sha.clone().into(),
            outcome: badge.outcome,
            flags: badge.flags,
            app_left_down: receipt.app_left_down,
            recipe_steps: receipt.target.steps.len(),
            steps: receipt.step_rows(),
        }
    }
}

impl From<receipt::Listing> for ReceiptList {
    fn from(listing: receipt::Listing) -> Self {
        Self {
            older: listing.older,
            rows: listing.rows.into_iter().map(ReceiptListRow::from).collect(),
        }
    }
}

impl From<Listed> for ReceiptListRow {
    fn from(row: Listed) -> Self {
        match row {
            Listed::Receipt { id, path, receipt } => Self::Receipt {
                id: id.into(),
                path: path.to_string_lossy().into_owned().into(),
                receipt: Box::new(ReceiptSummary::from(receipt.as_ref())),
            },
            Listed::Newer {
                id,
                env,
                started_at_ms,
                version,
                path,
            } => Self::Newer {
                id: id.into(),
                env: env.into(),
                started_at_ms,
                version,
                path: path.to_string_lossy().into_owned().into(),
            },
            Listed::Unreadable {
                id,
                env,
                started_at_ms,
                path,
                reason,
            } => Self::Unreadable {
                id: id.into(),
                env: env.into(),
                started_at_ms,
                path: path.to_string_lossy().into_owned().into(),
                reason: reason.into(),
            },
        }
    }
}

/// Lists local runs newest first. The limit bounds the files read before
/// filtering by checkout. With a checkout, an unreadable or newer receipt
/// ends the search: older runs cannot safely describe what needs recovery.
pub fn receipts(
    root: &Path,
    project: &str,
    env: Option<&str>,
    limit: Option<usize>,
    checkout: Option<&Path>,
) -> Result<ReceiptList, ReceiptQueryError> {
    let mut listing = receipt::list(root, project, env, limit)?;
    if let Some(checkout) = checkout {
        let checkout = checkout.to_string_lossy();
        let mut blocked = false;
        listing.rows.retain(|row| {
            if blocked {
                return false;
            }
            match row {
                Listed::Receipt { receipt, .. } => receipt.repo_root == checkout,
                _ => {
                    blocked = true;
                    true
                }
            }
        });
    }
    Ok(listing.into())
}
