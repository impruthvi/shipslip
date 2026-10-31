use std::error::Error;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use super::receipts::next_step_text;
use shipslip::git::Git;
use shipslip::logs::escape_field;
use shipslip::receipt::{
    self, age, comparison_text, newer_elsewhere_text, sentence, short_id, stale_hint_text,
    target_moved_text, EnvComparison, Overview, ReceiptKind, RepoRow, StaleHint,
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
            let _ = writeln!(out, "  {env:<width$}  {}", sentence(cell, now_ms));
            if let Some(attempt) = &cell.last_attempt {
                let badge = &attempt.badge;
                let brought_up = attempt.kind == ReceiptKind::Up && badge.outcome.succeeded();
                let mut line = if brought_up {
                    format!(
                        "app brought up {} ({})",
                        short_id(&attempt.run_id),
                        age(now_ms, attempt.started_at_ms)
                    )
                } else {
                    format!(
                        "last {} {} ({}): {}",
                        if attempt.kind == ReceiptKind::Up {
                            "`slip up`"
                        } else {
                            "run"
                        },
                        short_id(&attempt.run_id),
                        age(now_ms, attempt.started_at_ms),
                        badge.outcome.headline()
                    )
                };
                for flag in &badge.flags {
                    line.push_str("  ");
                    line.push_str(&flag.text());
                }
                let _ = writeln!(out, "{pad}{}", escape_field(&line));
                for step in &attempt.next_steps.steps {
                    let _ = writeln!(out, "{pad}  → {}", next_step_text(step));
                }
            }
            if let Some(target) = &cell.target {
                let _ = writeln!(out, "{pad}on {}", escape_field(target));
            }
            if let Some(line) = target_moved_text(cell) {
                let _ = writeln!(out, "{pad}⚠ {line}");
            }
            if let Some(line) = newer_elsewhere_text(cell, now_ms) {
                let _ = writeln!(out, "{pad}⚠ {line}");
            }
            if let (
                Some(line),
                Some(StaleHint::NewestUnreadable { path } | StaleHint::UnattributedNewer { path }),
            ) = (stale_hint_text(cell), &cell.stale_hint)
            {
                let _ = writeln!(
                    out,
                    "{pad}⚠ {line}: {}",
                    escape_field(&path.to_string_lossy())
                );
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
                comparison_text(a, b, &compare_envs(row, a, b))
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

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
