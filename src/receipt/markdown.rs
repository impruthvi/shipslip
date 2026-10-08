//! Markdown rendering of a receipt for sharing.
//!
//! By default only structured facts are included: identifiers, commands and
//! commit subjects from the repository, statuses, exit codes and counts. The
//! deployer appears by name only; the email address needs `details`.
//! Text that came from the server or from errors (step output, log messages,
//! reasons, warnings) can contain secrets and appears only with `details`.

use std::fmt::Write;

use super::{describe_plan, describe_watch, format_duration, format_utc, strip_color, Receipt};
use crate::logs::{escape, escape_field};
use crate::{SmokeResult, StepStatus};

pub fn markdown(receipt: &Receipt, details: bool) -> String {
    let mut out = String::new();
    let target = &receipt.target;
    let badge = receipt.badge();
    let _ = writeln!(
        out,
        "# Shipslip receipt: {} / {}\n",
        inline(&receipt.project),
        inline(&target.env)
    );
    if details {
        out.push_str(
            "> Includes server output and error text. Review it for secrets before sharing.\n\n",
        );
    }
    let outcome = match (&receipt.outcome, details) {
        (Some(outcome), true) if !badge.unfinished => inline(&outcome.summary()),
        _ => inline(&badge.headline()),
    };
    out.push_str("| | |\n|---|---|\n");
    row(&mut out, "Run", &code(&receipt.run_id));
    row(&mut out, "Outcome", &outcome);
    row(&mut out, "Plan", &inline(&describe_plan(receipt.run_plan)));
    row(
        &mut out,
        "Server",
        &format!(
            "{} {}{}",
            code(&target.ssh_alias),
            code(&target.path),
            if target.production {
                " (production)"
            } else {
                ""
            }
        ),
    );
    row(&mut out, "Branch", &code(&target.branch));
    row(&mut out, "Started", &format_utc(receipt.started_at_ms));
    row(
        &mut out,
        "Finished",
        &receipt
            .finished_at_ms
            .map(format_utc)
            .unwrap_or_else(|| "not finished".into()),
    );
    if let Some(finished) = receipt.finished_at_ms {
        row(
            &mut out,
            "Duration",
            &format_duration(finished.saturating_sub(receipt.started_at_ms)),
        );
    }
    row(
        &mut out,
        "Started by",
        &inline(
            &if details {
                receipt.started_by()
            } else {
                receipt.started_by_name()
            }
            .unwrap_or_else(|| "not recorded".into()),
        ),
    );
    row(
        &mut out,
        "From → target",
        &format!(
            "{} → {}",
            code(short(&receipt.from_sha)),
            code(short(&receipt.target_sha))
        ),
    );
    if receipt.ended_in_failure() {
        row(&mut out, "Server HEAD at end", &server_head(receipt));
    }

    if !badge.flags.is_empty() {
        out.push_str("\n## Flags\n\n");
        for flag in &badge.flags {
            let _ = writeln!(out, "- {}", inline(&flag.text()));
        }
    }

    if !receipt.commits.is_empty() {
        out.push_str("\n## Commits\n\n");
        for commit in &receipt.commits {
            let _ = writeln!(out, "- {}", inline(commit));
        }
    }

    out.push_str("\n## Steps\n\n| Step | Command | Status | Exit |\n|---|---|---|---|\n");
    if let Some(status) = &receipt.maintenance_down_status {
        maintenance_row(
            &mut out,
            "Maintenance on",
            "php artisan down",
            status,
            receipt.maintenance_down_exit_code,
        );
    }
    for step in &receipt.steps {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            step.index,
            code(&step.command),
            step.status.label(),
            exit(step.exit_code)
        );
    }
    if let Some(status) = &receipt.maintenance_up_status {
        maintenance_row(
            &mut out,
            "Maintenance off",
            "php artisan up",
            status,
            receipt.maintenance_up_exit_code,
        );
    }

    if let Some(watch) = &receipt.watch {
        out.push_str("\n## Log watch\n\n");
        let _ = writeln!(
            out,
            "Status: {}, {}, {} lines read from {}.",
            describe_watch(watch.status),
            format_duration(u128::from(watch.duration_ms)),
            watch.observed_lines,
            code(watch.log_path.as_deref().unwrap_or(target.log_path()))
        );
        if !watch.new_errors.is_empty() {
            out.push_str("\n| New error | File | Count |\n|---|---|---|\n");
            for group in &watch.new_errors {
                let _ = writeln!(
                    out,
                    "| {} | {} | {} |",
                    code(&group.exception),
                    group.file.as_deref().map(code).unwrap_or_default(),
                    group.count
                );
            }
            if watch.overflow_groups > 0 {
                let _ = writeln!(out, "\nAnd {} more groups.", watch.overflow_groups);
            }
            if details {
                for group in &watch.new_errors {
                    for variant in &group.variants {
                        let _ = writeln!(
                            out,
                            "\n{} ×{}{}\n{}",
                            code(&group.exception),
                            variant.count,
                            variant
                                .display_file_line
                                .as_deref()
                                .map(|line| format!(" at {}", code(line)))
                                .unwrap_or_default(),
                            fence(&variant.message)
                        );
                    }
                }
            }
        }
        if details && !watch.warnings.is_empty() {
            out.push('\n');
            for warning in &watch.warnings {
                let _ = writeln!(out, "- {}", inline(warning));
            }
        }
    }

    if let Some(smoke) = &receipt.smoke {
        out.push_str("\n## Smoke check\n\n");
        let text = match smoke {
            SmokeResult::NotConfigured => "Not configured.".into(),
            SmokeResult::Skipped => "Skipped.".into(),
            SmokeResult::Passed { status, latency_ms } => {
                format!("Passed: HTTP {status} in {latency_ms} ms.")
            }
            SmokeResult::Failed { status, reason } => match (status, details) {
                (Some(status), true) if *reason != format!("HTTP {status}") => {
                    format!("Failed: HTTP {status}: {}", inline(reason))
                }
                (Some(status), _) => format!("Failed: HTTP {status}."),
                (None, true) => format!("Failed: {}", inline(reason)),
                (None, false) => "Failed.".into(),
            },
        };
        let _ = writeln!(out, "{text}");
    }

    if details {
        let notes: Vec<&String> = receipt
            .last_message
            .iter()
            .chain(&receipt.warnings)
            .collect();
        if !notes.is_empty() {
            out.push_str("\n## Notes\n\n");
            for note in notes {
                let _ = writeln!(out, "- {}", inline(note));
            }
        }
        let with_output: Vec<_> = receipt
            .steps
            .iter()
            .filter(|step| !step.output.is_empty())
            .collect();
        if !with_output.is_empty() {
            out.push_str("\n## Output\n");
            for step in with_output {
                let _ = writeln!(
                    out,
                    "\nStep {} (last {} lines):\n{}",
                    step.index,
                    step.output.len(),
                    fence(&strip_color(&step.output.join("\n")))
                );
            }
        }
    }
    out
}

fn row(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(out, "| {label} | {value} |");
}

fn maintenance_row(
    out: &mut String,
    label: &str,
    command: &str,
    status: &StepStatus,
    exit_code: Option<i32>,
) {
    let _ = writeln!(
        out,
        "| {label} | {} | {} | {} |",
        code(command),
        super::ReceiptStepStatus::from(status.clone()).label(),
        exit(exit_code)
    );
}

fn server_head(receipt: &Receipt) -> String {
    match (&receipt.server_head_at_end, receipt.tree_dirty) {
        (Some(head), Some(true)) => format!("{} with uncommitted changes", code(short(head))),
        (Some(head), _) => code(short(head)),
        (None, _) => "not recorded".into(),
    }
}

fn exit(code: Option<i32>) -> String {
    code.map(|code| code.to_string()).unwrap_or_default()
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// Text inside a table cell or list item: one line, no Markdown syntax.
fn inline(text: &str) -> String {
    let mut out = String::new();
    for c in escape_field(text).chars() {
        if matches!(
            c,
            '\\' | '`'
                | '*'
                | '_'
                | '['
                | ']'
                | '<'
                | '>'
                | '#'
                | '|'
                | '~'
                | '!'
                | '('
                | ')'
                | '&'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Inline code that no backtick in `text` can close early.
fn code(text: &str) -> String {
    let text = escape_field(text).replace('|', "\\|");
    let ticks = "`".repeat(longest_run(&text, '`') + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{text}{pad}{ticks}")
}

/// A fenced block longer than any backtick run inside it.
fn fence(text: &str) -> String {
    let text = escape(text);
    let ticks = "`".repeat((longest_run(&text, '`') + 1).max(3));
    format!("{ticks}text\n{text}\n{ticks}")
}

fn longest_run(text: &str, target: char) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for c in text.chars() {
        if c == target {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}
