//! Read-only views over saved receipts: listing, lookup and badges.
//!
//! Nothing here opens a receipt's `.lock` file: probing it could make a
//! concurrent `slip attach` fail to claim the run.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{
    io_error, read_receipt, safe_component, Receipt, ReceiptError, ReceiptStatus, ReceiptStepStatus,
};
use crate::logs::escape_field;
use crate::{DeployOutcome, RunPlan, SmokeResult, WatchStatus};

/// Shortest run ID prefix shown to users.
pub const MIN_ID_LEN: usize = 8;

pub struct Listing {
    pub rows: Vec<Listed>,
    /// Receipts beyond the requested limit.
    pub older: usize,
}

pub enum Listed {
    Receipt {
        id: String,
        path: PathBuf,
        receipt: Box<Receipt>,
    },
    /// Written by a newer Shipslip; only its file name can be interpreted.
    Newer {
        id: String,
        env: String,
        started_at_ms: Option<u128>,
        version: u32,
        path: PathBuf,
    },
    Unreadable {
        id: String,
        env: String,
        started_at_ms: Option<u128>,
        path: PathBuf,
        reason: String,
    },
}

pub enum Found {
    One(PathBuf),
    /// Display IDs of every matching receipt.
    Ambiguous(Vec<String>),
    Missing,
}

pub(super) struct Entry {
    /// The env directory's name, which is the encoded env.
    pub(super) env: String,
    pub(super) started_at_ms: Option<u128>,
    pub(super) run_id: String,
    pub(super) path: PathBuf,
}

/// Lists a project's receipts newest first, optionally for one environment.
/// Only the newest `limit` files are opened.
pub fn list(
    root: &Path,
    project: &str,
    env: Option<&str>,
    limit: Option<usize>,
) -> Result<Listing, ReceiptError> {
    let all = entries(root, project)?;
    let ids = display_ids(&all);
    let selected: Vec<(&Entry, &String)> = all
        .iter()
        .zip(&ids)
        .filter(|(entry, _)| env.is_none_or(|env| entry.env == safe_component(env)))
        .collect();
    let shown = limit.unwrap_or(selected.len()).min(selected.len());
    let rows = selected[..shown]
        .iter()
        .map(|(entry, id)| listed(entry, id))
        .collect();
    Ok(Listing {
        rows,
        older: selected.len() - shown,
    })
}

/// Finds one receipt by a run ID prefix, ignoring case.
pub fn find(root: &Path, project: &str, query: &str) -> Result<Found, ReceiptError> {
    let query = query.to_ascii_lowercase();
    if query.is_empty() {
        return Ok(Found::Missing);
    }
    let all = entries(root, project)?;
    let ids = display_ids(&all);
    let matches: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.run_id.starts_with(&query))
        .map(|(index, _)| index)
        .collect();
    Ok(match matches.as_slice() {
        [] => Found::Missing,
        [index] => Found::One(all[*index].path.clone()),
        _ => Found::Ambiguous(matches.iter().map(|index| ids[*index].clone()).collect()),
    })
}

fn listed(entry: &Entry, id: &str) -> Listed {
    let unreadable = |reason: String| Listed::Unreadable {
        id: id.into(),
        env: entry.env.clone(),
        started_at_ms: entry.started_at_ms,
        path: entry.path.clone(),
        reason,
    };
    match read_receipt(&entry.path) {
        Ok(receipt) => Listed::Receipt {
            id: id.into(),
            path: entry.path.clone(),
            receipt: Box::new(receipt),
        },
        Err(ReceiptError::Version(version)) => Listed::Newer {
            id: id.into(),
            env: entry.env.clone(),
            started_at_ms: entry.started_at_ms,
            version,
            path: entry.path.clone(),
        },
        Err(ReceiptError::Parse { source, .. }) => unreadable(source.to_string()),
        Err(ReceiptError::Io { source, .. }) => unreadable(source.to_string()),
        Err(error) => unreadable(error.to_string()),
    }
}

/// Receipt files of a project, newest first by the start time in their name.
/// Fails on the first directory that cannot be read.
fn entries(root: &Path, project: &str) -> Result<Vec<Entry>, ReceiptError> {
    let (entries, problems) = entries_in(&root.join(safe_component(project)));
    match problems.into_iter().next() {
        Some(problem) => Err(problem),
        None => Ok(entries),
    }
}

/// Like [`entries`], for an already-encoded project directory, but an
/// unreadable directory becomes a problem and the rest is still listed.
pub(super) fn entries_in(project_dir: &Path) -> (Vec<Entry>, Vec<ReceiptError>) {
    let mut entries = Vec::new();
    let mut problems = Vec::new();
    let env_dirs = match read_dir(project_dir) {
        Ok(dirs) => dirs,
        Err(problem) => return (entries, vec![problem]),
    };
    for env_dir in env_dirs {
        if !env_dir.is_dir() {
            continue;
        }
        let env = env_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let files = match read_dir(&env_dir) {
            Ok(files) => files,
            Err(problem) => {
                problems.push(problem);
                continue;
            }
        };
        for path in files {
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") || !path.is_file() {
                continue;
            }
            let stem = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (started_at_ms, run_id) = match stem.split_once('-') {
                Some((millis, run_id)) => match millis.parse::<u128>() {
                    Ok(millis) => (Some(millis), run_id.to_ascii_lowercase()),
                    Err(_) => (None, stem.to_ascii_lowercase()),
                },
                None => (None, stem.to_ascii_lowercase()),
            };
            entries.push(Entry {
                env: env.clone(),
                started_at_ms,
                run_id,
                path,
            });
        }
    }
    entries.sort_by(|a, b| {
        b.started_at_ms
            .cmp(&a.started_at_ms)
            .then_with(|| b.run_id.cmp(&a.run_id))
    });
    (entries, problems)
}

fn read_dir(dir: &Path) -> Result<Vec<PathBuf>, ReceiptError> {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()
            .map_err(|source| io_error(dir, source)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(io_error(dir, source)),
    }
}

/// The shortest prefix of each run ID, at least [`MIN_ID_LEN`] long, that no
/// other receipt of the project shares.
fn display_ids(entries: &[Entry]) -> Vec<String> {
    // In sorted order, the longest prefix an ID shares is with a neighbor.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|a, b| entries[*a].run_id.cmp(&entries[*b].run_id));
    let mut ids = vec![String::new(); entries.len()];
    for (position, &index) in order.iter().enumerate() {
        let id = &entries[index].run_id;
        let shared = [position.checked_sub(1), Some(position + 1)]
            .into_iter()
            .flatten()
            .filter_map(|neighbor| order.get(neighbor))
            .map(|&other| common_prefix(id, &entries[other].run_id))
            .max()
            .unwrap_or(0);
        let mut len = (shared + 1).max(MIN_ID_LEN).min(id.len());
        while !id.is_char_boundary(len) {
            len += 1;
        }
        ids[index] = id[..len].to_owned();
    }
    ids
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(a, b)| a == b).count()
}

/// The dashboard summary of one run. It never reduces a run to a single
/// "healthy" state: checks that did not finish are flagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    /// The receipt is still in progress: the run is running, was
    /// interrupted, or crashed. `outcome` is then only the last recorded one.
    pub unfinished: bool,
    pub outcome: Option<DeployOutcome>,
    pub flags: Vec<Flag>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flag {
    /// New error groups seen by the log watch.
    NewErrors(u64),
    LogNotFullyObserved,
    LogNotRecorded,
    SmokeFailed(Option<u16>),
    SmokeNotRecorded,
    AppLeftDown,
}

impl Receipt {
    pub fn badge(&self) -> Badge {
        let mut flags = Vec::new();
        // Checks never apply to a run that stopped before changing anything.
        let checks_apply = !matches!(
            (self.status, &self.outcome),
            (
                ReceiptStatus::Final,
                Some(
                    DeployOutcome::CancelledBeforeChanges
                        | DeployOutcome::AbortedBeforeChanges(_)
                        | DeployOutcome::StoppedInMaintenance(_)
                )
            )
        );
        match &self.watch {
            None if checks_apply => flags.push(Flag::LogNotRecorded),
            None => {}
            Some(watch) => {
                let groups = watch.new_errors.len() as u64 + watch.overflow_groups;
                if groups > 0 {
                    flags.push(Flag::NewErrors(groups));
                }
                match watch.status {
                    WatchStatus::Complete | WatchStatus::NotRun => {}
                    WatchStatus::Partial
                    | WatchStatus::Unavailable
                    | WatchStatus::NoLogSeen
                    | WatchStatus::Cancelled => flags.push(Flag::LogNotFullyObserved),
                }
            }
        }
        match &self.smoke {
            None if checks_apply => flags.push(Flag::SmokeNotRecorded),
            None => {}
            Some(SmokeResult::Failed { status, .. }) => flags.push(Flag::SmokeFailed(*status)),
            Some(
                SmokeResult::Passed { .. } | SmokeResult::NotConfigured | SmokeResult::Skipped,
            ) => {}
        }
        if self.app_left_down {
            flags.push(Flag::AppLeftDown);
        }
        Badge {
            unfinished: self.status == ReceiptStatus::InProgress,
            outcome: self.outcome.clone(),
            flags,
        }
    }
}

impl Badge {
    /// The outcome part, e.g. `Succeeded` or `Unfinished (last recorded: Succeeded)`.
    pub fn headline(&self) -> String {
        let outcome = self.outcome.as_ref().map(DeployOutcome::label);
        match (self.unfinished, outcome) {
            (false, Some(outcome)) => outcome,
            (false, None) => "Finished without an outcome".into(),
            (true, Some(outcome)) => format!("Unfinished (last recorded: {outcome})"),
            (true, None) => "Unfinished".into(),
        }
    }
}

impl Flag {
    pub fn text(&self) -> String {
        match self {
            Self::NewErrors(1) => "⚠ 1 new error group".into(),
            Self::NewErrors(count) => format!("⚠ {count} new error groups"),
            Self::LogNotFullyObserved => "ⓘ log not fully observed".into(),
            Self::LogNotRecorded => "ⓘ log not recorded".into(),
            Self::SmokeFailed(Some(status)) => format!("⚠ smoke {status}"),
            Self::SmokeFailed(None) => "⚠ smoke failed".into(),
            Self::SmokeNotRecorded => "ⓘ smoke not recorded".into(),
            Self::AppLeftDown => "⛔ app in maintenance mode".into(),
        }
    }
}

impl Receipt {
    /// Who started the run, e.g. `Ana <ana@example.com> (local user ana)`.
    pub fn started_by(&self) -> Option<String> {
        let actor = self.actor.as_ref()?;
        let git = match (&actor.git_name, &actor.git_email) {
            (Some(name), Some(email)) => Some(format!("{name} <{email}>")),
            (Some(value), None) | (None, Some(value)) => Some(value.clone()),
            (None, None) => None,
        };
        match (git, &actor.user) {
            (Some(git), Some(user)) => Some(format!("{git} (local user {user})")),
            (Some(git), None) => Some(git),
            (None, Some(user)) => Some(format!("local user {user}")),
            (None, None) => None,
        }
    }

    /// Who started the run without contact details: the Git name, or the
    /// local user when no name was recorded.
    pub fn started_by_name(&self) -> Option<String> {
        let actor = self.actor.as_ref()?;
        actor.git_name.clone().or_else(|| actor.user.clone())
    }

    /// The run ended where the server may need attention.
    pub fn ended_in_failure(&self) -> bool {
        matches!(
            self.outcome,
            Some(
                DeployOutcome::FailedAtStep { .. }
                    | DeployOutcome::StoppedAfterStep { .. }
                    | DeployOutcome::Unknown { .. }
            )
        )
    }

    /// Recovery hints built only from what the receipt recorded. Server text
    /// is escaped, so the lines are safe to print as-is.
    pub fn next_steps(&self) -> Vec<String> {
        let env = escape_field(&self.target.env);
        let mut lines = Vec::new();
        if self.status == ReceiptStatus::InProgress {
            lines.push(format!(
                "The run did not finish. If no deploy is running, resume it from {} with `slip attach {env}`.",
                escape_field(&self.repo_root)
            ));
        }
        match &self.outcome {
            Some(DeployOutcome::FailedAtStep { step: 0, .. }) => lines.push(
                "The fast-forward failed. Check the server checkout before deploying again.".into(),
            ),
            Some(DeployOutcome::FailedAtStep { step, .. }) => {
                lines.push(format!("Step {step} failed and later steps did not run."));
                lines.extend(failed_step_options(&env, *step));
            }
            Some(DeployOutcome::StoppedAfterStep { step, .. })
                if *step < self.target.steps.len() =>
            {
                lines.push(format!(
                    "Steps after {step} did not run. `slip from-step {env} {}` runs them on the deployed commit.",
                    step + 1
                ))
            }
            Some(DeployOutcome::StoppedInMaintenance(_)) => lines.push(
                "No deploy steps ran, so the code on the server did not change.".into(),
            ),
            Some(DeployOutcome::Unknown { step, .. }) => lines.push(format!(
                "The result of step {step} is unknown. Check the server before running anything again."
            )),
            _ => {}
        }
        if self.app_left_down {
            lines.push(format!(
                "The app may still be in maintenance mode. After checking the server, `slip up {env}` turns it off."
            ));
        }
        lines
    }
}

/// Retrying the same commit only helps when the cause was outside the code.
/// `env` must already be escaped.
pub fn failed_step_options(env: &str, step: usize) -> [String; 2] {
    [
        format!(
            "If the cause was on the server (permissions, .env, database), fix it there, then `slip from-step {env} {step}` runs the remaining steps on the deployed commit."
        ),
        format!(
            "If it needs a code change, push the fix and run `slip deploy {env}`."
        ),
    ]
}

pub fn describe_plan(plan: RunPlan) -> String {
    match plan {
        RunPlan::Deploy => "deploy".into(),
        RunPlan::Rerun => "rerun on the deployed commit".into(),
        RunPlan::FromStep(step) => format!("from step {step} on the deployed commit"),
    }
}

pub fn describe_watch(status: WatchStatus) -> &'static str {
    match status {
        WatchStatus::Complete => "complete",
        WatchStatus::Partial => "partial",
        WatchStatus::Unavailable => "unavailable",
        WatchStatus::Cancelled => "cancelled",
        WatchStatus::NotRun => "not run",
        WatchStatus::NoLogSeen => "no log file seen",
    }
}

impl ReceiptStepStatus {
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
