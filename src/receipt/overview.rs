//! The release map: for each checkout and environment, which code the saved
//! receipts say is on the server. Reads only local files.
//!
//! A commit is only reported with a recorded or observed fact behind it:
//! the fast-forward step's own status, a rerun's checked-out commit, or the
//! server HEAD read after a failed step. Nothing here opens `.lock` files.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::history::{entries_in, Entry};
use super::{io_error, read_receipt, safe_component, Badge, Receipt, ReceiptError, ReceiptStatus};
use super::{ReceiptStepStatus, ReceiptSummary};
use crate::git::{Comparison, Git};
use crate::{DeployOutcome, RunPlan};

/// Receipts of one cell read before it settles on "no code change".
pub const WALK_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    pub repos: Vec<RepoRow>,
    /// Files or directories that could not be read; the rest is still shown.
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRow {
    pub repo_root: String,
    /// `None` for a checkout known only from the trust store.
    pub project: Option<String>,
    pub label: String,
    /// The checkout is no longer on disk.
    pub path_missing: bool,
    pub envs: Vec<EnvCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvCell {
    pub env: String,
    /// An approval record exists. It does not mean the current config is
    /// still trusted, which is never checked here.
    pub approved: bool,
    pub code: CodeState,
    /// The newest readable run of any kind.
    pub last_attempt: Option<AttemptSummary>,
    pub stale_hint: Option<StaleHint>,
    /// `ssh_alias:path` of the newest readable run.
    pub target: Option<String>,
    /// Another checkout ran on the same `ssh_alias:path` after this cell's
    /// last run, so its code may have replaced this one.
    pub newer_elsewhere: Option<NewerElsewhere>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewerElsewhere {
    pub repo_root: String,
    pub env: String,
    pub run_id: String,
    pub started_at_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeState {
    /// From the receipt's own facts.
    Recorded {
        sha: String,
        /// The run finished and succeeded.
        complete: bool,
        evidence: Evidence,
        run_id: String,
        at_ms: u128,
    },
    /// The server HEAD read after a failed or unknown step.
    Observed {
        sha: String,
        dirty: Option<bool>,
        /// The commit the run was deploying.
        expected: String,
        run_id: String,
        at_ms: u128,
    },
    /// The code may have changed, but nothing recorded says to what.
    NotKnown {
        run_id: Option<String>,
        reason: NotKnownReason,
    },
    NeverDeployed,
    /// None of this many runs on the current target changed the code.
    NoCodeChangeInLast {
        records: usize,
    },
    /// Runs exist only for an older `ssh_alias:path`.
    TargetChanged {
        run_id: String,
        target: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// The fast-forward step succeeded.
    FastForward,
    /// A rerun or from-step run checked that the target was checked out.
    CheckedOutAtStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotKnownReason {
    /// The fast-forward started but did not end `Ok`.
    StepZero(ReceiptStepStatus),
    /// A deploy receipt without a fast-forward step.
    NoStepZero,
    /// None of the cell's receipts could be read.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptSummary {
    pub run_id: String,
    pub started_at_ms: u128,
    pub run_plan: RunPlan,
    pub badge: Badge,
}

/// A receipt newer than the one that decided the cell could not be read, so
/// the cell may be out of date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleHint {
    /// One of this checkout's receipts.
    NewestUnreadable { path: PathBuf },
    /// A file in the same directory whose checkout cannot be told.
    UnattributedNewer { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    pub message: String,
}

impl Problem {
    fn from_error(fallback: &Path, error: &ReceiptError) -> Self {
        let path = match error {
            ReceiptError::Io { path, .. } | ReceiptError::Parse { path, .. } => path.clone(),
            _ => fallback.to_path_buf(),
        };
        Self {
            path,
            message: error.to_string(),
        }
    }
}

/// One receipt file attributed to a checkout.
struct Own {
    project_dir: PathBuf,
    started_at_ms: u128,
    path: PathBuf,
}

#[derive(Default)]
struct RowBuild {
    project: Option<String>,
    /// Keyed by the encoded env name.
    envs: BTreeMap<String, CellBuild>,
}

#[derive(Default)]
struct CellBuild {
    raw_env: Option<String>,
    approved: bool,
    owns: Vec<Own>,
}

/// Builds the release map. Only an existing receipts root that cannot be
/// read is an error; anything else unreadable becomes a [`Problem`].
pub fn overview(receipts_root: &Path, trust_path: &Path) -> Result<Overview, ReceiptError> {
    let mut problems = Vec::new();
    let mut rows: BTreeMap<String, RowBuild> = BTreeMap::new();
    // Files no checkout can claim, by (project dir, encoded env).
    let mut unattributed: BTreeMap<(PathBuf, String), Vec<(u128, PathBuf)>> = BTreeMap::new();

    for project_dir in project_dirs(receipts_root)? {
        let (entries, dir_problems) = entries_in(&project_dir);
        problems.extend(
            dir_problems
                .iter()
                .map(|error| Problem::from_error(&project_dir, error)),
        );
        for entry in entries {
            let Entry {
                env,
                started_at_ms: Some(started_at_ms),
                path,
                ..
            } = entry
            else {
                // Not a `<ms>-<run_id>.json` receipt, e.g. legacy history.
                continue;
            };
            match read_summary(&path) {
                Ok(summary) => {
                    let row = rows.entry(summary.repo_root).or_default();
                    if row.project.is_none() {
                        row.project = summary.project;
                    }
                    row.envs.entry(env).or_default().owns.push(Own {
                        project_dir: project_dir.clone(),
                        started_at_ms,
                        path,
                    });
                }
                Err(error) => {
                    problems.push(Problem::from_error(&path, &error));
                    unattributed
                        .entry((project_dir.clone(), env))
                        .or_default()
                        .push((started_at_ms, path));
                }
            }
        }
    }

    match crate::config::approved_envs(trust_path) {
        Ok(approved) => {
            for (repo_root, envs) in approved {
                let row = rows.entry(repo_root).or_default();
                for env in envs {
                    let cell = row.envs.entry(safe_component(&env)).or_default();
                    cell.approved = true;
                    cell.raw_env = Some(env);
                }
            }
        }
        Err(error) => problems.push(Problem {
            path: trust_path.to_path_buf(),
            message: error.to_string(),
        }),
    }

    let mut repos: Vec<RepoRow> = rows
        .into_iter()
        .map(|(repo_root, row)| {
            let mut envs: Vec<EnvCell> = row
                .envs
                .into_iter()
                .map(|(key, cell)| build_cell(&key, cell, &unattributed))
                .collect();
            envs.sort_by(|a, b| a.env.cmp(&b.env));
            let label = row.project.clone().unwrap_or_else(|| {
                Path::new(&repo_root)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| repo_root.clone())
            });
            RepoRow {
                path_missing: !Path::new(&repo_root).is_dir(),
                project: row.project,
                label,
                repo_root,
                envs,
            }
        })
        .collect();
    repos.sort_by(|a, b| {
        a.label
            .cmp(&b.label)
            .then_with(|| a.repo_root.cmp(&b.repo_root))
    });
    mark_newer_elsewhere(&mut repos);
    Ok(Overview { repos, problems })
}

/// Cells are per checkout, but two checkouts can deploy to one server path.
fn mark_newer_elsewhere(repos: &mut [RepoRow]) {
    let mut runs: BTreeMap<String, Vec<NewerElsewhere>> = BTreeMap::new();
    for row in repos.iter() {
        for cell in &row.envs {
            if let (Some(target), Some(attempt)) = (&cell.target, &cell.last_attempt) {
                runs.entry(target.clone())
                    .or_default()
                    .push(NewerElsewhere {
                        repo_root: row.repo_root.clone(),
                        env: cell.env.clone(),
                        run_id: attempt.run_id.clone(),
                        started_at_ms: attempt.started_at_ms,
                    });
            }
        }
    }
    for row in repos.iter_mut() {
        for cell in &mut row.envs {
            let (Some(target), Some(attempt)) = (&cell.target, &cell.last_attempt) else {
                continue;
            };
            cell.newer_elsewhere = runs
                .get(target)
                .into_iter()
                .flatten()
                .filter(|other| {
                    other.repo_root != row.repo_root && other.started_at_ms > attempt.started_at_ms
                })
                .max_by_key(|other| other.started_at_ms)
                .cloned();
        }
    }
}

fn project_dirs(root: &Path) -> Result<Vec<PathBuf>, ReceiptError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_error(root, source)),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let path = entry.map_err(|source| io_error(root, source))?.path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn read_summary(path: &Path) -> Result<ReceiptSummary, ReceiptError> {
    let bytes = fs::read(path).map_err(|source| io_error(path, source))?;
    serde_json::from_slice(&bytes).map_err(|source| ReceiptError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn build_cell(
    key: &str,
    mut cell: CellBuild,
    unattributed: &BTreeMap<(PathBuf, String), Vec<(u128, PathBuf)>>,
) -> EnvCell {
    // A renamed project leaves one checkout's receipts in two directories.
    cell.owns.sort_by(|a, b| {
        b.started_at_ms
            .cmp(&a.started_at_ms)
            .then_with(|| b.path.cmp(&a.path))
    });
    let walk = walk(&cell.owns);
    let mut stale_hint = walk.stale_hint;
    if stale_hint.is_none() {
        // Any unclaimed file newer than the deciding run may be this
        // checkout's newest receipt.
        stale_hint = cell
            .owns
            .iter()
            .map(|own| &own.project_dir)
            .filter_map(|dir| unattributed.get(&(dir.clone(), key.to_string())))
            .flatten()
            .filter(|(started, _)| walk.decided_at_ms.is_none_or(|decided| *started > decided))
            .max_by_key(|(started, _)| *started)
            .map(|(_, path)| StaleHint::UnattributedNewer { path: path.clone() });
    }
    EnvCell {
        env: walk.env.or(cell.raw_env).unwrap_or_else(|| key.to_string()),
        approved: cell.approved,
        code: walk.code,
        last_attempt: walk.last_attempt,
        stale_hint,
        target: walk.target,
        newer_elsewhere: None,
    }
}

struct Walk {
    code: CodeState,
    last_attempt: Option<AttemptSummary>,
    stale_hint: Option<StaleHint>,
    target: Option<String>,
    env: Option<String>,
    /// Start time of the run that decided `code`.
    decided_at_ms: Option<u128>,
}

/// Newest first, the first run on the current target that moved or
/// observed the code decides the cell.
fn walk(owns: &[Own]) -> Walk {
    let mut result = Walk {
        code: CodeState::NeverDeployed,
        last_attempt: None,
        stale_hint: None,
        target: None,
        env: None,
        decided_at_ms: None,
    };
    let mut current: Option<(String, String)> = None;
    let mut counted = 0;
    let mut unreadable = 0;
    let mut first_on_target: Option<String> = None;
    let mut saw_other_target = false;
    for own in owns {
        if counted == WALK_LIMIT {
            result.code = CodeState::NoCodeChangeInLast { records: counted };
            return result;
        }
        let receipt = match read_receipt(&own.path) {
            Ok(receipt) => receipt,
            Err(_) => {
                // Listed as a problem by `slip receipts`; here it only marks
                // the cell as possibly stale.
                result
                    .stale_hint
                    .get_or_insert(StaleHint::NewestUnreadable {
                        path: own.path.clone(),
                    });
                unreadable += 1;
                continue;
            }
        };
        let target = (
            receipt.target.ssh_alias.clone(),
            receipt.target.path.clone(),
        );
        if current.is_none() {
            result.target = Some(format!("{}:{}", target.0, target.1));
            result.env = Some(receipt.target.env.clone());
            result.last_attempt = Some(AttemptSummary {
                run_id: receipt.run_id.clone(),
                started_at_ms: receipt.started_at_ms,
                run_plan: receipt.run_plan,
                badge: receipt.badge(),
            });
            current = Some(target.clone());
        }
        if current.as_ref() != Some(&target) {
            saw_other_target = true;
            continue;
        }
        counted += 1;
        first_on_target = Some(receipt.run_id.clone());
        if let Some(code) = code_of(&receipt) {
            result.code = code;
            result.decided_at_ms = Some(receipt.started_at_ms);
            return result;
        }
    }
    result.code = match (current, first_on_target) {
        (Some((alias, path)), Some(run_id)) if saw_other_target => CodeState::TargetChanged {
            run_id,
            target: format!("{alias}:{path}"),
        },
        (Some(_), _) => CodeState::NoCodeChangeInLast { records: counted },
        (None, _) if unreadable > 0 => CodeState::NotKnown {
            run_id: None,
            reason: NotKnownReason::Unreadable,
        },
        (None, _) => CodeState::NeverDeployed,
    };
    result
}

/// What one run says about the server's code, or `None` when it did not
/// change it.
fn code_of(receipt: &Receipt) -> Option<CodeState> {
    let run_id = receipt.run_id.clone();
    let at_ms = receipt.finished_at_ms.unwrap_or(receipt.started_at_ms);
    if let Some(head) = &receipt.server_head_at_end {
        return Some(CodeState::Observed {
            sha: head.clone(),
            dirty: receipt.tree_dirty,
            expected: receipt.target_sha.clone(),
            run_id,
            at_ms,
        });
    }
    let complete =
        receipt.status == ReceiptStatus::Final && receipt.outcome == Some(DeployOutcome::Succeeded);
    let recorded = |evidence| CodeState::Recorded {
        sha: receipt.target_sha.clone(),
        complete,
        evidence,
        run_id: run_id.clone(),
        at_ms,
    };
    match receipt.run_plan {
        RunPlan::Deploy => {
            let Some(step) = receipt.steps.iter().find(|step| step.index == 0) else {
                return Some(CodeState::NotKnown {
                    run_id: Some(run_id),
                    reason: NotKnownReason::NoStepZero,
                });
            };
            match step.status {
                ReceiptStepStatus::Ok => Some(recorded(Evidence::FastForward)),
                ReceiptStepStatus::Running
                | ReceiptStepStatus::Failed
                | ReceiptStepStatus::Unknown => Some(CodeState::NotKnown {
                    run_id: Some(run_id),
                    reason: NotKnownReason::StepZero(step.status),
                }),
                ReceiptStepStatus::Pending
                | ReceiptStepStatus::NotStarted
                | ReceiptStepStatus::Skipped => None,
            }
        }
        // Preflight refuses these unless the target is checked out, so any
        // run that got past it recorded that commit.
        RunPlan::Rerun | RunPlan::FromStep(_) => receipt
            .mutation_started
            .then(|| recorded(Evidence::CheckedOutAtStart)),
    }
}

/// Something about one side that the commit count alone would hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qualifier {
    DeployIncomplete,
    TreeDirty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvComparison {
    /// Env `a` relative to env `b`.
    pub result: Comparison,
    /// Env name and what to say next to the count.
    pub notes: Vec<(String, Qualifier)>,
}

/// Compares the code two envs of one checkout are on. Only recorded or
/// observed commits are compared.
pub fn compare_envs(git: &Git, row: &RepoRow, a: &str, b: &str) -> EnvComparison {
    let mut notes = Vec::new();
    let unavailable = |reason: String| EnvComparison {
        result: Comparison::Unavailable(reason),
        notes: Vec::new(),
    };
    let mut shas = Vec::new();
    for name in [a, b] {
        let Some(cell) = row
            .envs
            .iter()
            .find(|cell| safe_component(&cell.env) == safe_component(name))
        else {
            return unavailable(format!("no {name} environment"));
        };
        match &cell.code {
            CodeState::Recorded { sha, complete, .. } => {
                if !complete {
                    notes.push((cell.env.clone(), Qualifier::DeployIncomplete));
                }
                shas.push(sha.clone());
            }
            CodeState::Observed { sha, dirty, .. } => {
                if *dirty == Some(true) {
                    notes.push((cell.env.clone(), Qualifier::TreeDirty));
                }
                shas.push(sha.clone());
            }
            _ => return unavailable(format!("no recorded code on {}", cell.env)),
        }
    }
    if row.path_missing {
        return unavailable("checkout not found".into());
    }
    EnvComparison {
        result: git.compare(Path::new(&row.repo_root), &shas[0], &shas[1]),
        notes,
    }
}

#[cfg(test)]
mod tests;
