use std::error::Error;
use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;

use shipslip::config::LoadedConfig;
use shipslip::logs::{escape, escape_field};
use shipslip::receipt::{
    self, describe_plan, describe_watch, format_duration, strip_color, Found, Listed, Receipt,
    ReceiptError, ReceiptStatus, ReceiptStepStatus,
};
use shipslip::{DeployOutcome, SmokeResult, WatchStatus};

use super::invalid_input;

/// Receipts listed unless `--all` is given.
const LIST_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Args {
    List { env: Option<String>, all: bool },
    Show { id: String, md: bool, details: bool },
}

impl Args {
    pub(super) fn parse(args: &[String]) -> Result<Self, Box<dyn Error>> {
        if args.first().map(String::as_str) == Some("show") {
            let mut id = None;
            let (mut md, mut details) = (false, false);
            for arg in &args[1..] {
                match arg.as_str() {
                    "--md" => md = true,
                    "--with-details" => details = true,
                    flag if flag.starts_with('-') => {
                        return Err(
                            invalid_input(format!("unknown receipts show option {flag}")).into(),
                        )
                    }
                    _ if id.is_some() => {
                        return Err(invalid_input("receipts show accepts one receipt ID").into())
                    }
                    value => id = Some(value.to_string()),
                }
            }
            let id = id.ok_or_else(|| invalid_input("receipts show requires a receipt ID"))?;
            if details && !md {
                return Err(invalid_input("--with-details only applies with --md").into());
            }
            return Ok(Self::Show { id, md, details });
        }
        let mut env = None;
        let mut all = false;
        for arg in args {
            match arg.as_str() {
                "--all" => all = true,
                flag if flag.starts_with('-') => {
                    return Err(invalid_input(format!("unknown receipts option {flag}")).into())
                }
                _ if env.is_some() => {
                    return Err(
                        invalid_input("receipts accepts at most one environment name").into(),
                    )
                }
                value => env = Some(value.to_string()),
            }
        }
        Ok(Self::List { env, all })
    }
}

pub(super) fn run(
    config: &LoadedConfig,
    root: &Path,
    args: Args,
) -> Result<ExitCode, Box<dyn Error>> {
    let project = config.project_name();
    match args {
        Args::List { env, all } => {
            let listing =
                receipt::list(root, project, env.as_deref(), (!all).then_some(LIST_LIMIT))?;
            print!("{}", render_list(project, env.as_deref(), &listing));
        }
        Args::Show { id, md, details } => {
            let path = match receipt::find(root, project, &id)? {
                Found::One(path) => path,
                Found::Missing => {
                    return Err(invalid_input(format!(
                        "no receipt of {} matches `{}`; run `slip receipts` to list them",
                        escape_field(project),
                        escape_field(&id)
                    ))
                    .into())
                }
                Found::Ambiguous(ids) => {
                    return Err(invalid_input(format!(
                        "`{}` matches several receipts ({}); type more of the ID",
                        escape_field(&id),
                        ids.join(", ")
                    ))
                    .into())
                }
            };
            let receipt = receipt::read(&path).map_err(|error| match error {
                ReceiptError::Version(version) => invalid_input(format!(
                    "this receipt was written by a newer Shipslip (receipt schema v{version}); upgrade slip to read it"
                ))
                .into(),
                error => Box::<dyn Error>::from(error),
            })?;
            if md {
                print!("{}", receipt::markdown(&receipt, details));
            } else {
                print!("{}", render_show(&receipt));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn render_list(project: &str, env: Option<&str>, listing: &receipt::Listing) -> String {
    let mut out = String::new();
    let scope = match env {
        Some(env) => format!("{} in {}", escape_field(project), escape_field(env)),
        None => escape_field(project),
    };
    if listing.rows.is_empty() {
        let _ = writeln!(out, "No receipts for {scope} yet.");
        return out;
    }
    let mut rows = vec![[
        "ID".to_string(),
        "STARTED".into(),
        "ENV".into(),
        "FROM → TARGET".into(),
        "OUTCOME".into(),
    ]];
    for listed in &listing.rows {
        rows.push(match listed {
            Listed::Receipt { id, receipt, .. } => {
                let badge = receipt.badge();
                let mut outcome = badge.headline();
                for flag in &badge.flags {
                    outcome.push_str("  ");
                    outcome.push_str(&flag.text());
                }
                [
                    id.clone(),
                    local_time(receipt.started_at_ms, false),
                    escape_field(&receipt.target.env),
                    format!(
                        "{} → {}",
                        short(&receipt.from_sha),
                        short(&receipt.target_sha)
                    ),
                    escape_field(&outcome),
                ]
            }
            Listed::Newer {
                id,
                env,
                started_at_ms,
                version,
                ..
            } => [
                id.clone(),
                started_at_ms
                    .map(|ms| local_time(ms, false))
                    .unwrap_or_default(),
                escape_field(env),
                String::new(),
                format!("needs a newer slip (receipt schema v{version})"),
            ],
            Listed::Unreadable {
                id,
                env,
                started_at_ms,
                reason,
                ..
            } => [
                escape_field(id),
                started_at_ms
                    .map(|ms| local_time(ms, false))
                    .unwrap_or_default(),
                escape_field(env),
                String::new(),
                format!("unreadable: {}", escape_field(reason)),
            ],
        });
    }
    let widths: Vec<usize> = (0..4)
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let _ = writeln!(out, "Receipts for {scope}, newest first\n");
    for row in &rows {
        let mut line = String::new();
        for (column, width) in widths.iter().enumerate() {
            let _ = write!(line, "{:<width$}  ", row[column]);
        }
        line.push_str(&row[4]);
        let _ = writeln!(out, "{}", line.trim_end());
    }
    if listing.older > 0 {
        let _ = writeln!(
            out,
            "\n{} older receipt{}; use --all to list them.",
            listing.older,
            if listing.older == 1 { "" } else { "s" }
        );
    }
    let _ = writeln!(out, "\nShow one: slip receipts show ID [--md]");
    out
}

fn render_show(receipt: &Receipt) -> String {
    let mut out = String::new();
    let target = &receipt.target;
    let badge = receipt.badge();
    let field = |out: &mut String, label: &str, value: &str| {
        let _ = writeln!(out, "{label:<20}{value}");
    };
    let _ = writeln!(out, "Receipt {}\n", escape_field(&receipt.run_id));
    let outcome = match &receipt.outcome {
        Some(outcome) if !badge.unfinished => outcome.summary(),
        _ => badge.headline(),
    };
    field(&mut out, "Outcome:", &escape_field(&outcome));
    if !badge.flags.is_empty() {
        let flags: Vec<String> = badge.flags.iter().map(|flag| flag.text()).collect();
        field(&mut out, "Flags:", &flags.join(", "));
    }
    field(
        &mut out,
        "Project:",
        &format!(
            "{} (checkout {})",
            escape_field(&receipt.project),
            escape_field(&receipt.repo_root)
        ),
    );
    field(
        &mut out,
        "Environment:",
        &format!(
            "{}{} via {}:{}, branch {}",
            escape_field(&target.env),
            if target.production {
                " (production)"
            } else {
                ""
            },
            escape_field(&target.ssh_alias),
            escape_field(&target.path),
            escape_field(&target.branch)
        ),
    );
    field(&mut out, "Plan:", &describe_plan(receipt.run_plan));
    field(
        &mut out,
        "Started:",
        &local_time(receipt.started_at_ms, true),
    );
    field(
        &mut out,
        "Finished:",
        &match receipt.finished_at_ms {
            Some(finished) => format!(
                "{} ({})",
                local_time(finished, true),
                format_duration(finished.saturating_sub(receipt.started_at_ms))
            ),
            None => "not finished".into(),
        },
    );
    field(
        &mut out,
        "Started by:",
        &escape_field(
            &receipt
                .started_by()
                .unwrap_or_else(|| "not recorded".into()),
        ),
    );
    field(
        &mut out,
        "From → target:",
        &format!(
            "{} → {}",
            short(&receipt.from_sha),
            short(&receipt.target_sha)
        ),
    );
    if receipt.ended_in_failure() {
        field(
            &mut out,
            "Server HEAD at end:",
            &match (&receipt.server_head_at_end, receipt.tree_dirty) {
                (Some(head), Some(true)) => format!("{} with uncommitted changes", short(head)),
                (Some(head), _) => short(head),
                (None, _) => "not recorded".into(),
            },
        );
    }

    if !receipt.commits.is_empty() {
        out.push_str("\nCommits:\n");
        for commit in &receipt.commits {
            let _ = writeln!(out, "  {}", escape_field(commit));
        }
    }

    out.push_str("\nSteps:\n");
    if let Some(status) = &receipt.maintenance_down_status {
        step_line(
            &mut out,
            "on",
            "Maintenance on: php artisan down",
            status.clone().into(),
            receipt.maintenance_down_exit_code,
        );
    }
    for step in &receipt.steps {
        step_line(
            &mut out,
            &step.index.to_string(),
            &step.command,
            step.status,
            step.exit_code,
        );
        for line in &step.output {
            let _ = writeln!(out, "        {}", escape(&strip_color(line)));
        }
    }
    if let Some(status) = &receipt.maintenance_up_status {
        step_line(
            &mut out,
            "off",
            "Maintenance off: php artisan up",
            status.clone().into(),
            receipt.maintenance_up_exit_code,
        );
    }

    if let Some(watch) = &receipt.watch {
        let _ = writeln!(
            out,
            "\nLog watch: {}, {}, {} lines read from {}",
            describe_watch(watch.status),
            format_duration(u128::from(watch.duration_ms)),
            watch.observed_lines,
            escape_field(watch.log_path.as_deref().unwrap_or(target.log_path()))
        );
        if watch.status != WatchStatus::NotRun {
            let _ = writeln!(
                out,
                "  Compared against {} recent log signatures and {} from earlier deploys",
                watch.baseline_signatures, watch.history_signatures
            );
        }
        for group in &watch.new_errors {
            let _ = writeln!(
                out,
                "  New: ×{} {}{}",
                group.count,
                escape_field(&group.exception),
                group
                    .file
                    .as_deref()
                    .map(|file| format!("  {}", escape_field(file)))
                    .unwrap_or_default()
            );
            for variant in &group.variants {
                let _ = writeln!(
                    out,
                    "    ×{}{} {}",
                    variant.count,
                    variant
                        .display_file_line
                        .as_deref()
                        .map(|line| format!(" {}", escape_field(line)))
                        .unwrap_or_default(),
                    escape_field(&variant.message)
                );
            }
        }
        if watch.overflow_groups > 0 {
            let _ = writeln!(out, "  And {} more groups", watch.overflow_groups);
        }
        for warning in &watch.warnings {
            let _ = writeln!(out, "  Warning: {}", escape_field(warning));
        }
    }

    if let Some(smoke) = &receipt.smoke {
        let text = match smoke {
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
        };
        let _ = writeln!(out, "\nSmoke check: {text}");
    }

    let notes: Vec<&String> = receipt
        .last_message
        .iter()
        .chain(&receipt.warnings)
        .collect();
    if !notes.is_empty() {
        out.push_str("\nNotes:\n");
        for note in notes {
            let _ = writeln!(out, "  {}", escape_field(note));
        }
    }

    let next = next_steps(receipt);
    if !next.is_empty() {
        out.push_str("\nAt the time of this run:\n");
        for line in next {
            let _ = writeln!(out, "  {line}");
        }
    }
    out
}

/// Recovery hints built only from what the receipt recorded.
fn next_steps(receipt: &Receipt) -> Vec<String> {
    let env = escape_field(&receipt.target.env);
    let mut lines = Vec::new();
    if receipt.status == ReceiptStatus::InProgress {
        lines.push(format!(
            "The run did not finish. If no deploy is running, resume it from {} with `slip attach {env}`.",
            escape_field(&receipt.repo_root)
        ));
    }
    match &receipt.outcome {
        Some(DeployOutcome::FailedAtStep { step: 0, .. }) => lines.push(
            "The fast-forward failed. Check the server checkout before deploying again.".into(),
        ),
        Some(DeployOutcome::FailedAtStep { step, .. }) => {
            lines.push(format!("Step {step} failed and later steps did not run."));
            lines.extend(failed_step_options(&env, *step));
        }
        Some(DeployOutcome::StoppedAfterStep { step, .. })
            if *step < receipt.target.steps.len() =>
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
    if receipt.app_left_down {
        lines.push(format!(
            "The app may still be in maintenance mode. After checking the server, `slip up {env}` turns it off."
        ));
    }
    lines
}

/// Retrying the same commit only helps when the cause was outside the code.
fn failed_step_options(env: &str, step: usize) -> [String; 2] {
    [
        format!(
            "If the cause was on the server (permissions, .env, database), fix it there, then `slip from-step {env} {step}` runs the remaining steps on the deployed commit."
        ),
        format!(
            "If it needs a code change, push the fix and run `slip deploy {env}`."
        ),
    ]
}

/// Explains an "already up to date" block when the deployed commit's last run
/// did not succeed, so a retry does not hide a broken or down app.
pub(super) fn up_to_date_hint(root: &Path, project: &str, env: &str) -> Option<String> {
    let listing = receipt::list(root, project, Some(env), Some(10)).ok()?;
    let (id, receipt) = listing.rows.iter().find_map(|row| match row {
        Listed::Receipt { id, receipt, .. } if receipt.mutation_started => Some((id, receipt)),
        _ => None,
    })?;
    let needs_attention = receipt.status == ReceiptStatus::InProgress
        || receipt.ended_in_failure()
        || receipt.app_left_down;
    if !needs_attention {
        return None;
    }
    let env = escape_field(env);
    let mut lines = vec![format!(
        "The last run here ({id}) did not succeed: {}. See `slip receipts show {id}`.",
        receipt.badge().headline()
    )];
    if let Some(DeployOutcome::FailedAtStep { step, .. }) = &receipt.outcome {
        if *step > 0 {
            lines.extend(failed_step_options(&env, *step));
        }
    }
    if receipt.app_left_down {
        lines.push(format!(
            "The app may still be in maintenance mode; `slip up {env}` turns it off."
        ));
    }
    Some(lines.join("\n"))
}

fn step_line(
    out: &mut String,
    index: &str,
    command: &str,
    status: ReceiptStepStatus,
    exit_code: Option<i32>,
) {
    let _ = writeln!(
        out,
        "  {:<12}{:>4}  {}{}",
        status.label(),
        index,
        escape_field(command),
        exit_code
            .map(|code| format!(" (exit {code})"))
            .unwrap_or_default()
    );
}

fn short(sha: &str) -> String {
    escape_field(sha.get(..12).unwrap_or(sha))
}

/// Local time, `2026-10-07 19:32` or with seconds and zone.
fn local_time(millis: u128, seconds: bool) -> String {
    #[cfg(unix)]
    {
        let secs = (millis / 1000) as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: `secs` and `tm` are valid for the duration of the call.
        if !unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            let date = format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min
            );
            if !seconds {
                return date;
            }
            let zone = if tm.tm_zone.is_null() {
                String::new()
            } else {
                // SAFETY: localtime_r sets tm_zone to a static NUL-terminated string.
                unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) }
                    .to_string_lossy()
                    .into_owned()
            };
            return format!("{date}:{:02} {zone}", tm.tm_sec)
                .trim_end()
                .to_string();
        }
    }
    let utc = receipt::format_utc(millis);
    if seconds {
        utc
    } else {
        utc[..16].to_string()
    }
}

#[cfg(test)]
#[path = "receipts_tests.rs"]
mod tests;
