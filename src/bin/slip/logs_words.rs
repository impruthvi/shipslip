//! `slip logs` text, rendered from the logs record.

use shipslip::logs::{
    escape, escape_field, frame_rest, vendor_frame, Anchor, Entry, Level, RunCheck, Since,
};
use shipslip::records::{
    markdown_code as code, markdown_fence as fence, markdown_inline as inline, LogBadge,
    LogChannel, LogChannelKind, LogCoverage, LogEntry, LogFormat, LogGroup, LogsDetail,
    LogsSummary, SameRequest, Untrusted,
};

const DEFAULT_ROWS: usize = 20;
const ID_LEN: usize = 5;
const OUTPUT_COLUMNS: usize = 96;
/// Lines shown on each side of the entry in a same-request section.
const RELATED_SIDE: usize = 10;
/// Runs of at least this many vendor frames fold into one line.
const FOLD_RUN: usize = 3;
/// Keys whose value is copied into queued jobs when set through `Context`.
const JOB_SPANNING_KEYS: [&str; 2] = ["trace_id", "correlation_id"];

fn raw(text: &Option<Untrusted>) -> Option<&str> {
    text.as_ref().map(Untrusted::raw)
}

fn since_label(since: &Since) -> String {
    match since {
        Since::Ago(seconds) if seconds % 86_400 == 0 => format!("since {}d", seconds / 86_400),
        Since::Ago(seconds) if seconds % 3600 == 0 => format!("since {}h", seconds / 3600),
        Since::Ago(seconds) => format!("since {}m", seconds / 60),
        Since::At(at) => format!("since {at}"),
    }
}

pub(crate) fn window_label(anchor: &Anchor) -> String {
    match anchor {
        Anchor::CheckoutChange { commit } => {
            format!("since checkout change {}", &commit.raw()[..7])
        }
        Anchor::RunInProgress { run_id, step } => format!(
            "deploy in progress (run {}{}); since run start",
            run_id.raw(),
            step.map(|step| format!(", step {step}"))
                .unwrap_or_default()
        ),
        Anchor::Run {
            run_id,
            plan,
            target_commit,
            check,
            server_marker_outcome,
            reflog_recorded,
        } => {
            let detail = match check {
                RunCheck::Unverified { reason } => format!("unverified: {}", reason.raw()),
                RunCheck::Verified => if *reflog_recorded {
                    raw(server_marker_outcome).unwrap_or("outcome unknown")
                } else {
                    "outcome unknown"
                }
                .into(),
            };
            format!(
                "since slip {} {} ({}, {detail})",
                plan.raw(),
                run_id.raw(),
                &target_commit.raw()[..7]
            )
        }
        Anchor::Since { since } => since_label(since),
        Anchor::Default { .. } => "since 24h (default)".into(),
    }
}

fn kind_label(kind: &LogChannelKind) -> &'static str {
    match kind {
        LogChannelKind::Single => "single",
        LogChannelKind::Daily => "daily",
        LogChannelKind::Mixed => "single+daily",
    }
}

fn format_label(format: &LogFormat) -> String {
    match format {
        LogFormat::Recognized => "Laravel".into(),
        LogFormat::Partial { percent } => format!("partly recognized ({percent}% of lines)"),
        LogFormat::Unrecognized => "format not recognized".into(),
        LogFormat::Empty => "no lines in this window".into(),
        LogFormat::NotRead => "not read".into(),
    }
}

fn has_unknown_content(format: &LogFormat) -> bool {
    matches!(
        format,
        LogFormat::Partial { .. } | LogFormat::Unrecognized | LogFormat::NotRead
    )
}

fn complete(coverage: &LogCoverage) -> bool {
    matches!(coverage, LogCoverage::Complete)
}

fn covered_from(coverage: &LogCoverage) -> Option<&str> {
    match coverage {
        LogCoverage::Complete => None,
        LogCoverage::Partial { from } => raw(from),
    }
}

/// Some of this channel's window was not read, or was read but not understood.
fn channel_incomplete(channel: &LogChannel) -> bool {
    !complete(&channel.coverage) || channel.files.is_empty() || has_unknown_content(&channel.format)
}

/// Entries in the window may be missing from what was read, so every count
/// is a lower bound.
fn coverage_incomplete(summary: &LogsSummary) -> bool {
    summary.channels.is_empty() || summary.channels.iter().any(channel_incomplete)
}

fn clip(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn short_class(class: &str) -> &str {
    class.rsplit('\\').next().unwrap_or(class)
}

fn count_label(lower_bound: bool, count: u64) -> String {
    if lower_bound {
        format!("≥{count}")
    } else {
        format!("×{count}")
    }
}

fn group_count(group: &LogGroup) -> String {
    count_label(group.count_is_lower_bound, group.count)
}

/// Wrap presentation text; raw entries and stack traces keep their original lines.
fn push_wrapped(out: &mut String, prefix: &str, text: &str) {
    let mut line = prefix.to_string();
    let mut columns = prefix.chars().count();
    let mut has_word = false;
    for word in text.split_whitespace() {
        if has_word && columns + 1 + word.chars().count() > OUTPUT_COLUMNS {
            out.push_str(&line);
            out.push('\n');
            line = "    ".into();
            columns = 4;
            has_word = false;
        }
        if has_word {
            line.push(' ');
            columns += 1;
        }
        for character in word.chars() {
            if columns >= OUTPUT_COLUMNS {
                out.push_str(&line);
                out.push('\n');
                line = "    ".into();
                columns = 4;
            }
            line.push(character);
            columns += 1;
        }
        has_word = true;
    }
    out.push_str(&line);
    out.push('\n');
}

pub(crate) fn short_path<'a>(summary: &LogsSummary, path: &'a str) -> &'a str {
    // Log paths may be relative. Only use a root when an absolute storage path
    // identifies it; an empty hint must never strip a leading slash.
    if let Some(root) = summary
        .channels
        .iter()
        .flat_map(|channel| &channel.files)
        .map(Untrusted::raw)
        .filter(|file| file.starts_with('/'))
        .find_map(|file| file.rsplit_once("/storage/logs/").map(|(root, _)| root))
    {
        return path
            .strip_prefix(root)
            .and_then(|path| path.strip_prefix('/'))
            .unwrap_or(path);
    }
    // Laravel source directories still identify the root when log paths are relative.
    ["/app/", "/database/", "/routes/", "/config/", "/resources/"]
        .iter()
        .filter_map(|dir| path.rfind(dir))
        .max()
        .map(|at| &path[at + 1..])
        .unwrap_or(path)
}

fn status_label(summary: &LogsSummary, badge: &LogBadge) -> &'static str {
    if !summary.baseline_enabled {
        return "-";
    }
    badge_label(badge)
}

fn badge_label(badge: &LogBadge) -> &'static str {
    match badge {
        LogBadge::New => "NEW",
        LogBadge::Seen => "seen",
        LogBadge::NoBaseline => "?",
    }
}

/// Without a baseline every variant counts as seen.
fn variant_label(summary: &LogsSummary, badge: &LogBadge) -> &'static str {
    if summary.baseline_enabled {
        badge_label(badge)
    } else {
        "seen"
    }
}

fn short_message<'a>(message: &'a str, class: &str) -> &'a str {
    message
        .strip_prefix(class)
        .and_then(|message| message.strip_prefix(':'))
        .map(str::trim_start)
        .unwrap_or(message)
}

pub(crate) fn header(summary: &LogsSummary) -> String {
    let names: Vec<&str> = summary
        .channels
        .iter()
        .map(|channel| channel.name.raw())
        .collect();
    let mut out = String::new();
    push_wrapped(
        &mut out,
        "",
        &format!(
            "{} · {} · times {}",
            summary.environment.field(),
            if names.is_empty() {
                "no log channels".into()
            } else {
                escape_field(&names.join(", "))
            },
            summary.window.zone.field(),
        ),
    );
    push_wrapped(
        &mut out,
        "",
        &format!(
            "{} ({})",
            escape_field(&window_label(&summary.window.anchor)),
            summary.window.start.field()
        ),
    );
    let partial: Vec<_> = summary
        .channels
        .iter()
        .filter(|channel| !complete(&channel.coverage))
        .collect();
    if let Some(channel) = partial.iter().max_by_key(|channel| {
        let from = covered_from(&channel.coverage);
        (from.is_none(), from)
    }) {
        let others = if partial.len() > 1 {
            format!("; {} other channels partial", partial.len() - 1)
        } else {
            "; others complete".into()
        };
        if let Some(from) = covered_from(&channel.coverage) {
            push_wrapped(&mut out, "", &format!("covered: since {from} only ({}: read limit reached; raise it with --max-bytes){others}", channel.name.field()));
        } else {
            push_wrapped(
                &mut out,
                "",
                &format!(
                    "covered: partly ({}; see --channels){others}",
                    channel.name.field()
                ),
            );
        }
    }
    if summary.baseline_enabled {
        if let Some(baseline) = baseline_coverage(summary) {
            push_wrapped(
                &mut out,
                "",
                &format!(
                    "baseline: {} before the window ({}; {})",
                    escape_field(&baseline.span),
                    escape_field(baseline.channel),
                    baseline.read
                ),
            );
            out.push_str("    NEW = not seen in that baseline\n");
        } else {
            out.push_str("baseline: unavailable; ? = no readable comparison\n");
        }
        let missing: Vec<_> = summary
            .channels
            .iter()
            .filter(|channel| !channel.baseline.available)
            .map(|channel| channel.name.field())
            .collect();
        if !missing.is_empty() {
            push_wrapped(
                &mut out,
                "",
                &format!("no baseline: {}", missing.join(", ")),
            );
        }
    }
    for channel in &summary.channels {
        if channel.files.is_empty() {
            push_wrapped(
                &mut out,
                "",
                &format!(
                    "{}: no log file found ({})",
                    channel.name.field(),
                    channel.source.field()
                ),
            );
        }
        if let Some(format) = &channel.baseline.format {
            if has_unknown_content(format) {
                push_wrapped(
                    &mut out,
                    "",
                    &format!(
                        "{}: baseline {} — NEW unavailable for this channel",
                        channel.name.field(),
                        format_label(format)
                    ),
                );
            }
        }
        if matches!(
            channel.format,
            LogFormat::Partial { .. } | LogFormat::Unrecognized
        ) {
            push_wrapped(
                &mut out,
                "",
                &format!(
                    "{}: {} — see --raw",
                    channel.name.field(),
                    format_label(&channel.format)
                ),
            );
        }
        for file in &channel.changed_during_read {
            push_wrapped(
                &mut out,
                "",
                &format!(
                    "{}: {} changed during read and was skipped",
                    channel.name.field(),
                    file.field()
                ),
            );
        }
    }
    for warning in &summary.warnings {
        push_wrapped(&mut out, "", &format!("warning: {}", warning.field()));
    }
    out
}

/// The baseline's time span and how it was read, from the channel that
/// limits it. `None` when no channel has a readable baseline.
struct BaselineCoverage<'a> {
    span: String,
    channel: &'a str,
    read: &'static str,
}

fn baseline_coverage(summary: &LogsSummary) -> Option<BaselineCoverage<'_>> {
    let channel = summary
        .channels
        .iter()
        .filter(|channel| channel.baseline.available)
        .max_by_key(|channel| raw(&channel.baseline.from))?;
    let from = raw(&channel.baseline.from).unwrap_or("time unknown");
    let to = raw(&channel.baseline.to).unwrap_or("time unknown");
    Some(BaselineCoverage {
        span: if from == to {
            from.to_string()
        } else {
            format!("{from} to {to}")
        },
        channel: channel.name.raw(),
        read: if complete(&channel.baseline.coverage) {
            "full read"
        } else {
            "partial read"
        },
    })
}

fn counts_line(summary: &LogsSummary) -> Option<String> {
    if summary.level_counts.is_empty() {
        return None;
    }
    let list = summary
        .level_counts
        .iter()
        .map(|(level, count)| format!("{count} {}", level.name()))
        .collect::<Vec<_>>()
        .join(" · ");
    Some(if coverage_incomplete(summary) {
        format!("in this window: at least {list} (some channels partly read)")
    } else {
        format!("in this window: {list}")
    })
}

pub(crate) fn summary(summary: &LogsSummary, all: bool) -> String {
    let mut out = header(summary);
    if let Some(line) = counts_line(summary) {
        push_wrapped(&mut out, "", &line);
    }
    out.push('\n');
    let groups = &summary.groups;
    if groups.is_empty() {
        if coverage_incomplete(summary) {
            out.push_str("No matching entries in the portion read; log coverage is incomplete.\n");
        } else if summary.filter.text.is_some() {
            out.push_str(&format!(
                "No entries match --grep at {} level or above.\n",
                summary.filter.level
            ));
        } else {
            out.push_str(&format!(
                "No entries at {} level or above in the selected channels in this window.\n",
                summary.filter.level
            ));
        }
        return out;
    }
    let shown = if all {
        groups.len()
    } else {
        groups.len().min(DEFAULT_ROWS)
    };
    let id_width = groups
        .iter()
        .take(shown)
        .map(|group| group.id.raw().len())
        .max()
        .unwrap_or(ID_LEN);
    let count_width = groups
        .iter()
        .take(shown)
        .map(|group| group_count(group).chars().count())
        .max()
        .unwrap_or(0)
        .max(5);
    out.push_str(&format!(
        "{:>3}  {:<id_width$}  {:>count_width$}  {:<6}  EXCEPTION\n",
        "ROW", "ID", "COUNT", "STATUS"
    ));
    for (row, group) in groups.iter().take(shown).enumerate() {
        let prefix = format!(
            "{:>3}  {:<id_width$}  {:>count_width$}  {:<6}  ",
            row + 1,
            group.id.raw(),
            group_count(group),
            status_label(summary, &group.badge),
        );
        push_wrapped(
            &mut out,
            &prefix,
            &escape_field(short_class(group.exception.raw())),
        );
        let channels = group
            .channels
            .iter()
            .map(Untrusted::field)
            .collect::<Vec<_>>()
            .join(", ");
        let source = match raw(&group.file) {
            Some(file) => format!(
                "{} · channels: {channels}",
                escape_field(short_path(summary, file))
            ),
            None => format!("channels: {channels}"),
        };
        push_wrapped(&mut out, "    ", &source);
        let message = short_message(group.message.raw(), group.exception.raw());
        push_wrapped(&mut out, "    ", &escape_field(clip(message, 120)));
    }
    if shown < groups.len() {
        out.push_str(&format!("+{} more groups (--all)\n", groups.len() - shown));
    }
    if summary.overflow_groups > 0 {
        out.push_str(&format!(
            "+{} groups not tracked\n",
            summary.overflow_groups
        ));
    }
    out.push_str(&format!(
        "\nDetails: slip logs {} <ID or row>\n",
        summary.environment.field()
    ));
    out
}

/// `text` with runs of vendor frames folded into a count. Frame #0 (where
/// the exception was thrown) and every other line stay as written.
fn fold_frames(text: &str) -> String {
    fn flush(out: &mut Vec<String>, run: &mut Vec<&str>) {
        if run.len() >= FOLD_RUN {
            out.push(format!("… {} vendor frames (--full to show)", run.len()));
        } else {
            out.extend(run.iter().map(|line| line.to_string()));
        }
        run.clear();
    }
    let mut out = Vec::new();
    let mut run = Vec::new();
    for line in text.split('\n') {
        if vendor_frame(line) && frame_rest(line).is_some_and(|(number, _)| number > 0) {
            run.push(line);
            continue;
        }
        flush(&mut out, &mut run);
        out.push(line.to_string());
    }
    flush(&mut out, &mut run);
    out.join("\n")
}

fn entry_text(entry: &LogEntry, full: bool) -> String {
    if full {
        entry.text.raw().to_string()
    } else {
        fold_frames(entry.text.raw())
    }
}

/// The variant's `file:line`, as the grouping pass recorded it.
fn variant_at(file: Option<&Untrusted>, line: Option<u64>) -> Option<String> {
    let file = file?.raw();
    Some(match line {
        Some(line) => format!("{file}:{line}"),
        None => file.to_string(),
    })
}

fn shows_variants(summary: &LogsSummary, group: &LogGroup) -> bool {
    summary.baseline_enabled || group.variants.len() > 1 || group.overflow_variants > 0
}

/// One group in detail. Unless `full`, runs of vendor stack frames are folded.
pub(crate) fn detail(detail: &LogsDetail, full: bool) -> String {
    let summary = &detail.snapshot;
    let group = &detail.group;
    let latest = &detail.latest.entry;
    let channels: Vec<&str> = group.channels.iter().map(Untrusted::raw).collect();
    let mut out = String::new();
    push_wrapped(
        &mut out,
        &format!(
            "{}  {}  ",
            group.id.raw(),
            status_label(summary, &group.badge)
        ),
        &group.exception.field(),
    );
    push_wrapped(
        &mut out,
        "File:       ",
        &escape_field(raw(&group.file).unwrap_or("-")),
    );
    push_wrapped(&mut out, "Count:      ", &group_count(group));
    push_wrapped(
        &mut out,
        "First seen: ",
        if group.count_is_lower_bound {
            "—"
        } else {
            raw(&group.first_seen).unwrap_or("-")
        },
    );
    push_wrapped(
        &mut out,
        "Last seen:  ",
        raw(&group.last_seen).unwrap_or("-"),
    );
    push_wrapped(
        &mut out,
        "Channels:   ",
        &escape_field(&channels.join(", ")),
    );
    if shows_variants(summary, group) {
        out.push_str("\nVariants:\n");
        for variant in &group.variants {
            let prefix = format!(
                "  {:>5}  {:<4}  ",
                count_label(group.count_is_lower_bound, variant.count),
                variant_label(summary, &variant.badge),
            );
            push_wrapped(
                &mut out,
                &prefix,
                &format!(
                    "{}{}",
                    escape_field(clip(
                        short_message(variant.message.raw(), group.exception.raw()),
                        160
                    )),
                    variant_at(variant.file.as_ref(), variant.line)
                        .map(|at| format!(" (at {})", escape_field(short_path(summary, &at))))
                        .unwrap_or_default()
                ),
            );
        }
        if group.overflow_variants > 0 {
            out.push_str(&format!("  +{} more\n", group.overflow_variants));
        }
    }
    out.push('\n');
    push_wrapped(
        &mut out,
        "",
        &format!(
            "Latest entry ({}, {}):",
            raw(&latest.time).unwrap_or("time unknown"),
            latest.file.field(),
        ),
    );
    out.push_str(&escape(&entry_text(latest, full)));
    out.push('\n');
    if latest.truncated {
        out.push_str("Entry truncated at 256 KiB.\n");
    }
    out
}

/// `word` as typed in a shell: bare when it is plainly safe, quoted otherwise.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/@%+=,-".contains(&byte));
    if plain {
        word.into()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The same-request section for a group's latest entry, or a short note when
/// it has no ID. `window_args` are the user's `--since`/`--max-bytes` flags,
/// repeated in the follow-up command so it reads the same window.
pub(crate) fn related(detail: &LogsDetail, window_args: &[String]) -> String {
    let latest = &detail.latest.entry;
    let (key, value, conflict, before, after, more, may_be_missing) = match &detail.same_request
    {
        SameRequest::Shared {
            key,
            value,
            conflict,
            before,
            after,
            more,
            may_be_missing,
        } => (key, value, conflict, before, after, more, may_be_missing),
        SameRequest::Cut => {
            return "\nEntry was cut at 256 KiB, so an ID at its end may be missing.\n".into()
        }
        SameRequest::NoId if Level::parse(latest.level).is_some_and(|l| l >= Level::Error) => {
            return "\nNo request ID in this entry. Laravel's Context::add('request_id', …) in a middleware adds one to every log line.\n".into()
        }
        SameRequest::NoId => return String::new(),
    };
    // The record lists every match; the text shows up to RELATED_SIDE a side.
    let hidden =
        before.len().saturating_sub(RELATED_SIDE) + after.len().saturating_sub(RELATED_SIDE);
    let more = more + hidden as u64;
    let before = &before[before.len().saturating_sub(RELATED_SIDE)..];
    let after = &after[..after.len().min(RELATED_SIDE)];
    let mut out = String::new();
    out.push('\n');
    push_wrapped(
        &mut out,
        "",
        &format!(
            "Shared {} {} (correlation, not cause{}):",
            key,
            value.raw(),
            if JOB_SPANNING_KEYS.contains(key) {
                "; can include queued jobs"
            } else {
                ""
            }
        ),
    );
    if *conflict {
        out.push_str("  context and extra disagree; using context\n");
    }
    let line = |entry: &LogEntry, this: bool| {
        format!(
            "  {}  {}  {:<8} {}{}\n",
            raw(&entry.time).unwrap_or("time unknown"),
            entry.channel.field(),
            entry.level.to_ascii_uppercase(),
            escape_field(clip(entry.headline.raw(), 120)),
            if this { "  ← this entry" } else { "" }
        )
    };
    for entry in before {
        out.push_str(&line(entry, false));
    }
    out.push_str(&line(latest, true));
    for entry in after {
        out.push_str(&line(entry, false));
    }
    if before.is_empty() && after.is_empty() {
        out.push_str("  no other entry in this window has this ID\n");
    }
    if more > 0 {
        // One unwrapped line, so it can be pasted as is.
        let command = [
            "slip",
            "logs",
            detail.snapshot.environment.raw(),
            "--raw",
            "--grep",
            value.raw(),
        ]
        .into_iter()
        .chain(window_args.iter().map(String::as_str))
        .map(shell_word)
        .collect::<Vec<_>>()
        .join(" ");
        out.push_str(&format!(
            "  +{more} more; broader text search: {}\n",
            escape_field(&command)
        ));
    }
    if *may_be_missing {
        out.push_str("  (some channels only partly read)\n");
    }
    out
}

/// One group as Markdown to paste into an issue. Every value from the
/// server is escaped or fenced so none of it renders as Markdown or HTML.
pub(crate) fn detail_md(detail: &LogsDetail, full: bool) -> String {
    let summary = &detail.snapshot;
    let group = &detail.group;
    let latest = &detail.latest.entry;
    let channels: Vec<&str> = group.channels.iter().map(Untrusted::raw).collect();
    let mut out = format!(
        "## {} {}\n\n",
        code(group.id.raw()),
        inline(short_class(group.exception.raw()))
    );
    let mut item = |label: &str, value: String| out.push_str(&format!("- {label}: {value}\n"));
    item("Environment", inline(summary.environment.raw()));
    item("Log times", inline(summary.window.zone.raw()));
    item(
        "Window",
        format!(
            "{} ({} to {})",
            inline(&window_label(&summary.window.anchor)),
            inline(summary.window.start.raw()),
            inline(raw(&summary.window.end).unwrap_or("time unknown"))
        ),
    );
    item(
        "Coverage",
        if coverage_incomplete(summary) {
            "incomplete; counts are lower bounds".into()
        } else {
            "complete".into()
        },
    );
    item(
        "Baseline",
        if !summary.baseline_enabled {
            "off".into()
        } else {
            match baseline_coverage(summary) {
                Some(baseline) => format!(
                    "{} before the window ({}; {})",
                    inline(&baseline.span),
                    inline(baseline.channel),
                    baseline.read
                ),
                None => "unavailable".into(),
            }
        },
    );
    item(
        "Filters",
        match raw(&summary.filter.text) {
            Some(grep) => format!("{} or above; --grep {}", summary.filter.level, code(grep)),
            None => format!("{} or above", summary.filter.level),
        },
    );
    item("slip", env!("CARGO_PKG_VERSION").into());
    out.push('\n');
    let mut item = |label: &str, value: String| out.push_str(&format!("- {label}: {value}\n"));
    item("Exception", inline(group.exception.raw()));
    item("Status", status_label(summary, &group.badge).into());
    item(
        "File",
        raw(&group.file).map(code).unwrap_or_else(|| "-".into()),
    );
    item("Count", group_count(group));
    item(
        "First seen",
        if group.count_is_lower_bound {
            "—".into()
        } else {
            inline(raw(&group.first_seen).unwrap_or("-"))
        },
    );
    item("Last seen", inline(raw(&group.last_seen).unwrap_or("-")));
    item("Channels", inline(&channels.join(", ")));
    if shows_variants(summary, group) {
        out.push_str("\nVariants:\n\n");
        for variant in &group.variants {
            out.push_str(&format!(
                "- {} {} {}{}\n",
                count_label(group.count_is_lower_bound, variant.count),
                variant_label(summary, &variant.badge),
                inline(clip(
                    short_message(variant.message.raw(), group.exception.raw()),
                    160
                )),
                variant_at(variant.file.as_ref(), variant.line)
                    .map(|at| format!(" (at {})", code(short_path(summary, &at))))
                    .unwrap_or_default()
            ));
        }
        if group.overflow_variants > 0 {
            out.push_str(&format!("- +{} more\n", group.overflow_variants));
        }
    }
    out.push_str(&format!(
        "\nLatest entry ({}, {}):\n\n{}\n",
        inline(raw(&latest.time).unwrap_or("time unknown")),
        code(latest.file.raw()),
        fence(&entry_text(latest, full))
    ));
    if latest.truncated {
        out.push_str("\nEntry truncated at 256 KiB.\n");
    }
    out
}

/// What to say when a lookup finds no group.
pub(crate) fn miss(summary: &LogsSummary, query: &str) -> String {
    if summary.channels.is_empty() {
        return format!("group {} not found in what was read; no visible log channels — inspect channel configuration\n", escape(query));
    }
    if summary
        .channels
        .iter()
        .any(|channel| has_unknown_content(&channel.format))
    {
        return format!("group {} not found in what was read; some log formats are not fully recognized — inspect --raw or --channels\n", escape(query));
    }
    let partial = summary
        .channels
        .iter()
        .find(|channel| channel_incomplete(channel));
    let label = window_label(&summary.window.anchor);
    match partial.and_then(|channel| covered_from(&channel.coverage)) {
        Some(from) => format!(
            "group {} not found in what was read (entries since {from} of a window from {}); \
             try --max-bytes N or a later --since\n",
            escape(query),
            summary.window.start.raw()
        ),
        None if partial.is_some() => format!(
            "group {} not found in what was read; try --max-bytes N\n",
            escape(query)
        ),
        None => format!(
            "group {} not seen {label} ({}); try --since 7d\n",
            escape(query),
            summary.window.start.raw()
        ),
    }
}

pub(crate) fn ambiguous(matches: &[&LogGroup]) -> String {
    let mut out = String::from("That prefix matches several groups; use more letters:\n");
    for group in matches {
        out.push_str(&format!(
            "  {}  {}  {}  {}\n",
            group.full_id.raw(),
            group_count(group),
            escape(short_class(group.exception.raw())),
            escape(raw(&group.file).unwrap_or("-"))
        ));
    }
    out
}

/// Entries at the record's level or above, oldest first. The record has no
/// entry list, so the entries come from the snapshot it was built from.
pub(crate) fn raw_entries(summary: &LogsSummary, entries: &[Entry]) -> String {
    let min_level = Level::parse(summary.filter.level).unwrap_or(Level::Debug);
    let grep = raw(&summary.filter.text).map(str::to_lowercase);
    let mut out = header(summary);
    if summary.window.clock_changed {
        out.push_str("clock changed in this window; order across files approximate\n");
    }
    let mut entries: Vec<&Entry> = entries.iter().collect();
    entries.sort_by(|a, b| {
        a.time
            .cmp(&b.time)
            .then_with(|| a.file.cmp(&b.file))
            .then(a.offset.cmp(&b.offset))
    });
    for entry in entries {
        if entry.level >= min_level
            && grep
                .as_deref()
                .is_none_or(|grep| entry.text.to_lowercase().contains(grep))
        {
            out.push_str(&escape(&entry.text));
            out.push('\n');
        }
    }
    for block in &summary.unrecognized_samples {
        out.push_str(&format!("== {} / {} (not fully parsed: shown as-is, newest last, no --level/--since filtering) ==\n", block.channel.field(), block.file.field()));
        for line in block.text.raw().lines() {
            if grep
                .as_deref()
                .is_none_or(|grep| line.to_lowercase().contains(grep))
            {
                out.push_str(&escape(line));
                out.push('\n');
            }
        }
    }
    out
}

/// The discovery/coverage view uses the same bounded snapshot as the summary.
pub(crate) fn channels(summary: &LogsSummary) -> String {
    let mut out = header(summary);
    let labels: Vec<_> = summary
        .channels
        .iter()
        .map(|channel| {
            if channel.key == channel.name {
                channel.name.field()
            } else {
                format!("{} ({})", channel.name.field(), channel.key.field())
            }
        })
        .collect();
    let label_width = labels
        .iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(7, 26);
    let kind_width = summary
        .channels
        .iter()
        .map(|channel| kind_label(&channel.kind).len())
        .max()
        .unwrap_or(0)
        .max(4);
    let size_width = summary
        .channels
        .iter()
        .map(|channel| format!("{} B", channel.size).len())
        .max()
        .unwrap_or(0)
        .max(4);
    out.push_str(&format!(
        "\n{:<label_width$}  {:<kind_width$}  {:>5}  {:>size_width$}\n",
        "CHANNEL", "KIND", "FILES", "SIZE"
    ));
    for (channel, label) in summary.channels.iter().zip(labels) {
        let coverage = if channel.files.is_empty() {
            "no file".into()
        } else if complete(&channel.coverage) {
            "full".into()
        } else if !channel.changed_during_read.is_empty() {
            "changed during read".into()
        } else {
            format!(
                "partial: {} / {} bytes{}",
                channel.bytes_read,
                channel.size,
                covered_from(&channel.coverage)
                    .map(|from| format!(", since {from}"))
                    .unwrap_or_default()
            )
        };
        let baseline = if !channel.baseline.available {
            channel
                .baseline
                .format
                .as_ref()
                .filter(|format| has_unknown_content(format))
                .map(|format| format!("none ({})", format_label(format)))
                .unwrap_or_else(|| "none".into())
        } else {
            format!(
                "{} bytes, since {} ({})",
                channel.baseline.bytes_read,
                raw(&channel.baseline.from).unwrap_or("time unknown"),
                if complete(&channel.baseline.coverage) {
                    "full read"
                } else {
                    "partial read"
                }
            )
        };
        let short_label = if label.chars().count() > label_width {
            format!(
                "{}…",
                label.chars().take(label_width - 1).collect::<String>()
            )
        } else {
            label.clone()
        };
        out.push_str(&format!(
            "{:<label_width$}  {:<kind_width$}  {:>5}  {:>size_width$}\n",
            short_label,
            kind_label(&channel.kind),
            channel.files.len(),
            format!("{} B", channel.size)
        ));
        if short_label != label {
            push_wrapped(&mut out, "    NAME: ", &label);
        }
        if channel.omitted_files > 0 {
            push_wrapped(
                &mut out,
                "    FILES: ",
                &format!(
                    "{} selected (+{} omitted)",
                    channel.files.len(),
                    channel.omitted_files
                ),
            );
        }
        push_wrapped(
            &mut out,
            "    LAST WRITE: ",
            raw(&channel.last_write).unwrap_or("-"),
        );
        push_wrapped(&mut out, "    FORMAT: ", &format_label(&channel.format));
        push_wrapped(&mut out, "    COVERAGE: ", &coverage);
        push_wrapped(&mut out, "    BASELINE: ", &baseline);
        out.push('\n');
    }
    out
}
