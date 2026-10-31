use std::error::Error;
use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;

use shipslip::config::LoadedConfig;
use shipslip::logs::{escape, escape_field};
use shipslip::receipt::{
    self, describe_smoke, describe_watch, format_duration, strip_color, timing_suffix, Found,
    Listed, Receipt, ReceiptError, ReceiptKind, ReceiptStatus, ReceiptStepStatus,
};
use shipslip::{DeployOutcome, StepTimes, WatchStatus};

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
        match local_zone() {
            Some(zone) => format!("STARTED ({zone})"),
            None => "STARTED (UTC)".into(),
        },
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
                    match receipt.kind {
                        ReceiptKind::Up => format!("brought up at {}", short(&receipt.target_sha)),
                        ReceiptKind::Deploy => format!(
                            "{} → {}",
                            short(&receipt.from_sha),
                            short(&receipt.target_sha)
                        ),
                    },
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
    let outcome = match receipt.summary() {
        Some(summary) if !badge.unfinished => summary,
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
    field(&mut out, "Plan:", &receipt.plan_text());
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
    if receipt.kind == ReceiptKind::Up {
        field(&mut out, "Server HEAD:", &short(&receipt.from_sha));
    } else {
        field(
            &mut out,
            "From → target:",
            &format!(
                "{} → {}",
                short(&receipt.from_sha),
                short(&receipt.target_sha)
            ),
        );
    }
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
        if receipt.more_commits > 0 {
            let _ = writeln!(out, "  and {} more commits", receipt.more_commits);
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
            receipt.maintenance_down_times(),
        );
    }
    for step in &receipt.steps {
        step_line(
            &mut out,
            &step.index.to_string(),
            &step.command,
            step.status,
            step.exit_code,
            step.times(),
        );
        for line in &step.output {
            let _ = writeln!(out, "        {}", escape(&strip_color(line)));
        }
    }
    if receipt.kind == shipslip::receipt::ReceiptKind::Deploy {
        if let Some(status) = &receipt.maintenance_up_status {
            step_line(
                &mut out,
                "off",
                "Maintenance off: php artisan up",
                status.clone().into(),
                receipt.maintenance_up_exit_code,
                receipt.maintenance_up_times(),
            );
        }
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
        let text = describe_smoke(smoke);
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

    let next = receipt.next_steps();
    if !next.is_empty() {
        out.push_str("\nAt the time of this run:\n");
        for line in next {
            let _ = writeln!(out, "  {line}");
        }
    }
    out
}

/// Explains an "already up to date" block when the deployed commit's last run
/// did not succeed, so a retry does not hide a broken or down app.
pub(super) fn up_to_date_hint(root: &Path, project: &str, env: &str) -> Option<String> {
    let listing = receipt::list(root, project, Some(env), Some(receipt::WALK_LIMIT)).ok()?;
    let runs: Vec<_> = listing
        .rows
        .iter()
        .filter_map(|row| match row {
            Listed::Receipt { id, receipt, .. } if receipt.mutation_started => Some((id, receipt)),
            _ => None,
        })
        .collect();
    let same_target = |a: &Receipt, b: &Receipt| {
        a.target.ssh_alias == b.target.ssh_alias && a.target.path == b.target.path
    };
    let needs_attention = |receipt: &Receipt| {
        receipt.status == ReceiptStatus::InProgress
            || receipt.ended_in_failure()
            || receipt.app_left_down
    };
    let latest_up = runs
        .first()
        .copied()
        .filter(|(_, receipt)| receipt.kind == ReceiptKind::Up && needs_attention(receipt));
    let deploy_index = runs
        .iter()
        .position(|(_, receipt)| receipt.kind == ReceiptKind::Deploy);
    let relevant_up = deploy_index.and_then(|index| {
        runs[..index]
            .iter()
            .copied()
            .find(|(_, up)| same_target(runs[index].1, up))
    });
    let env = escape_field(env);
    let mut lines = Vec::new();
    if let Some(index) = deploy_index {
        let (id, deploy) = runs[index];
        // An Up changes maintenance state, not the result of the deploy. Only
        // newer runs on the same server checkout can update that state.
        let app_left_down = relevant_up.map_or(deploy.app_left_down, |(_, up)| {
            up.status != ReceiptStatus::Final
                || up.outcome != Some(DeployOutcome::Succeeded)
                || up.app_left_down
        });
        if deploy.status == ReceiptStatus::InProgress || deploy.ended_in_failure() || app_left_down
        {
            lines.push(format!(
                "The last run here ({id}) did not succeed: {}. See `slip receipts show {id}`.",
                deploy.badge().headline()
            ));
            if let Some(DeployOutcome::FailedAtStep { step, .. }) = &deploy.outcome {
                if *step > 0 {
                    lines.extend(receipt::failed_step_options(&env, *step));
                }
            }
            // A failed/unfinished Up has more specific recovery instructions,
            // including attach or checking an unknown result before retrying.
            if app_left_down && !relevant_up.is_some_and(|(_, up)| needs_attention(up)) {
                lines.push(format!(
                    "The app may still be in maintenance mode; `slip up {env}` turns it off."
                ));
            }
        }
    }
    let up_recovery = [
        relevant_up.filter(|(_, up)| needs_attention(up)),
        latest_up.filter(|(id, _)| !relevant_up.is_some_and(|(relevant_id, _)| relevant_id == *id)),
    ];
    for (id, up) in up_recovery.into_iter().flatten() {
        lines.push(format!(
            "The last `slip up` here ({id}) did not succeed: {}. See `slip receipts show {id}`.",
            up.badge().headline()
        ));
        lines.extend(up.next_steps());
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn step_line(
    out: &mut String,
    index: &str,
    command: &str,
    status: ReceiptStepStatus,
    exit_code: Option<i32>,
    times: StepTimes,
) {
    let _ = writeln!(
        out,
        "  {:<12}{:>4}  {}{}{}",
        status.label(),
        index,
        escape_field(command),
        exit_code
            .map(|code| format!(" (exit {code})"))
            .unwrap_or_default(),
        timing_suffix(times, time_of_day)
    );
}

fn short(sha: &str) -> String {
    escape_field(sha.get(..12).unwrap_or(sha))
}

/// The local time zone's abbreviation, e.g. `IST`; `None` where times fall
/// back to UTC.
fn local_zone() -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let zone = local_time(now, true);
    // `2026-10-08 16:30:00 IST`: the zone is the third field.
    zone.split(' ').nth(2).map(str::to_string)
}

/// `19:32:11 IST`.
fn time_of_day(millis: u128) -> String {
    let shown = local_time(millis, true);
    match shown.split_once(' ') {
        Some((_, time)) => time.to_string(),
        None => shown,
    }
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
