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
use super::{
    io_error, read_receipt, safe_component, Receipt, ReceiptError, ReceiptKind, ReceiptStatus,
};
use super::{ReceiptStepStatus, ReceiptSummary};
use crate::git::{Comparison, Git};
use crate::records::{env_order, Flag, Untrusted};
use crate::{DeployOutcome, RunPlan};

/// Receipts of one cell read before it settles on "no code change".
pub const WALK_LIMIT: usize = 20;

pub use crate::records::{
    AttemptSummary, CellRef, CodeState, EnvCell, Evidence, NewerElsewhere, NotKnownReason, Problem,
    ProblemKind, RepoRow, StaleHint, Status as Overview,
};

/// Where a problem was found, to work out the cells it touches.
struct Scope {
    project_dir: PathBuf,
    /// Encoded env name; `None` for the whole project directory.
    env: Option<String>,
}

impl Problem {
    fn from_error(fallback: &Path, error: &ReceiptError, kind: ProblemKind) -> Self {
        let path = match error {
            ReceiptError::Io { path, .. } | ReceiptError::Parse { path, .. } => path.clone(),
            _ => fallback.to_path_buf(),
        };
        Self {
            path,
            message: error.to_string().into(),
            kind,
            cells: Vec::new(),
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
    approved_target: Option<String>,
    owns: Vec<Own>,
}

/// Builds the release map. Only an existing receipts root that cannot be
/// read is an error; anything else unreadable becomes a [`Problem`].
pub fn overview(receipts_root: &Path, trust_path: &Path) -> Result<Overview, ReceiptError> {
    let mut problems: Vec<(Problem, Option<Scope>)> = Vec::new();
    let mut rows: BTreeMap<String, RowBuild> = BTreeMap::new();
    // Files no checkout can claim, by (project dir, encoded env).
    let mut unattributed: BTreeMap<(PathBuf, String), Vec<(u128, PathBuf)>> = BTreeMap::new();

    for project_dir in project_dirs(receipts_root)? {
        let (entries, dir_problems) = entries_in(&project_dir);
        problems.extend(dir_problems.iter().map(|error| {
            let problem = Problem::from_error(&project_dir, error, ProblemKind::ProjectDir);
            // An unreadable env directory only touches that env's cells.
            let env = problem
                .path
                .strip_prefix(&project_dir)
                .ok()
                .and_then(|rest| rest.iter().next())
                .map(|env| env.to_string_lossy().into_owned());
            let scope = Scope {
                project_dir: project_dir.clone(),
                env,
            };
            (problem, Some(scope))
        }));
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
                    let scope = Scope {
                        project_dir: project_dir.clone(),
                        env: Some(env.clone()),
                    };
                    problems.push((
                        Problem::from_error(&path, &error, ProblemKind::Receipt),
                        Some(scope),
                    ));
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
                for (env, target) in envs {
                    let cell = row.envs.entry(safe_component(&env)).or_default();
                    cell.approved = true;
                    cell.approved_target = Some(target);
                    cell.raw_env = Some(env);
                }
            }
        }
        Err(error) => problems.push((
            Problem {
                path: trust_path.to_path_buf(),
                message: error.to_string().into(),
                kind: ProblemKind::Trust,
                cells: Vec::new(),
            },
            None,
        )),
    }

    // Which cells read receipts from each project dir and env.
    let mut readers: BTreeMap<(PathBuf, String), Vec<CellRef>> = BTreeMap::new();

    let mut repos: Vec<RepoRow> = rows
        .into_iter()
        .map(|(repo_root, row)| {
            let mut envs: Vec<EnvCell> = Vec::new();
            for (key, cell) in row.envs {
                let dirs: Vec<PathBuf> = cell
                    .owns
                    .iter()
                    .map(|own| own.project_dir.clone())
                    .collect();
                let built = build_cell(&key, cell, &unattributed);
                for dir in dirs {
                    let refs = readers.entry((dir, key.clone())).or_default();
                    let cell_ref = CellRef {
                        repo_root: repo_root.clone().into(),
                        env: built.env.clone(),
                    };
                    if !refs.contains(&cell_ref) {
                        refs.push(cell_ref);
                    }
                }
                envs.push(built);
            }
            envs.sort_by(|a, b| env_order(a.env.raw(), b.env.raw()));
            let label = row.project.clone().unwrap_or_else(|| {
                Path::new(&repo_root)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| repo_root.clone())
            });
            RepoRow {
                path_missing: !Path::new(&repo_root).is_dir(),
                project: row.project.map(Into::into),
                label: label.into(),
                repo_root: repo_root.into(),
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
    let problems = problems
        .into_iter()
        .map(|(mut problem, scope)| {
            if let Some(scope) = scope {
                let mut cells: Vec<CellRef> = readers
                    .iter()
                    .filter(|((dir, env), _)| {
                        *dir == scope.project_dir && scope.env.as_ref().is_none_or(|key| key == env)
                    })
                    .flat_map(|(_, refs)| refs.iter().cloned())
                    .collect();
                cells.sort();
                cells.dedup();
                problem.cells = cells;
            }
            problem
        })
        .collect();
    Ok(Overview { repos, problems })
}

/// Cells are per checkout, but two checkouts can deploy to one server path.
/// The newest run of a cell that could have changed the code: `slip up`
/// never does.
fn last_deploy(cell: &EnvCell) -> Option<&AttemptSummary> {
    cell.last_attempt
        .as_ref()
        .filter(|attempt| attempt.kind == ReceiptKind::Deploy)
        .or(cell.code_run.as_ref())
}

fn mark_newer_elsewhere(repos: &mut [RepoRow]) {
    let mut runs: BTreeMap<Untrusted, Vec<NewerElsewhere>> = BTreeMap::new();
    for row in repos.iter() {
        for cell in &row.envs {
            if let (Some(target), Some(attempt)) = (&cell.target, last_deploy(cell)) {
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
            let (Some(target), Some(attempt)) = (&cell.target, last_deploy(cell)) else {
                continue;
            };
            let started_at_ms = attempt.started_at_ms;
            cell.newer_elsewhere = runs
                .get(target)
                .into_iter()
                .flatten()
                .filter(|other| {
                    other.repo_root != row.repo_root && other.started_at_ms > started_at_ms
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
        env: walk
            .env
            .or(cell.raw_env)
            .unwrap_or_else(|| key.to_string())
            .into(),
        approved: cell.approved,
        approved_target: cell.approved_target.map(Into::into),
        code: walk.code,
        last_attempt: walk.last_attempt,
        code_run: walk.code_run,
        stale_hint,
        target: walk.target.map(Into::into),
        newer_elsewhere: None,
    }
}

struct Walk {
    code: CodeState,
    last_attempt: Option<AttemptSummary>,
    code_run: Option<AttemptSummary>,
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
        code_run: None,
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
    // A newer successful `slip up` turned maintenance mode off.
    let mut brought_up = false;
    // Smoke evidence is independent of the run that established the code.
    let mut smoke_flags = None;
    for own in owns {
        if counted == WALK_LIMIT {
            break;
        }
        let receipt = match read_receipt(&own.path) {
            Ok(receipt) => receipt,
            Err(_) => {
                // Listed as a problem by `slip receipts`; here it only marks
                // the cell as possibly stale.
                if result.code_run.is_none() {
                    result
                        .stale_hint
                        .get_or_insert(StaleHint::NewestUnreadable {
                            path: own.path.clone(),
                        });
                }
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
            result.last_attempt = Some(AttemptSummary::of(&receipt, &own.path));
            current = Some(target.clone());
        }
        if current.as_ref() != Some(&target) {
            saw_other_target = true;
            continue;
        }
        counted += 1;
        first_on_target = Some(receipt.run_id.clone());
        if smoke_flags.is_none()
            && matches!(
                receipt.smoke,
                Some(
                    crate::SmokeResult::Passed { .. }
                        | crate::SmokeResult::Failed { .. }
                        | crate::SmokeResult::Skipped
                )
            )
        {
            smoke_flags = Some(
                receipt
                    .badge()
                    .flags
                    .into_iter()
                    .filter(|flag| matches!(flag, Flag::SmokeFailed { .. }))
                    .collect::<Vec<_>>(),
            );
        }
        brought_up |= receipt.kind == ReceiptKind::Up
            && receipt.status == ReceiptStatus::Final
            && receipt.outcome == Some(DeployOutcome::Succeeded);
        if let Some(code) = code_of(&receipt).filter(|_| result.code_run.is_none()) {
            let mut code_run = AttemptSummary::of(&receipt, &own.path);
            if brought_up {
                code_run
                    .badge
                    .flags
                    .retain(|flag| *flag != Flag::AppLeftDown);
                code_run.next_steps.steps.retain(|step| !step.runs("up"));
            }
            result.code = code;
            result.code_run = Some(code_run);
            result.decided_at_ms = Some(receipt.started_at_ms);
        }
        if result.code_run.is_some() && smoke_flags.is_some() {
            break;
        }
    }
    // Map summaries carry the current smoke signal; the receipts themselves
    // retain each run's original result.
    if let Some(smoke_flags) = smoke_flags {
        for attempt in result
            .code_run
            .iter_mut()
            .chain(result.last_attempt.iter_mut())
        {
            attempt
                .badge
                .flags
                .retain(|flag| !matches!(flag, Flag::SmokeFailed { .. } | Flag::SmokeNotRecorded));
            attempt.badge.flags.extend(smoke_flags.iter().cloned());
        }
    }
    if result.code_run.is_some() {
        return result;
    }
    result.code = match (current, first_on_target) {
        (Some((alias, path)), Some(run_id)) if saw_other_target => CodeState::TargetChanged {
            run_id: run_id.into(),
            target: format!("{alias}:{path}").into(),
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
    if receipt.kind == ReceiptKind::Up {
        return None;
    }
    let run_id = Untrusted::new(receipt.run_id.clone());
    let at_ms = receipt.finished_at_ms.unwrap_or(receipt.started_at_ms);
    if let Some(head) = &receipt.server_head_at_end {
        return Some(CodeState::Observed {
            sha: head.clone().into(),
            dirty: receipt.tree_dirty,
            expected: receipt.target_sha.clone().into(),
            run_id,
            outcome: receipt.outcome.as_ref().map(|_| receipt.badge().outcome),
            at_ms,
        });
    }
    let complete =
        receipt.status == ReceiptStatus::Final && receipt.outcome == Some(DeployOutcome::Succeeded);
    let recorded = |evidence| CodeState::Recorded {
        sha: receipt.target_sha.clone().into(),
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
    compare_envs_with(row, a, b, |root, a, b| git.compare(root, a, b))
}

/// [`compare_envs`] with the commit comparison supplied by the caller, e.g.
/// to answer from a cache. `compare` runs only when both cells have a
/// commit and the checkout exists.
pub fn compare_envs_with(
    row: &RepoRow,
    a: &str,
    b: &str,
    compare: impl FnOnce(&Path, &str, &str) -> Comparison,
) -> EnvComparison {
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
            .find(|cell| safe_component(cell.env.raw()) == safe_component(name))
        else {
            return unavailable(format!("no {name} environment"));
        };
        match &cell.code {
            CodeState::Recorded { sha, complete, .. } => {
                if !complete {
                    notes.push((cell.env.raw().to_string(), Qualifier::DeployIncomplete));
                }
                shas.push(sha.clone());
            }
            CodeState::Observed { sha, dirty, .. } => {
                if *dirty == Some(true) {
                    notes.push((cell.env.raw().to_string(), Qualifier::TreeDirty));
                }
                shas.push(sha.clone());
            }
            _ => return unavailable(format!("no recorded code on {}", cell.env.raw())),
        }
    }
    if row.path_missing {
        return unavailable("checkout not found".into());
    }
    EnvComparison {
        result: compare(Path::new(row.repo_root.raw()), shas[0].raw(), shas[1].raw()),
        notes,
    }
}

mod text;
pub use crate::records::Mark;
pub use text::comparison_text;

#[cfg(test)]
mod tests;
