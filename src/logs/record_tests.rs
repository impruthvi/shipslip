use super::*;
use crate::records::{logs_summary, versioned};
use serde_json::json;

fn fixture(text: &str) -> Snapshot {
    Snapshot {
        env: "production".into(),
        zone: "UTC".into(),
        anchor: Anchor::Default { seconds: 86400 },
        anchor_fallbacks: vec![],
        start: "2026-10-01 00:00:00".into(),
        read_at: Some("2026-10-02 00:00:00".into()),
        channels: vec![Channel {
            name: "laravel".into(),
            key: "private".into(),
            kind: ChannelKind::Single,
            source: "storage/logs/laravel.log".into(),
            files: vec!["storage/logs/laravel.log".into()],
            budget: 4096,
            bytes_read: text.len() as u64,
            complete: true,
            covered_from: None,
            changed: vec![],
            size: text.len() as u64,
            last_write: None,
            format: Format::Recognized,
            omitted_files: 0,
            baseline: Baseline::default(),
        }],
        entries: parse_entries(text.as_bytes(), 0, 0, "storage/logs/laravel.log"),
        raw_blocks: vec![],
        baseline_entries: vec![],
        baseline_enabled: false,
        clock_changed: false,
        warnings: vec![],
        bytes_transferred: 42,
    }
}

#[test]
fn summary_json_has_a_typed_window_and_no_baseline_without_a_comparison() {
    let snapshot = fixture("[2026-10-01 02:00:00] production.ERROR: RuntimeException: failed at /srv/app/app/Job.php:17\n");
    let groups = group(&snapshot, Level::Error, None);
    let record = serde_json::to_value(versioned(logs_summary(
        &snapshot,
        &groups,
        Level::Error,
        None,
    )))
    .unwrap();
    assert_eq!(record["schema"], 1);
    assert_eq!(
        record["window"],
        json!({
            "start":"2026-10-01 00:00:00", "end":"2026-10-02 00:00:00", "zone":"UTC",
            "clock_changed":false, "anchor":{"kind":"default", "seconds":86400}, "anchor_fallbacks":[]
        })
    );
    assert_eq!(record["groups"][0]["badge"], "no_baseline");
    assert_eq!(record["groups"][0]["exception"], "RuntimeException");
    assert_eq!(record["groups"][0]["channels"], json!(["laravel"]));
    assert_eq!(record["groups"][0]["count_is_lower_bound"], false);
    assert_eq!(record["groups"][0]["full_id"].as_str().unwrap().len(), 16);
    assert_eq!(record["filter"], json!({"level":"error", "text":null}));
    assert!(record.to_string().find("offset").is_none());
    assert!(record.to_string().find("private").is_none());
}

#[test]
fn detail_keeps_full_evidence_frames_and_same_request_correlation() {
    let context = json!({"exception":"[object] (RuntimeException: failed at /srv/app/app/Job.php:17)\n[stacktrace]\n#0 /srv/app/app/Job.php(17): Job->handle()\n#1 /srv/app/vendor/Queue.php(42): Queue->run()\n[previous exception] [object] (LogicException: earlier)\n#0 /srv/app/app/Worker.php(12): Worker->work()\n#1 {main}\n", "request_id":"request-one"}).to_string().replace("\\n", "\n");
    let text = format!("[2026-10-01 01:00:00] production.INFO: before {{\"request_id\":\"request-one\"}}\n[2026-10-01 02:00:00] production.ERROR: failed {context} {{\"request_id\":\"request-two\"}}\n[2026-10-01 03:00:00] production.INFO: after {{\"request_id\":\"request-one\"}}\n");
    let snapshot = fixture(&text);
    let groups = group(&snapshot, Level::Error, None);
    let record = serde_json::to_value(crate::records::logs_detail(
        &snapshot,
        &groups.groups[0],
        Level::Error,
        None,
    ))
    .unwrap();
    assert_eq!(record["latest"]["text"], snapshot.entries[1].text);
    assert_eq!(record["latest"]["bytes"], snapshot.entries[1].text.len());
    assert_eq!(record["latest"]["clipped"], false);
    assert_eq!(
        record["latest"]["frames"],
        json!([
            {"index":0,"file":"/srv/app/app/Job.php","line":17,"call":"Job->handle()","vendor":false,"chain":0},
            {"index":1,"file":"/srv/app/vendor/Queue.php","line":42,"call":"Queue->run()","vendor":true,"chain":0},
            {"index":0,"file":"/srv/app/app/Worker.php","line":12,"call":"Worker->work()","vendor":false,"chain":1},
            {"index":1,"file":null,"line":null,"call":"{main}","vendor":false,"chain":1}
        ])
    );
    assert_eq!(record["same_request"]["kind"], "shared");
    assert_eq!(record["same_request"]["key"], "request_id");
    assert_eq!(record["same_request"]["value"], "request-one");
    assert_eq!(record["same_request"]["conflict"], true);
    assert_eq!(
        record["same_request"]["before"].as_array().unwrap().len(),
        1
    );
    assert_eq!(record["same_request"]["after"].as_array().unwrap().len(), 1);
    assert_eq!(
        record["same_request"]["before"][0],
        json!({
            "time":"2026-10-01 01:00:00", "level":"info", "channel":"laravel",
            "file":"storage/logs/laravel.log", "text":snapshot.entries[0].text,
            "truncated":false, "clipped":false, "bytes":snapshot.entries[0].text.len()
        })
    );
    assert_eq!(record["same_request"]["more"], 0);
    assert_eq!(record["same_request"]["may_be_missing"], false);
}

#[test]
fn detail_marks_a_truncated_entry_and_keeps_every_same_request_entry_in_the_cli() {
    let text = (0..25)
        .map(|second| {
            let level = if second == 12 { "ERROR" } else { "INFO" };
            format!("[2026-10-01 01:00:{second:02}] production.{level}: step {second} {{\"request_id\":\"req-12345678\"}}\n")
        })
        .collect::<String>();
    let mut snapshot = fixture(&text);
    snapshot.entries[12].truncated = true;
    snapshot.channels[0].complete = false;
    let groups = group(&snapshot, Level::Error, None);
    let record = serde_json::to_value(crate::records::logs_detail(
        &snapshot,
        &groups.groups[0],
        Level::Error,
        None,
    ))
    .unwrap();
    assert_eq!(record["latest"]["truncated"], true);
    assert_eq!(record["latest"]["text"], snapshot.entries[12].text);
    let shared = &record["same_request"];
    assert_eq!(shared["kind"], "shared");
    assert_eq!(shared["before"].as_array().unwrap().len(), 12);
    assert_eq!(shared["after"].as_array().unwrap().len(), 12);
    assert_eq!(shared["more"], 0);
    assert_eq!(shared["may_be_missing"], true);
}

#[test]
fn summary_preserves_every_group_badge_count_variant_and_overflow() {
    let text = (0..501).map(|index| format!("[2026-10-01 02:00:00] production.ERROR: Problem{index}Exception: failed at /srv/app/app/Job.php:17\n")).collect::<String>();
    let mut snapshot = fixture(&text);
    snapshot.baseline_enabled = true;
    snapshot.channels[0].complete = false;
    snapshot.channels[0].baseline.available = true;
    snapshot.baseline_entries = parse_entries(
        text.lines().next().unwrap().as_bytes(),
        0,
        0,
        "baseline.log",
    );
    let groups = group(&snapshot, Level::Error, None);
    let record = serde_json::to_value(logs_summary(
        &snapshot,
        &groups,
        Level::Error,
        Some("failed"),
    ))
    .unwrap();
    assert_eq!(
        record["groups"].as_array().unwrap().len(),
        groups.groups.len()
    );
    assert!(record["groups"].as_array().unwrap().len() > 20);
    for (record, group) in record["groups"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&groups.groups)
    {
        assert_eq!(record["full_id"], group.full_id);
        assert_eq!(record["id"], group.id);
        assert_eq!(record["exception"], group.class);
        assert_eq!(record["file"], json!(group.file));
        assert_eq!(record["count"], group.count);
        assert_eq!(record["first_seen"], json!(group.first_seen));
        assert_eq!(record["last_seen"], json!(group.last_seen));
        assert_eq!(record["message"], group.message);
        assert_eq!(record["count_is_lower_bound"], true);
        assert_eq!(
            record["badge"],
            if group.class == "Problem0Exception" {
                "seen"
            } else {
                "new"
            }
        );
        let expected = match group.badge {
            Badge::New => "new",
            Badge::Seen => "seen",
            Badge::Unknown => "no_baseline",
        };
        assert_eq!(record["badge"], expected);
        assert_eq!(record["variants"][0]["badge"], expected);
        assert_eq!(record["variants"][0]["count"], 1);
        assert_eq!(record["variants"][0]["file"], "/srv/app/app/Job.php");
        assert_eq!(record["variants"][0]["line"], 17);
    }
    assert_eq!(record["overflow_groups"], groups.overflow);
    let text = (0..60).map(|index| format!("[2026-10-01 02:00:00] production.ERROR: RuntimeException: variant-{}{} at /srv/app/app/Job.php:17\n", char::from(b'A' + (index / 26) as u8), char::from(b'a' + (index % 26) as u8))).collect::<String>();
    let snapshot = fixture(&text);
    let groups = group(&snapshot, Level::Error, None);
    let record =
        serde_json::to_value(logs_summary(&snapshot, &groups, Level::Error, None)).unwrap();
    assert!(record["groups"][0]["overflow_variants"].as_u64().unwrap() > 0);
    assert!(record["groups"][0]["variants"]
        .as_array()
        .unwrap()
        .iter()
        .all(|variant| variant["badge"] == "no_baseline"));
}

#[test]
fn channels_report_all_formats_coverage_samples_and_empty_snapshots() {
    let mut snapshot = fixture("");
    snapshot.channels.clear();
    snapshot.warnings.push("no log files".into());
    let empty = serde_json::to_value(logs_summary(
        &snapshot,
        &Groups::default(),
        Level::Error,
        None,
    ))
    .unwrap();
    assert_eq!(empty["channels"], json!([]));
    assert_eq!(empty["warnings"], json!(["no log files"]));
    let template = fixture("").channels.remove(0);
    for (index, (format, expected)) in [
        (Format::Recognized, json!({"kind":"recognized"})),
        (Format::Partial(37), json!({"kind":"partial","percent":37})),
        (Format::Unrecognized, json!({"kind":"unrecognized"})),
        (Format::Empty, json!({"kind":"empty"})),
        (Format::NotRead, json!({"kind":"not_read"})),
    ]
    .into_iter()
    .enumerate()
    {
        let mut channel = template.clone();
        channel.format = format;
        channel.kind = match index % 3 {
            0 => ChannelKind::Single,
            1 => ChannelKind::Daily,
            _ => ChannelKind::Mixed,
        };
        channel.complete = index == 0;
        channel.covered_from = Some("2026-10-01 12:00:00".into());
        channel.omitted_files = 2;
        channel.changed = vec!["rotated.log".into()];
        snapshot.channels = vec![channel];
        snapshot.raw_blocks = vec![RawBlock {
            channel: 0,
            file: "odd.log".into(),
            text: "<b>data</b>\u{1b}[2J".into(),
        }];
        let value = serde_json::to_value(logs_summary(
            &snapshot,
            &Groups::default(),
            Level::Error,
            None,
        ))
        .unwrap();
        assert_eq!(value["channels"][0]["format"], expected);
        assert_eq!(
            value["channels"][0]["coverage"],
            if index == 0 {
                json!({"kind":"complete"})
            } else {
                json!({"kind":"partial","from":"2026-10-01 12:00:00"})
            }
        );
        assert_eq!(value["channels"][0]["omitted_files"], 2);
        assert_eq!(value["channels"][0]["name"], "laravel");
        assert_eq!(
            value["channels"][0]["files"],
            json!(["storage/logs/laravel.log"])
        );
        assert_eq!(value["channels"][0]["size"], 0);
        assert_eq!(value["channels"][0]["last_write"], json!(null));
        assert_eq!(
            value["channels"][0]["baseline"],
            json!({"available":false,"coverage":{"kind":"partial","from":null},"to":null})
        );
        assert_eq!(value["bytes_transferred"], 42);
        assert_eq!(
            value["channels"][0]["changed_during_read"],
            json!(["rotated.log"])
        );
        assert_eq!(
            value["unrecognized_samples"][0]["text"],
            "<b>data</b>\u{1b}[2J"
        );
        assert_eq!(value["unrecognized_samples"][0]["clipped"], false);
    }
}

#[test]
fn detail_distinguishes_no_id_from_cut_and_odd_traces_are_not_errors() {
    for cut in [false, true] {
        let mut snapshot = fixture("[2026-10-01 02:00:00] production.ERROR: RuntimeException: odd\n#not-a-frame\n#2 not a location\n");
        snapshot.entries[0].truncated = cut;
        let groups = group(&snapshot, Level::Error, None);
        let value = serde_json::to_value(crate::records::logs_detail(
            &snapshot,
            &groups.groups[0],
            Level::Error,
            None,
        ))
        .unwrap();
        assert_eq!(
            value["same_request"]["kind"],
            if cut { "cut" } else { "no_id" }
        );
        assert_eq!(value["latest"]["frames"], json!([]));
    }
}

#[test]
fn window_serializes_all_anchor_and_fallback_evidence_without_english_labels() {
    let mut snapshot = fixture("");
    for (anchor, expected) in [
        (
            Anchor::Run {
                run_id: "run-one".into(),
                plan: "deploy".into(),
                target_commit: "abc".into(),
                check: RunCheck::Verified,
                server_marker_outcome: Some("untrusted outcome".into()),
                reflog_recorded: true,
            },
            json!({"kind":"run","run_id":"run-one","plan":"deploy","target_commit":"abc","check":{"kind":"verified"},"server_marker_outcome":"untrusted outcome"}),
        ),
        (
            Anchor::RunInProgress {
                run_id: "run-two".into(),
                step: Some(3),
            },
            json!({"kind":"run_in_progress","run_id":"run-two","step":3}),
        ),
        (
            Anchor::CheckoutChange {
                commit: "abc".into(),
            },
            json!({"kind":"checkout_change","commit":"abc"}),
        ),
        (
            Anchor::Since {
                since: Since::Ago(3600),
            },
            json!({"kind":"since","since":{"kind":"ago","value":3600}}),
        ),
        (
            Anchor::Since {
                since: Since::At("2026-10-01 12:00".into()),
            },
            json!({"kind":"since","since":{"kind":"at","value":"2026-10-01 12:00"}}),
        ),
        (
            Anchor::Default { seconds: 86400 },
            json!({"kind":"default","seconds":86400}),
        ),
    ] {
        snapshot.anchor = anchor;
        let record = serde_json::to_value(logs_summary(
            &snapshot,
            &Groups::default(),
            Level::Error,
            None,
        ))
        .unwrap();
        assert_eq!(record["window"]["anchor"], expected);
    }
    snapshot.anchor = Anchor::Run {
        run_id: "run-one".into(),
        plan: "rerun".into(),
        target_commit: "abc".into(),
        check: RunCheck::Unverified {
            reason: "server says no reflog".into(),
        },
        server_marker_outcome: None,
        reflog_recorded: false,
    };
    snapshot.anchor_fallbacks = vec![
        AnchorFallback::Run {
            run_id: "run-one".into(),
            reason: RunFallbackReason::TargetNotReached {
                target_commit: "def".into(),
            },
        },
        AnchorFallback::LastRunMarker {
            reason: MarkerFallbackReason::Unreadable {
                message: "server text".into(),
            },
        },
        AnchorFallback::CheckoutChange {
            reason: CheckoutFallbackReason::HeadReadFailed {
                message: "server text".into(),
            },
        },
    ];
    let record = serde_json::to_value(logs_summary(
        &snapshot,
        &Groups::default(),
        Level::Error,
        None,
    ))
    .unwrap();
    assert_eq!(
        record["window"]["anchor"]["check"],
        json!({"kind":"unverified","reason":"server says no reflog"})
    );
    assert_eq!(
        record["window"]["anchor_fallbacks"],
        json!([
            {"kind":"run","run_id":"run-one","reason":{"kind":"target_not_reached","target_commit":"def"}},
            {"kind":"last_run_marker","reason":{"kind":"unreadable","message":"server text"}},
            {"kind":"checkout_change","reason":{"kind":"head_read_failed","message":"server text"}}
        ])
    );
}
