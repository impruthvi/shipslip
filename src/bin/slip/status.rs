use std::error::Error;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use shipslip::git::{Comparison, Git};
use shipslip::logs::escape_field;
use shipslip::receipt::{
    self, CodeState, EnvCell, EnvComparison, Evidence, NotKnownReason, Overview, Qualifier,
    ReceiptStepStatus, RepoRow, StaleHint,
};

use super::invalid_input;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Args {
    pub compare: Option<(String, String)>,
}

impl Args {
    pub(super) fn parse(args: &[String]) -> Result<Self, Box<dyn Error>> {
        match args {
            [] => Ok(Self { compare: None }),
            [flag, a, b] if flag == "--compare" && a != b => Ok(Self {
                compare: Some((a.clone(), b.clone())),
            }),
            _ => Err(invalid_input("usage: slip status [--compare ENV ENV]").into()),
        }
    }
}

pub(super) fn run(
    receipts_root: &Path,
    trust_path: &Path,
    args: Args,
) -> Result<ExitCode, Box<dyn Error>> {
    let overview = receipt::overview(receipts_root, trust_path)?;
    let git = Git::new(PathBuf::from("git"));
    let compare = args.compare.as_ref().map(|(a, b)| (a.as_str(), b.as_str()));
    print!(
        "{}",
        render(&overview, now_ms(), compare, |row, a, b| {
            receipt::compare_envs(&git, row, a, b)
        })
    );
    Ok(ExitCode::SUCCESS)
}

fn render(
    overview: &Overview,
    now_ms: u128,
    compare: Option<(&str, &str)>,
    compare_envs: impl Fn(&RepoRow, &str, &str) -> EnvComparison,
) -> String {
    let mut out = String::new();
    if overview.repos.is_empty() {
        out.push_str(
            "No deploys or approvals recorded yet. Run `slip trust ENV` and `slip deploy ENV` in a project.\n",
        );
    } else {
        out.push_str(
            "Last recorded code per environment, from local receipts. Nothing here is live.\n",
        );
    }
    for row in &overview.repos {
        let _ = write!(
            out,
            "\n{}  {}",
            escape_field(&row.label),
            escape_field(&row.repo_root)
        );
        out.push_str(if row.path_missing {
            "  (checkout not found)\n"
        } else {
            "\n"
        });
        let width = row
            .envs
            .iter()
            .map(|cell| escape_field(&cell.env).chars().count())
            .max()
            .unwrap_or(0);
        for cell in &row.envs {
            let env = escape_field(&cell.env);
            let pad = " ".repeat(width + 4);
            let _ = writeln!(out, "  {env:<width$}  {}", code_line(cell, now_ms));
            if let Some(attempt) = &cell.last_attempt {
                let badge = &attempt.badge;
                let mut line = format!(
                    "last run {} ({}): {}",
                    short_id(&attempt.run_id),
                    age(now_ms, attempt.started_at_ms),
                    badge.headline()
                );
                for flag in &badge.flags {
                    line.push_str("  ");
                    line.push_str(&flag.text());
                }
                if badge.unfinished {
                    let _ = write!(line, "; if nothing is running, `slip attach {}`", cell.env);
                }
                let _ = writeln!(out, "{pad}{}", escape_field(&line));
            }
            if let Some(target) = &cell.target {
                let _ = writeln!(out, "{pad}on {}", escape_field(target));
            }
            if let Some(other) = &cell.newer_elsewhere {
                let line = format!(
                    "⚠ a newer run ({}, {}) from {} ({}) used this server; this may be out of date",
                    short_id(&other.run_id),
                    age(now_ms, other.started_at_ms),
                    other.repo_root,
                    other.env
                );
                let _ = writeln!(out, "{pad}{}", escape_field(&line));
            }
            match &cell.stale_hint {
                Some(
                    StaleHint::NewestUnreadable { path } | StaleHint::UnattributedNewer { path },
                ) => {
                    let _ = writeln!(
                        out,
                        "{pad}⚠ a newer receipt could not be read; this may be out of date: {}",
                        escape_field(&path.to_string_lossy())
                    );
                }
                None => {}
            }
        }
    }
    if let Some((a, b)) = compare {
        let _ = writeln!(out, "\n{} vs {}:", escape_field(a), escape_field(b));
        let mut any = false;
        for row in &overview.repos {
            let has = |name: &str| row.envs.iter().any(|cell| cell.env == name);
            if !has(a) || !has(b) {
                continue;
            }
            any = true;
            let _ = writeln!(
                out,
                "  {}  {}",
                escape_field(&row.label),
                comparison_line(a, b, &compare_envs(row, a, b))
            );
        }
        if !any {
            let _ = writeln!(
                out,
                "  No checkout has both {} and {}.",
                escape_field(a),
                escape_field(b)
            );
        }
    }
    if !overview.problems.is_empty() {
        out.push_str("\nCould not read:\n");
        for problem in &overview.problems {
            let _ = writeln!(out, "  {}", escape_field(&problem.message));
        }
    }
    out
}

/// What the receipts say about the code; never "deployed" or "running".
fn code_line(cell: &EnvCell, now_ms: u128) -> String {
    let line = match &cell.code {
        CodeState::Recorded {
            sha,
            complete,
            evidence,
            at_ms,
            ..
        } => {
            let mut line = format!("code at {}", short(sha));
            if !complete {
                line.push_str(", deploy incomplete");
            }
            if *evidence == Evidence::CheckedOutAtStart {
                line.push_str(", checked out at start");
            }
            format!("{line} (recorded {})", age(now_ms, *at_ms))
        }
        CodeState::Observed {
            sha,
            dirty,
            expected,
            run_id,
            at_ms,
        } => {
            let mut line = format!("observed {}", short(sha));
            if !sha.eq_ignore_ascii_case(expected) {
                let _ = write!(
                    line,
                    " after run {}, expected {}",
                    short_id(run_id),
                    short(expected)
                );
            }
            if *dirty == Some(true) {
                line.push_str(", tree dirty");
            }
            format!("{line} ({})", age(now_ms, *at_ms))
        }
        CodeState::NotKnown { run_id, reason } => {
            let why: String = match reason {
                NotKnownReason::StepZero(ReceiptStepStatus::Failed) => "fast-forward failed".into(),
                NotKnownReason::StepZero(ReceiptStepStatus::Running) => {
                    "fast-forward was running when saved".into()
                }
                NotKnownReason::StepZero(_) => "fast-forward result unknown".into(),
                NotKnownReason::NoStepZero => "no fast-forward step recorded".into(),
                NotKnownReason::Unreadable => "its receipts could not be read".into(),
            };
            match run_id {
                Some(run_id) => format!(
                    "server code not known after run {} ({why})",
                    short_id(run_id)
                ),
                None => format!("server code not known ({why})"),
            }
        }
        CodeState::NeverDeployed if cell.approved => "approved before, never deployed".into(),
        CodeState::NeverDeployed => "never deployed".into(),
        CodeState::NoCodeChangeInLast { records: 1 } => "no code change in the last run".into(),
        CodeState::NoCodeChangeInLast { records } => {
            format!("no code change in the last {records} runs")
        }
        CodeState::TargetChanged { run_id, target } => format!(
            "target changed in run {}; no code recorded for {target}",
            short_id(run_id)
        ),
    };
    escape_field(&line)
}

fn comparison_line(a: &str, b: &str, comparison: &EnvComparison) -> String {
    let commits = |n: usize| if n == 1 { "commit" } else { "commits" };
    let mut line = match &comparison.result {
        Comparison::Same => format!("{a} and {b} are on the same commit"),
        Comparison::Ahead(n) => format!("{a} is {n} {} ahead of {b}", commits(*n)),
        Comparison::Behind(n) => format!("{a} is {n} {} behind {b}", commits(*n)),
        Comparison::Diverged { a: left, b: right } => {
            format!("{a} and {b} have diverged: {left} only on {a}, {right} only on {b}")
        }
        Comparison::Unavailable(reason) => format!("cannot compare: {reason}"),
    };
    if !comparison.notes.is_empty() {
        let notes: Vec<String> = comparison
            .notes
            .iter()
            .map(|(env, qualifier)| match qualifier {
                Qualifier::DeployIncomplete => format!("{env} deploy incomplete"),
                Qualifier::TreeDirty => format!("{env} tree dirty"),
            })
            .collect();
        let _ = write!(line, " ({})", notes.join("; "));
    }
    escape_field(&line)
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn short_id(run_id: &str) -> &str {
    run_id.get(..receipt::MIN_ID_LEN).unwrap_or(run_id)
}

fn age(now_ms: u128, then_ms: u128) -> String {
    let minutes = now_ms.saturating_sub(then_ms) / 60_000;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes}m ago"),
        60..=2879 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
