//! Read-only views over saved receipts: listing and lookup.
//!
//! Nothing here opens a receipt's `.lock` file: probing it could make a
//! concurrent `slip attach` fail to claim the run.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{io_error, read_receipt, safe_component, Receipt, ReceiptError};
use crate::logs::escape_field;
use crate::{DeployOutcome, SmokeResult, WatchStatus};

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

impl Receipt {
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
}

/// The smoke check's result in words, with server text escaped.
pub fn describe_smoke(smoke: &SmokeResult) -> String {
    match smoke {
        SmokeResult::NotConfigured => "not configured".into(),
        SmokeResult::Skipped => "skipped".into(),
        SmokeResult::Passed { status, latency_ms } => {
            format!("passed, HTTP {status} in {latency_ms} ms")
        }
        SmokeResult::Failed {
            status: Some(status),
            reason,
        } if *reason == format!("HTTP {status}") => format!("failed, HTTP {status}"),
        SmokeResult::Failed {
            status: Some(status),
            reason,
        } => format!("failed, HTTP {status}: {}", escape_field(reason)),
        SmokeResult::Failed {
            status: None,
            reason,
        } => format!("failed: {}", escape_field(reason)),
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
