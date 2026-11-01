use super::*;
use crate::logs_json_tests::read;
use shipslip::logs::{escape, Anchor, Entry, Format, RawBlock, RunCheck};

fn line(time: &str, level: &str, message: &str) -> String {
    format!("[{time}] production.{level}: {message}\n")
}

/// A one-channel snapshot read through the fake transport, with the window
/// and coverage fixed so text can be compared exactly.
async fn snapshot_of(text: &str, complete: bool) -> logs::Snapshot {
    let mut snapshot = read(text).await;
    snapshot.baseline_enabled = false;
    snapshot.start = "2026-10-01 00:00:00".into();
    snapshot.read_at = Some("2026-10-02 00:00:00".into());
    let channel = &mut snapshot.channels[0];
    channel.complete = complete;
    channel.covered_from = (!complete).then(|| "2026-10-01 09:00:00".into());
    channel.last_write = Some("2026-10-01 10:00:00".into());
    channel.baseline = Default::default();
    snapshot
}

async fn entries_in(text: &str, channel: usize, file: &str) -> Vec<Entry> {
    read(text)
        .await
        .entries
        .into_iter()
        .map(|mut entry| {
            entry.channel = channel;
            entry.file = file.into();
            entry
        })
        .collect()
}

fn summary_of(snapshot: &logs::Snapshot, level: Level, grep: Option<&str>, all: bool) -> String {
    let groups = logs::group(snapshot, level, grep);
    logs_words::summary(&records::logs_summary(snapshot, &groups, level, grep), all)
}

fn view(snapshot: &logs::Snapshot, options: LogsOptions) -> String {
    let level = options.level.unwrap_or(Level::Error);
    let grep = options.grep.as_deref();
    let groups = logs::group(snapshot, level, grep);
    logs_text(snapshot, &groups, level, grep, &options).0
}

/// The detail text of the group at `row`, folded unless `full`.
fn detail_of(snapshot: &logs::Snapshot, level: Level, row: usize, full: bool) -> String {
    let groups = logs::group(snapshot, level, None);
    let detail = records::logs_detail(snapshot, &groups.groups[row], level, None);
    logs_words::detail(&detail, full)
}

fn related_of(snapshot: &logs::Snapshot, level: Level, row: usize, args: &[String]) -> String {
    let groups = logs::group(snapshot, level, None);
    let detail = records::logs_detail(snapshot, &groups.groups[row], level, None);
    logs_words::related(&detail, args)
}

fn md_of(snapshot: &logs::Snapshot, level: Level, grep: Option<&str>, full: bool) -> String {
    let groups = logs::group(snapshot, level, grep);
    let detail = records::logs_detail(snapshot, &groups.groups[0], level, grep);
    logs_words::detail_md(&detail, full)
}

fn header_of(snapshot: &logs::Snapshot) -> String {
    logs_words::header(&summary_record(snapshot))
}

fn summary_record(snapshot: &logs::Snapshot) -> records::LogsSummary {
    records::logs_summary(snapshot, &logs::Groups::default(), Level::Debug, None)
}

fn raw_of(snapshot: &logs::Snapshot, level: Level, grep: Option<&str>) -> String {
    let summary = records::logs_summary(snapshot, &logs::Groups::default(), level, grep);
    logs_words::raw_entries(&summary, &snapshot.entries)
}

fn channels_of(snapshot: &logs::Snapshot) -> String {
    logs_words::channels(&summary_record(snapshot))
}

#[tokio::test]
async fn status_column_says_new_unknown_or_seen_and_names_channels_without_a_baseline() {
    let mut snapshot = snapshot_of("[2026-10-02 12:00:00] production.ERROR: KnownException: changed\n[2026-10-02 12:00:01] production.ERROR: NewException: new\n", true).await;
    snapshot.baseline_enabled = true;
    snapshot.channels[0].baseline.available = true;
    let mut missing = snapshot.channels[0].clone();
    missing.name = "queue".into();
    missing.baseline.available = false;
    snapshot.channels.push(missing);
    snapshot.entries.extend(
        entries_in(
            "[2026-10-02 12:00:02] production.ERROR: MissingException: no baseline\n",
            1,
            "queue.log",
        )
        .await,
    );
    snapshot.baseline_entries = entries_in(
        "[2026-10-01 12:00:00] production.ERROR: KnownException: previous\n",
        1,
        "queue.log",
    )
    .await;
    assert!(header_of(&snapshot).contains("no baseline: queue"));
    let summary = summary_of(&snapshot, Level::Error, None, false);
    let statuses: Vec<_> = summary
        .lines()
        .filter(|line| {
            line.split_whitespace()
                .next()
                .is_some_and(|word| word.parse::<usize>().is_ok())
        })
        .map(|row| row.split_whitespace().nth(3).unwrap())
        .collect();
    assert_eq!(statuses, ["NEW", "?", "seen"]);
}

#[tokio::test]
async fn a_merged_group_lists_its_channels_and_latest_entry() {
    let mut snapshot = snapshot_of(
        "[2026-10-02 12:00:00] production.ERROR: QueryException: newer\n",
        true,
    )
    .await;
    let mut second = snapshot.channels[0].clone();
    second.name = "payments".into();
    second.complete = false;
    snapshot.channels.push(second);
    snapshot.entries.extend(
        entries_in(
            "[2026-10-01 12:00:00] production.ERROR: QueryException: older\n",
            1,
            "payments.log",
        )
        .await,
    );
    assert!(detail_of(&snapshot, Level::Error, 0, true).contains("newer"));
    assert!(summary_of(&snapshot, Level::Error, None, false).contains("laravel, payments"));
}

#[tokio::test]
async fn raw_shows_unparsed_samples_escaped_and_grepped() {
    let text = format!(
        "plain worker stdout\n{}",
        line("2026-10-01 01:00:00", "ERROR", "RuntimeException: boom")
    );
    let mut snapshot = snapshot_of(&text, true).await;
    snapshot.channels[0].format = Format::Partial(10);
    snapshot.raw_blocks = vec![RawBlock {
        channel: 0,
        file: "worker.log".into(),
        text: "ordinary stdout\nPayment failed\x1b]0;secret\x07\n".into(),
    }];
    let raw = raw_of(&snapshot, Level::Emergency, Some("PAYMENT"));
    assert!(raw.contains("no --level/--since filtering"));
    assert!(raw.contains("Payment failed\\x1b]0;secret\\x07"));
    assert!(!raw.contains("ordinary stdout"));
    assert!(!raw.contains('\x1b'));
    assert!(header_of(&snapshot).contains("partly recognized (10% of lines) — see --raw"));
}

#[tokio::test]
async fn raw_level_is_a_threshold() {
    let text = [
        "DEBUG",
        "INFO",
        "NOTICE",
        "WARNING",
        "ERROR",
        "CRITICAL",
        "ALERT",
        "EMERGENCY",
    ]
    .iter()
    .enumerate()
    .map(|(n, level)| line(&format!("2026-10-01 01:00:0{n}"), level, "x"))
    .collect::<String>();
    let snapshot = snapshot_of(&text, true).await;
    for (name, kept) in [
        "debug",
        "info",
        "notice",
        "warning",
        "error",
        "critical",
        "alert",
        "emergency",
    ]
    .iter()
    .zip((1..=8).rev())
    {
        let raw = raw_of(&snapshot, Level::parse(name).unwrap(), None);
        assert_eq!(
            raw.lines().filter(|l| l.starts_with('[')).count(),
            kept,
            "{name}"
        );
    }
}

#[tokio::test]
async fn channels_view_has_a_stable_table_and_names_original_channel_after_rename() {
    let mut snapshot = snapshot_of("", true).await;
    snapshot.channels[0].name = "app".into();
    snapshot.channels[0].format = Format::Empty;
    let expected = "production · app · times UTC\nsince 24h (default) (2026-10-01 00:00:00)\n\nCHANNEL        KIND    FILES  SIZE\napp (laravel)  single      1   0 B\n    LAST WRITE: 2026-10-01 10:00:00\n    FORMAT: no lines in this window\n    COVERAGE: full\n    BASELINE: none\n\n";
    assert_eq!(channels_of(&snapshot), expected);
}

#[tokio::test]
async fn short_paths_handle_relative_logs_without_changing_the_detail() {
    let path = "/var/www/laravel/app/Services/Payment.php";
    let mut snapshot = snapshot_of(
        &line(
            "2026-10-01 01:00:00",
            "ERROR",
            &format!("PaymentException: failed at {path}:42"),
        ),
        true,
    )
    .await;
    snapshot.channels[0].files = vec!["storage/logs/laravel.log".into()];
    let summary = summary_of(&snapshot, Level::Error, None, false);
    assert!(summary.contains("app/Services/Payment.php · channels: laravel"));
    assert!(detail_of(&snapshot, Level::Error, 0, true).contains(path));

    let mut summary = summary_record(&snapshot);
    for (files, path, shown) in [
        (
            "storage/logs/laravel.log",
            "/opt/external/src/Failure.php",
            "/opt/external/src/Failure.php",
        ),
        (
            "storage/logs/laravel.log",
            "/var/www/laravel/database/migrations/x.php",
            "database/migrations/x.php",
        ),
        (
            "/srv/app/storage/logs/laravel.log",
            "/srv/app/routes/web.php",
            "routes/web.php",
        ),
        (
            "/srv/app/storage/logs/laravel.log",
            "/srv/app-copy/app/Failure.php",
            "/srv/app-copy/app/Failure.php",
        ),
    ] {
        summary.channels[0].files = vec![files.into()];
        assert_eq!(logs_words::short_path(&summary, path), shown);
    }
}

#[tokio::test]
async fn presentation_wraps_safely_and_raw_entries_retain_original_lines() {
    let message = format!("RuntimeException: {}\x1b[2J", "é".repeat(200));
    let mut snapshot = snapshot_of(&line("2026-10-01 01:00:00", "ERROR", &message), true).await;
    snapshot.channels[0].name = format!("{}\n\t", "channel-".repeat(20));
    snapshot.warnings.push("warning-word ".repeat(30));
    for output in [
        summary_of(&snapshot, Level::Error, None, false),
        channels_of(&snapshot),
    ] {
        assert!(output.lines().all(|line| line.chars().count() <= 96));
        assert!(!output.contains('\x1b'));
        assert!(output.contains("\\x0a\\x09"));
    }
    let raw = raw_of(&snapshot, Level::Error, None);
    assert!(raw.contains(&escape(&snapshot.entries[0].text)));
}

#[tokio::test]
async fn detail_names_the_full_decoded_class_and_keeps_the_entry() {
    let class = "Illuminate\\Database\\QueryException";
    let file = "/srv/app/app/Http/Controllers/InvoiceExportController.php";
    let context = serde_json::json!({"exception": format!("[object] ({class}(code: HY000): Missing table at {file}:16)")});
    let encoded = line(
        "2026-10-01 02:00:00",
        "ERROR",
        &format!("Missing table {context}"),
    );
    let snapshot = snapshot_of(&encoded, true).await;
    let detail = detail_of(&snapshot, Level::Error, 0, true);
    let first = detail.lines().next().unwrap();
    assert!(first.contains(class) && !first.contains(r"Illuminate\\Database"));
    assert!(detail.contains(encoded.trim_end()));
    assert!(raw_of(&snapshot, Level::Error, None).contains(encoded.trim_end()));
}

#[tokio::test]
async fn summary_shows_twenty_rows_then_points_to_all() {
    let text: String = (0..30)
        .map(|i| line("2026-10-01 01:00:00", "ERROR", &format!("E{i}Exception: x")))
        .collect();
    let snapshot = snapshot_of(&text, true).await;
    let summary = summary_of(&snapshot, Level::Error, None, false);
    assert!(summary.contains(" 20  "));
    assert!(!summary.contains(" 21  "));
    assert!(summary.contains("+10 more groups (--all)"));
    let all = summary_of(&snapshot, Level::Error, None, true);
    assert!(all.contains(" 30  "));
    assert!(!all.contains("more groups"));
}

#[tokio::test]
async fn partly_read_channels_mark_counts_as_lower_bounds_and_explain_a_miss() {
    let text = line("2026-10-01 10:00:00", "ERROR", "RuntimeException: x");
    let snapshot = snapshot_of(&text, false).await;
    let summary = summary_of(&snapshot, Level::Error, None, false);
    assert!(summary.contains("≥1"), "{summary}");
    assert!(summary.contains("covered: since 2026-10-01 09:00:00 only"));
    let miss = |snapshot: &logs::Snapshot| {
        view(
            snapshot,
            LogsOptions {
                query: Some("zz".into()),
                ..Default::default()
            },
        )
    };
    assert_eq!(miss(&snapshot), "group zz not found in what was read (entries since 2026-10-01 09:00:00 of a window from 2026-10-01 00:00:00); try --max-bytes N or a later --since\n");
    let complete = snapshot_of(&text, true).await;
    assert_eq!(
        miss(&complete),
        "group zz not seen since 24h (default) (2026-10-01 00:00:00); try --since 7d\n"
    );
}

#[tokio::test]
async fn a_failed_lookup_goes_to_stderr_with_the_text_exit_code() {
    let text: String = (0..20)
        .map(|n| {
            line(
                "2026-10-01 02:00:00",
                "ERROR",
                &format!("Problem{n}Exception: failed"),
            )
        })
        .collect();
    let snapshot = snapshot_of(&text, true).await;
    let groups = logs::group(&snapshot, Level::Error, None);
    let options = |query: &str| LogsOptions {
        query: Some(query.into()),
        ..Default::default()
    };
    let (_, code) = logs_text(&snapshot, &groups, Level::Error, None, &options("zzzz"));
    assert_eq!(code, 1);
    let prefix = (b'a'..=b'z')
        .map(|letter| (letter as char).to_string())
        .find(|prefix| {
            groups
                .groups
                .iter()
                .filter(|g| g.id.starts_with(prefix.as_str()))
                .count()
                > 1
        })
        .unwrap();
    let (text, code) = logs_text(&snapshot, &groups, Level::Error, None, &options(&prefix));
    assert_eq!(code, 2);
    assert!(text.starts_with("That prefix matches several groups; use more letters:\n"));
    assert_eq!(
        text.lines().count() - 1,
        groups
            .groups
            .iter()
            .filter(|g| g.id.starts_with(&prefix))
            .count()
    );
    assert_eq!(
        logs_text(&snapshot, &groups, Level::Error, None, &options("1")).1,
        0
    );
}

#[tokio::test]
async fn control_characters_are_escaped_in_every_view() {
    let text = line(
        "2026-10-01 01:00:00",
        "ERROR",
        "RuntimeException: \x1b[2Jboom",
    );
    let mut snapshot = snapshot_of(&text, true).await;
    snapshot.channels[0].name = "channel\x1b]0;title\x07".into();
    let groups = logs::group(&snapshot, Level::Error, Some("\x1b[2J"));
    let detail = records::logs_detail(&snapshot, &groups.groups[0], Level::Error, Some("\x1b[2J"));
    for out in [
        summary_of(&snapshot, Level::Error, None, false),
        logs_words::detail(&detail, true),
        logs_words::detail(&detail, false),
        logs_words::related(&detail, &["--since".into(), "\x1b[2J".into()]),
        logs_words::detail_md(&detail, false),
        raw_of(&snapshot, Level::Debug, None),
        channels_of(&snapshot),
    ] {
        assert!(!out.contains('\x1b'), "{out:?}");
    }
}

#[tokio::test]
async fn detail_keeps_the_whole_trace_and_the_summary_counts_untracked_groups() {
    let text: String = (0..550)
        .map(|i| {
            line(
                "2026-10-01 01:00:00",
                "ERROR",
                &format!("E{i}Exception: x\n[stacktrace]\n#0 full trace"),
            )
        })
        .collect();
    let snapshot = snapshot_of(&text, true).await;
    assert!(detail_of(&snapshot, Level::Error, 0, true).contains("#0 full trace"));
    assert!(summary_of(&snapshot, Level::Error, None, true).contains("+50 groups not tracked"));
}

/// A Laravel-shaped error entry: the origin and two app frames between
/// runs of framework frames, a short vendor run, and a chained exception.
/// Written by hand from Laravel's LineFormatter layout; no real log data.
const TRACE: &str = "Class not found {\"exception\":\"[object] (Error(code: 0): Class not found at /srv/acme-vendor/app/Http/Controllers/InvoiceController.php:16)\n\
[stacktrace]\n\
#0 /srv/acme-vendor/vendor/laravel/framework/src/Illuminate/Routing/Controller.php(54): handle()\n\
#1 /srv/acme-vendor/vendor/laravel/framework/src/Illuminate/Routing/ControllerDispatcher.php(43): callAction()\n\
#2 /srv/acme-vendor/vendor/laravel/framework/src/Illuminate/Routing/Route.php(260): dispatch()\n\
#3 /srv/acme-vendor/vendor/laravel/framework/src/Illuminate/Routing/Route.php(205): runController()\n\
#4 /srv/acme-vendor/app/Http/Middleware/Tenant.php(31): run()\n\
#5 /srv/acme-vendor/vendor/laravel/framework/src/Illuminate/Pipeline/Pipeline.php(183): handle()\n\
#6 /srv/acme-vendor/vendor/laravel/framework/src/Illuminate/Pipeline/Pipeline.php(119): Illuminate\\\\Pipeline\\\\{closure}()\n\
#7 [internal function]: Illuminate\\\\Foundation\\\\Http\\\\Kernel->handle()\n\
#8 /srv/acme-vendor/public/index.php(52): handle()\n\
#9 {main}\n\
[previous exception] [object] (LogicException(code: 0): earlier at /srv/acme-vendor/app/Models/Invoice.php:9)\n\
[stacktrace]\n\
#0 /srv/acme-vendor/vendor/a/b/One.php(1): x()\n\
#1 /srv/acme-vendor/vendor/a/b/Two.php(2): x()\n\
#2 /srv/acme-vendor/vendor/a/b/Three.php(3): x()\n\
#3 /srv/acme-vendor/vendor/a/b/Four.php(4): x()\n\
#4 {main}\n\
\"}";

#[tokio::test]
async fn folding_keeps_the_origin_app_frames_and_short_runs() {
    let snapshot = snapshot_of(&line("2026-10-01 01:00:00", "ERROR", TRACE), true).await;
    let folded = detail_of(&snapshot, Level::Error, 0, false);
    // The origin stays even though it is a framework frame.
    assert!(folded.contains(
        "#0 /srv/acme-vendor/vendor/laravel/framework/src/Illuminate/Routing/Controller.php(54)"
    ));
    // #1–#3 fold; the app frame after them stays.
    assert!(folded.contains("… 3 vendor frames (--full to show)\n#4 /srv/acme-vendor/app/Http/Middleware/Tenant.php(31)"));
    assert!(!folded.contains("ControllerDispatcher.php"));
    // A run of two stays, and so do frames whose path can't be read.
    assert!(folded.contains("Pipeline.php(183)") && folded.contains("Pipeline.php(119)"));
    assert!(folded.contains("#7 [internal function]") && folded.contains("#9 {main}"));
    // The chained exception starts again at #0 and folds its own run.
    assert!(folded.contains("[previous exception]"));
    assert!(folded.contains("#0 /srv/acme-vendor/vendor/a/b/One.php(1): x()\n… 3 vendor frames (--full to show)\n#4 {main}"));
    assert!(folded.ends_with("\"}\n"));
    let full = detail_of(&snapshot, Level::Error, 0, true);
    assert!(full.contains("ControllerDispatcher.php") && !full.contains("vendor frames ("));

    let plain = snapshot_of(
        &line(
            "2026-10-01 01:00:00",
            "ERROR",
            "RuntimeException: no frames here",
        ),
        true,
    )
    .await;
    assert!(
        detail_of(&plain, Level::Error, 0, false).ends_with("RuntimeException: no frames here\n")
    );
}

#[tokio::test]
async fn a_truncated_entry_says_so_after_the_folded_trace() {
    let mut snapshot = snapshot_of(&line("2026-10-01 01:00:00", "ERROR", TRACE), true).await;
    snapshot.entries[0].truncated = true;
    assert!(detail_of(&snapshot, Level::Error, 0, false)
        .ends_with("\"}\nEntry truncated at 256 KiB.\n"));
}

#[tokio::test]
async fn level_counts_cover_the_whole_window_and_mark_lower_bounds() {
    let text = [
        line("2026-10-01 01:00:00", "ERROR", "RuntimeException: a"),
        line("2026-10-01 01:00:01", "ERROR", "RuntimeException: b"),
        line("2026-10-01 01:00:02", "WARNING", "slow"),
        line("2026-10-01 01:00:03", "INFO", "hello"),
        line("2026-10-01 01:00:04", "CRITICAL", "LogicException: c"),
    ]
    .concat();
    let snapshot = snapshot_of(&text, true).await;
    // Filters change the groups, never the window totals.
    let summary = summary_of(&snapshot, Level::Critical, Some("nothing"), false);
    assert!(summary.contains("in this window: 1 critical · 2 error · 1 warning · 1 info\n"));
    assert!(summary.contains("No entries match --grep at critical level or above.\n"));
    assert!(summary_of(&snapshot, Level::Emergency, None, false)
        .contains("No entries at emergency level or above"));

    // Partly read: every count is a minimum.
    let partial = snapshot_of(&text, false).await;
    assert!(summary_of(&partial, Level::Error, None, false).contains("in this window: at least 1 critical · 2 error · 1 warning · 1 info (some channels partly read)"));

    // Fully read but partly understood counts the same way.
    let mut unknown = snapshot_of(&text, true).await;
    unknown.channels[0].format = Format::Partial(80);
    let summary = summary_of(&unknown, Level::Error, Some("nothing"), false);
    assert!(summary.contains("in this window: at least"));
    assert!(summary.contains("log coverage is incomplete"));

    let empty = snapshot_of("", true).await;
    assert!(!summary_of(&empty, Level::Error, None, false).contains("in this window:"));
}

#[tokio::test]
async fn counts_are_minimums_when_content_was_not_understood() {
    let text = [
        line("2026-10-01 01:00:00", "ERROR", "RuntimeException: a"),
        line("2026-10-01 01:00:01", "ERROR", "RuntimeException: a"),
    ]
    .concat();
    let mut snapshot = snapshot_of(&text, true).await;
    assert!(detail_of(&snapshot, Level::Error, 0, false).contains("×2"));
    snapshot.channels[0].format = Format::Unrecognized;
    let detail = detail_of(&snapshot, Level::Error, 0, false);
    assert!(detail.contains("Count:      ≥2"));
    assert!(detail.contains("First seen: —"));
    assert!(!detail.contains("×2"), "{detail}");
}

#[tokio::test]
async fn markdown_detail_is_inert_and_says_where_it_came_from() {
    let hostile = "RuntimeException: <img src=x onerror=alert(1)> [click](https://evil.example) ``` # heading **bold** | cell";
    let text = line(
        "2026-10-01 01:00:00",
        "WARNING",
        &format!("{hostile}\n{}", TRACE.split_once('\n').unwrap().1),
    )
    .replace("<img", "pay`ment <img");
    let mut snapshot = snapshot_of(&text, true).await;
    snapshot.env = "prod<b>".into();
    let md = md_of(&snapshot, Level::Warning, Some("pay`ment"), false);

    assert!(md.contains("- Environment: prod\\<b\\>\n"), "{md}");
    assert!(md.contains("- Log times: UTC\n"));
    assert!(md.contains(
        "- Window: since 24h \\(default\\) (2026-10-01 00:00:00 to 2026-10-02 00:00:00)\n"
    ));
    assert!(md.contains("- Coverage: complete\n"));
    assert!(md.contains("- Baseline: off\n"));
    assert!(md.contains("- Filters: warning or above; --grep ``pay`ment``\n"));
    assert!(md.contains(&format!("- slip: {}\n", env!("CARGO_PKG_VERSION"))));
    // The entry sits in a fence longer than its own ``` run.
    let (_, body) = md.split_once("Latest entry").unwrap();
    assert!(body.contains("\n````text\n") && body.trim_end().ends_with("````"));
    // Outside the fence, nothing renders: tags, links and emphasis are escaped.
    let (outside, _) = md.split_once("````text").unwrap();
    for raw in ["<img", "[click](", "**bold**", "](https"] {
        assert!(!outside.contains(raw), "{raw} unescaped in {outside}");
    }
    // Folds like the terminal unless --full.
    assert!(md.contains("… 3 vendor frames (--full to show)"));
    let full = md_of(&snapshot, Level::Warning, Some("pay`ment"), true);
    assert!(!full.contains("vendor frames (") && full.contains("ControllerDispatcher.php"));

    let mut partial = snapshot_of(&text, false).await;
    partial.read_at = None;
    let md = md_of(&partial, Level::Warning, None, false);
    assert!(md.contains("- Coverage: incomplete; counts are lower bounds\n"));
    assert!(md.contains("to time unknown)"));
    assert!(md.contains("- Count: ≥1\n") && md.contains("- First seen: —\n"));
    assert!(md.contains("- Filters: warning or above\n"));
}

#[tokio::test]
async fn summary_labels_drop_laravel_context_and_extra_but_detail_keeps_them() {
    let text = [
        line(
            "2026-10-01 01:00:00",
            "ERROR",
            "Invoice backfill failed: column totals_cents is missing (shipslip test) {\"exception\":\"[object] (RuntimeException(code: 0): Invoice backfill failed\n[stacktrace]\n#0 {main}\n\"}",
        ),
        line(
            "2026-10-01 01:00:01",
            "ERROR",
            "User authenticated. {\"auth_id\":27} {\"url\":\"https://example.com/login\",\"trace_id\":\"e04e1a11\"}",
        ),
    ]
    .concat();
    let snapshot = snapshot_of(&text, true).await;
    let summary = summary_of(&snapshot, Level::Error, None, false);
    assert!(
        summary.contains(
            "    Invoice backfill failed: column totals_cents is missing (shipslip test)\n"
        ),
        "{summary}"
    );
    assert!(summary.contains("    User authenticated.\n"), "{summary}");
    assert!(!summary.contains("{\"exception\""));
    assert!((0..2)
        .any(|row| detail_of(&snapshot, Level::Error, row, false)
            .contains("{\"exception\":\"[object]")));
}

#[tokio::test]
async fn same_request_lines_share_key_and_value_in_time_order() {
    let lines = [
        line(
            "2026-10-01 01:00:05",
            "WARNING",
            "Checkout started  {\"trace_id\":\"trace-aaaaaaaa\"}",
        ),
        line(
            "2026-10-01 01:00:01",
            "INFO",
            "Cart loaded  {\"trace_id\":\"trace-aaaaaaaa\"}",
        ),
        line(
            "2026-10-01 01:00:02",
            "INFO",
            "Other request  {\"trace_id\":\"trace-bbbbbbbb\"}",
        ),
        line(
            "2026-10-01 01:00:03",
            "INFO",
            "Same value, other key  {\"job_id\":\"trace-aaaaaaaa\"}",
        ),
        line(
            "2026-10-01 01:00:09",
            "ERROR",
            "Payment failed  {\"trace_id\":\"trace-aaaaaaaa\"}",
        ),
    ]
    .concat();
    let mut snapshot = snapshot_of(&lines, true).await;
    snapshot.entries[1].time = None;
    let out = related_of(&snapshot, Level::Error, 0, &[]);
    assert!(
        out.contains(
            "Shared trace_id trace-aaaaaaaa (correlation, not cause; can include queued jobs):"
        ),
        "{out}"
    );
    assert!(out.contains("WARNING  Checkout started\n"), "{out}");
    assert!(
        out.contains("ERROR    Payment failed  ← this entry\n"),
        "{out}"
    );
    // Untimed entries sort after timed ones.
    assert!(
        out.ends_with("← this entry\n  time unknown  laravel  INFO     Cart loaded\n"),
        "{out}"
    );
    assert!(!out.contains("Other request") && !out.contains("other key"));
    assert!(!out.contains("more;"));
}

#[tokio::test]
async fn same_request_lines_cap_each_side_and_print_a_broader_search() {
    let text: String = (0..25)
        .map(|second| {
            line(
                &format!("2026-10-01 01:00:{second:02}"),
                if second == 12 { "ERROR" } else { "INFO" },
                &format!("step {second}  {{\"request_id\":\"req-12345678\"}}"),
            )
        })
        .collect();
    let snapshot = snapshot_of(&text, false).await;
    let args = [
        "--since".to_string(),
        "2026-10-01 00:00".into(),
        "--max-bytes".into(),
        "20m".into(),
    ];
    let out = related_of(&snapshot, Level::Error, 0, &args);
    let shown: Vec<_> = out
        .lines()
        .filter_map(|line| line.split_once("  INFO     step "))
        .map(|(_, step)| step.parse::<u32>().unwrap())
        .collect();
    assert_eq!(shown, (2..12).chain(13..23).collect::<Vec<_>>());
    assert!(!out.contains("queued jobs"));
    assert!(out.contains("+4 more; broader text search: slip logs production --raw --grep req-12345678 --since '2026-10-01 00:00' --max-bytes 20m"), "{out}");
    assert!(out.contains("(some channels only partly read)"));
    // The suggested search shows at least every matching line.
    let raw = raw_of(&snapshot, Level::Debug, Some("req-12345678"));
    for entry in &snapshot.entries {
        assert!(raw.contains(&escape(&entry.text)));
    }
}

#[tokio::test]
async fn entries_without_an_id_get_a_tip_or_a_cut_note() {
    let mut snapshot = snapshot_of(
        &[
            line("2026-10-01 01:00:00", "ERROR", "RuntimeException: boom"),
            line("2026-10-01 01:00:01", "WARNING", "slow query"),
        ]
        .concat(),
        true,
    )
    .await;
    let groups = logs::group(&snapshot, Level::Warning, None);
    let row = |error: bool| {
        groups
            .groups
            .iter()
            .position(|group| (group.class == "RuntimeException") == error)
            .unwrap()
    };
    assert!(related_of(&snapshot, Level::Warning, row(true), &[])
        .contains("No request ID in this entry. Laravel's Context::add"));
    assert_eq!(related_of(&snapshot, Level::Warning, row(false), &[]), "");

    snapshot.entries[0].truncated = true;
    assert_eq!(
        related_of(&snapshot, Level::Error, 0, &[]),
        "\nEntry was cut at 256 KiB, so an ID at its end may be missing.\n"
    );

    let alone = snapshot_of(
        &line(
            "2026-10-01 01:00:00",
            "ERROR",
            "Boom  {\"request_id\":\"req-12345678\"}",
        ),
        true,
    )
    .await;
    assert!(related_of(&alone, Level::Error, 0, &[])
        .contains("no other entry in this window has this ID"));
}

#[test]
fn window_labels_name_the_anchor_kind() {
    let sha = "a".repeat(40);
    let run = |check: RunCheck, outcome: Option<&str>, reflog_recorded: bool| Anchor::Run {
        run_id: "r-one".into(),
        plan: "deploy".into(),
        target_commit: sha.as_str().into(),
        check,
        server_marker_outcome: outcome.map(Into::into),
        reflog_recorded,
    };
    let cases = [
        (Anchor::Default { seconds: 86400 }, "since 24h (default)"),
        (
            Anchor::Since {
                since: Since::Ago(7200),
            },
            "since 2h",
        ),
        (
            Anchor::Since {
                since: Since::Ago(86400),
            },
            "since 1d",
        ),
        (
            Anchor::Since {
                since: Since::Ago(90),
            },
            "since 1m",
        ),
        (
            Anchor::Since {
                since: Since::At("2026-10-01 14:00".into()),
            },
            "since 2026-10-01 14:00",
        ),
        (
            run(RunCheck::Verified, Some("succeeded"), true),
            "since slip deploy r-one (aaaaaaa, succeeded)",
        ),
        (
            run(RunCheck::Verified, Some("succeeded"), false),
            "since slip deploy r-one (aaaaaaa, outcome unknown)",
        ),
        (
            run(RunCheck::Verified, None, true),
            "since slip deploy r-one (aaaaaaa, outcome unknown)",
        ),
        (
            run(
                RunCheck::Unverified {
                    reason: "reflog was rewritten".into(),
                },
                Some("succeeded"),
                true,
            ),
            "since slip deploy r-one (aaaaaaa, unverified: reflog was rewritten)",
        ),
        (
            Anchor::RunInProgress {
                run_id: "r-one".into(),
                step: Some(2),
            },
            "deploy in progress (run r-one, step 2); since run start",
        ),
        (
            Anchor::RunInProgress {
                run_id: "r-one".into(),
                step: None,
            },
            "deploy in progress (run r-one); since run start",
        ),
        (
            Anchor::CheckoutChange {
                commit: sha.as_str().into(),
            },
            "since checkout change aaaaaaa",
        ),
    ];
    for (anchor, label) in cases {
        assert_eq!(logs_words::window_label(&anchor), label);
    }
}
