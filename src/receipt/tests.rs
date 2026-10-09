use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

use super::*;
use crate::{AbortReason, WatchStatus};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "shipslip-receipt-view-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A finished, successful v1 receipt as written before `actor` existed.
fn receipt_json(run_id: &str, started_at_ms: u64, env: &str) -> Value {
    let target = json!({
        "env": env,
        "production": false,
        "ssh_alias": "app-staging",
        "path": "/var/www/app",
        "branch": "main",
        "steps": ["php artisan migrate --force", "php artisan optimize"],
        "maintenance": true,
        "watch_log": true,
        "log": null,
        "log_daily": false,
        "smoke_url": "https://example.test/"
    });
    let watch = json!({
        "status": "Complete",
        "log_path": "storage/logs/laravel.log",
        "duration_ms": 120000,
        "baseline_signatures": 3,
        "history_signatures": 2,
        "observed_lines": 10,
        "parsed_lines": 10,
        "dropped_view_lines": 0,
        "truncated_entries": 0,
        "new_errors": [],
        "overflow_groups": 0,
        "overflow_signatures": 0,
        "warnings": []
    });
    json!({
        "version": 1,
        "project": "app",
        "repo_root": "/repo",
        "run_id": run_id,
        "owner_pid": 1,
        "confirmed": true,
        "started_at_ms": started_at_ms,
        "finished_at_ms": started_at_ms + 164_000,
        "status": "Final",
        "phase": "Finished",
        "target": target,
        "run_plan": "Deploy",
        "from_sha": "1111111111111111111111111111111111111111",
        "target_sha": "2222222222222222222222222222222222222222",
        "commits": ["2222222 Add billing"],
        "recipe_hash": "abc",
        "steps": [
            {"index": 0, "command": "git merge --ff-only 2222222222222222222222222222222222222222", "status": "Ok", "exit_code": 0, "output": []},
            {"index": 1, "command": "php artisan migrate --force", "status": "Ok", "exit_code": 0, "output": ["Migrated"]},
            {"index": 2, "command": "php artisan optimize", "status": "Ok", "exit_code": 0, "output": []}
        ],
        "maintenance_down_done": true,
        "maintenance_up_done": true,
        "maintenance_down_status": "Ok",
        "maintenance_down_exit_code": 0,
        "maintenance_up_status": "Ok",
        "maintenance_up_exit_code": 0,
        "app_left_down": false,
        "mutation_started": true,
        "outcome": "Succeeded",
        "server_head_at_end": null,
        "tree_dirty": null,
        "last_message": null,
        "watch": watch,
        "smoke": {"Passed": {"status": 200, "latency_ms": 80}}
    })
}

fn write(root: &Path, env: &str, name: &str, contents: &[u8]) -> PathBuf {
    let dir = root.join("app").join(env);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

fn write_receipt(root: &Path, value: &Value) -> PathBuf {
    let name = format!(
        "{}-{}.json",
        value["started_at_ms"],
        value["run_id"].as_str().unwrap()
    );
    write(
        root,
        value["target"]["env"].as_str().unwrap(),
        &name,
        &serde_json::to_vec(value).unwrap(),
    )
}

fn parse(value: Value) -> Receipt {
    serde_json::from_value(value).unwrap()
}

fn ids(listing: &Listing) -> Vec<String> {
    listing
        .rows
        .iter()
        .map(|row| match row {
            Listed::Receipt { id, .. }
            | Listed::Newer { id, .. }
            | Listed::Unreadable { id, .. } => id.clone(),
        })
        .collect()
}

#[test]
fn list_is_newest_first_filters_by_env_and_limits() {
    let root = TempRoot::new();
    write_receipt(&root.0, &receipt_json("aaaa0001-0", 1000, "staging"));
    write_receipt(&root.0, &receipt_json("bbbb0002-0", 3000, "production"));
    write_receipt(&root.0, &receipt_json("cccc0003-0", 2000, "staging"));

    let all = list(&root.0, "app", None, None).unwrap();
    assert_eq!(ids(&all), ["bbbb0002", "cccc0003", "aaaa0001"]);
    assert_eq!(all.older, 0);

    let staging = list(&root.0, "app", Some("staging"), Some(1)).unwrap();
    assert_eq!(ids(&staging), ["cccc0003"]);
    assert_eq!(staging.older, 1);

    assert!(list(&root.0, "other", None, None).unwrap().rows.is_empty());
}

#[cfg(unix)]
#[test]
fn entries_in_keeps_readable_envs_when_one_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempRoot::new();
    let project_dir = root.0.join(safe_component("my app"));
    for (env, name) in [
        ("staging", "1000-aaaa0001-0.json"),
        ("production", "2000-bbbb0002-0.json"),
    ] {
        let dir = project_dir.join(env);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), b"{}").unwrap();
    }
    assert_eq!(list(&root.0, "my app", None, None).unwrap().rows.len(), 2);

    let locked = project_dir.join("production");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let (entries, problems) = history::entries_in(&project_dir);
    let listed = list(&root.0, "my app", None, None);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

    let found: Vec<_> = entries
        .iter()
        .map(|entry| (entry.env.as_str(), entry.run_id.as_str()))
        .collect();
    assert_eq!(found, [("staging", "aaaa0001-0")]);
    assert_eq!(problems.len(), 1);
    assert!(matches!(&problems[0], ReceiptError::Io { path, .. } if *path == locked));
    // `list` and `find` still fail on any unreadable directory.
    assert!(listed.is_err());

    let (entries, problems) = history::entries_in(&root.0.join("missing"));
    assert!(entries.is_empty() && problems.is_empty());
}

#[test]
fn list_shows_unreadable_and_newer_receipts_and_skips_other_files() {
    let root = TempRoot::new();
    let good = write_receipt(&root.0, &receipt_json("aaaa0001-0", 1000, "staging"));
    write(&root.0, "staging", "2000-bbbb0002-0.json", b"{ not json");
    let mut newer = receipt_json("cccc0003-0", 3000, "staging");
    newer["version"] = json!(2);
    newer["status"] = json!("SomethingNew");
    write_receipt(&root.0, &newer);
    // The claim lock and unrelated files are never read as receipts.
    fs::write(good.with_extension("json.lock"), b"").unwrap();
    write(&root.0, "staging", "notes.txt", b"hello");

    let listing = list(&root.0, "app", None, None).unwrap();
    assert_eq!(listing.rows.len(), 3);
    assert!(matches!(
        &listing.rows[0],
        Listed::Newer {
            version: 2,
            started_at_ms: Some(3000),
            ..
        }
    ));
    assert!(matches!(
        &listing.rows[1],
        Listed::Unreadable {
            started_at_ms: Some(2000),
            ..
        }
    ));
    assert!(matches!(&listing.rows[2], Listed::Receipt { .. }));
}

#[test]
fn find_matches_unique_prefixes_only() {
    let root = TempRoot::new();
    let first = write_receipt(
        &root.0,
        &receipt_json("18dab54b0c76-aaaa-0", 1000, "staging"),
    );
    write_receipt(
        &root.0,
        &receipt_json("18dab54b0d00-bbbb-0", 2000, "production"),
    );
    write_receipt(&root.0, &receipt_json("ffff0000-cccc-0", 3000, "staging"));

    // Display IDs grow past 8 characters until they are unique.
    let listing = list(&root.0, "app", None, None).unwrap();
    assert_eq!(ids(&listing), ["ffff0000", "18dab54b0d", "18dab54b0c"]);

    assert!(
        matches!(find(&root.0, "app", "18DAB54B0C").unwrap(), Found::One(path) if path == first)
    );
    match find(&root.0, "app", "18dab54b").unwrap() {
        Found::Ambiguous(ids) => assert_eq!(ids, ["18dab54b0d", "18dab54b0c"]),
        _ => panic!("expected an ambiguous match"),
    }
    assert!(matches!(
        find(&root.0, "app", "9999").unwrap(),
        Found::Missing
    ));
    assert!(matches!(find(&root.0, "app", "").unwrap(), Found::Missing));
}

#[test]
fn read_reports_newer_versions_and_loads_receipts_without_actor() {
    let root = TempRoot::new();
    let mut newer = receipt_json("aaaa0001-0", 1000, "staging");
    newer["version"] = json!(3);
    newer["steps"] = json!("changed shape");
    let path = write_receipt(&root.0, &newer);
    assert!(matches!(read(&path), Err(ReceiptError::Version(3))));

    let old = write_receipt(&root.0, &receipt_json("bbbb0002-0", 2000, "staging"));
    let receipt = read(&old).unwrap();
    assert_eq!(receipt.actor, None);
    assert_eq!(receipt.started_by(), None);
}

#[test]
fn actor_is_best_effort() {
    let actor = Actor::local(Path::new("/nonexistent/shipslip/checkout"));
    assert_eq!(actor.git_name, None);
    assert_eq!(actor.git_email, None);

    let mut receipt = parse(receipt_json("aaaa0001-0", 1000, "staging"));
    receipt.actor = Some(Actor {
        user: Some("ana".into()),
        git_name: Some("Ana".into()),
        git_email: None,
    });
    assert_eq!(
        receipt.started_by().as_deref(),
        Some("Ana (local user ana)")
    );
    assert_eq!(receipt.started_by_name().as_deref(), Some("Ana"));

    receipt.actor = Some(Actor {
        user: Some("ana".into()),
        git_name: None,
        git_email: Some("ana@example.test".into()),
    });
    assert_eq!(receipt.started_by_name().as_deref(), Some("ana"));
}

#[test]
fn markdown_shows_the_deployer_email_only_with_details() {
    let mut receipt = parse(receipt_json("aaaa0001-0", 1000, "staging"));
    receipt.actor = Some(Actor {
        user: Some("ana".into()),
        git_name: Some("Ana Lee".into()),
        git_email: Some("ana@example.test".into()),
    });
    let shared = markdown(&receipt, false);
    assert!(shared.contains("| Started by | Ana Lee |"), "{shared}");
    assert!(!shared.contains("ana@example.test"));
    assert!(markdown(&receipt, true).contains("ana@example.test"));
}

#[test]
fn next_steps_text_for_every_outcome() {
    let with = |outcome: Value| {
        let mut value = receipt_json("aaaa0001-0", 1000, "staging");
        value["outcome"] = outcome;
        parse(value)
    };
    let server_fix = |step: usize, env: &str| {
        format!("If the cause was on the server (permissions, .env, database), fix it there, then `slip from-step {env} {step}` runs the remaining steps on the deployed commit.")
    };
    let code_fix =
        |env: &str| format!("If it needs a code change, push the fix and run `slip deploy {env}`.");
    let cases: Vec<(Value, Vec<String>)> = vec![
        (json!("Succeeded"), vec![]),
        (json!("CancelledBeforeChanges"), vec![]),
        (
            json!({"FailedAtStep": {"step": 0, "partial_update": true}}),
            vec!["The fast-forward failed. Check the server checkout before deploying again.".into()],
        ),
        (
            json!({"FailedAtStep": {"step": 2, "partial_update": false}}),
            vec![
                "Step 2 failed and later steps did not run.".into(),
                server_fix(2, "staging"),
                code_fix("staging"),
            ],
        ),
        (
            json!({"StoppedAfterStep": {"step": 1, "reason": "Requested"}}),
            vec!["Steps after 1 did not run. `slip from-step staging 2` runs them on the deployed commit.".into()],
        ),
        // Stopping after the last step leaves nothing to run.
        (json!({"StoppedAfterStep": {"step": 2, "reason": "LockLost"}}), vec![]),
        (
            json!({"StoppedInMaintenance": "LockLost"}),
            vec!["No deploy steps ran, so the code on the server did not change.".into()],
        ),
        (
            json!({"Unknown": {"step": 1, "reason": "connection lost"}}),
            vec!["The result of step 1 is unknown. Check the server before running anything again.".into()],
        ),
    ];
    for (outcome, expected) in cases {
        assert_eq!(with(outcome.clone()).next_steps(), expected, "{outcome}");
    }

    let mut unfinished = receipt_json("aaaa0001-0", 1000, "staging");
    unfinished["status"] = json!("InProgress");
    unfinished["app_left_down"] = json!(true);
    assert_eq!(
        parse(unfinished).next_steps(),
        [
            "The run did not finish. If no deploy is running, resume it from /repo with `slip attach staging`.",
            "The app may still be in maintenance mode. After checking the server, `slip up staging` turns it off.",
        ]
    );

    let mut forged = receipt_json("aaaa0001-0", 1000, "stag\ning");
    forged["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
    forged["app_left_down"] = json!(true);
    assert_eq!(
        parse(forged).next_steps(),
        [
            "Step 1 failed and later steps did not run.".to_string(),
            server_fix(1, "stag\\x0aing"),
            code_fix("stag\\x0aing"),
            "The app may still be in maintenance mode. After checking the server, `slip up stag\\x0aing` turns it off.".into(),
        ]
    );
}

#[test]
fn badge_flags_every_incomplete_check() {
    let base = || parse(receipt_json("aaaa0001-0", 1000, "staging"));
    let watch = |status: WatchStatus| {
        let mut receipt = base();
        receipt.watch.as_mut().unwrap().status = status;
        receipt.badge().flags
    };
    assert_eq!(base().badge().flags, []);
    assert_eq!(base().badge().headline(), "Succeeded");
    for status in [
        WatchStatus::Partial,
        WatchStatus::Unavailable,
        WatchStatus::NoLogSeen,
        WatchStatus::Cancelled,
    ] {
        assert_eq!(watch(status), [Flag::LogNotFullyObserved], "{status:?}");
    }
    assert_eq!(watch(WatchStatus::NotRun), []);

    let mut receipt = base();
    receipt.watch = None;
    receipt.smoke = None;
    assert_eq!(
        receipt.badge().flags,
        [Flag::LogNotRecorded, Flag::SmokeNotRecorded]
    );

    let mut receipt = base();
    let mut value = receipt_json("aaaa0001-0", 1000, "staging");
    value["watch"]["new_errors"] = json!([{
        "exception": "RuntimeException", "file": "app/Foo.php", "count": 2,
        "variants": [], "overflow_variants": 0
    }]);
    value["watch"]["overflow_groups"] = json!(2);
    receipt.watch = parse(value).watch;
    receipt.smoke = Some(SmokeResult::Failed {
        status: Some(500),
        reason: "HTTP 500".into(),
    });
    receipt.app_left_down = true;
    let flags = receipt.badge().flags;
    assert_eq!(
        flags,
        [
            Flag::NewErrors(3),
            Flag::SmokeFailed(Some(500)),
            Flag::AppLeftDown
        ]
    );
    assert_eq!(
        flags.iter().map(Flag::text).collect::<Vec<_>>(),
        [
            "⚠ 3 new error groups",
            "⚠ smoke 500",
            "⛔ app in maintenance mode"
        ]
    );

    // An unfinished receipt never reads as its saved outcome.
    let mut receipt = base();
    receipt.status = ReceiptStatus::InProgress;
    let badge = receipt.badge();
    assert!(badge.unfinished);
    assert_eq!(badge.headline(), "Unfinished (last recorded: Succeeded)");

    // Checks never apply to a run that changed nothing.
    let mut receipt = base();
    receipt.outcome = Some(DeployOutcome::AbortedBeforeChanges(AbortReason::LockLost));
    receipt.watch = None;
    receipt.smoke = None;
    assert_eq!(receipt.badge().flags, []);
    assert_eq!(receipt.badge().headline(), "Aborted before changes");
}

fn secret_receipt() -> Receipt {
    let mut value = receipt_json("aaaa0001-0", 1000, "staging");
    value["steps"][1]["output"] = json!(["token=SECRET-OUTPUT"]);
    value["watch"]["new_errors"] = json!([{
        "exception": "PDOException", "file": "app/Db.php", "count": 1,
        "variants": [{
            "signature": "s", "level": "ERROR", "message": "password SECRET-LOG",
            "display_file_line": "app/Db.php:9", "count": 1, "phase": "After"
        }],
        "overflow_variants": 0
    }]);
    value["watch"]["warnings"] = json!(["SECRET-WATCH-WARNING"]);
    value["warnings"] = json!(["SECRET-WARNING"]);
    value["last_message"] = json!("SECRET-MESSAGE");
    value["outcome"] = json!({"Unknown": {"step": 1, "reason": "SECRET-REASON"}});
    value["smoke"] = json!({"Failed": {"status": null, "reason": "SECRET-SMOKE"}});
    parse(value)
}

#[test]
fn markdown_keeps_server_text_out_unless_details_are_requested() {
    let receipt = secret_receipt();
    let shared = markdown(&receipt, false);
    assert!(!shared.contains("SECRET"), "{shared}");
    assert!(shared.contains("| Outcome | Unknown at step 1 |"));
    assert!(shared.contains("`PDOException`"));
    assert!(shared.contains("| Server HEAD at end | not recorded |"));

    let detailed = markdown(&receipt, true);
    for secret in [
        "SECRET-OUTPUT",
        "SECRET-LOG",
        "SECRET-WATCH-WARNING",
        "SECRET-WARNING",
        "SECRET-MESSAGE",
        "SECRET-REASON",
        "SECRET-SMOKE",
    ] {
        assert!(detailed.contains(secret), "{secret} missing:\n{detailed}");
    }
    assert!(detailed.contains("Review it for secrets before sharing"));
}

#[test]
fn markdown_lists_maintenance_and_escapes_untrusted_text() {
    let mut value = receipt_json("aaaa0001-0", 1000, "staging");
    value["commits"] = json!(["2222222 [click](https://evil.test) <b>x</b> | y"]);
    value["steps"][1]["command"] = json!("echo ``` | tee");
    value["steps"][1]["output"] = json!(["````", "\u{1b}]0;title\u{7}"]);
    value["maintenance_up_status"] = json!("Failed");
    value["maintenance_up_exit_code"] = json!(1);
    value["app_left_down"] = json!(true);
    let text = markdown(&parse(value), true);

    assert!(text.contains("| Maintenance on | `php artisan down` | ✓ ok | 0 |"));
    assert!(text.contains("| Maintenance off | `php artisan up` | ✗ failed | 1 |"));
    assert!(text.contains("- ⛔ app in maintenance mode"));
    assert!(text.contains("- 2222222 \\[click\\]\\(https://evil.test\\) \\<b\\>x\\</b\\> \\| y"));
    assert!(text.contains("| 1 | ````echo ``` \\| tee```` |"), "{text}");
    // Output fences outlast any backtick run, and control characters are escaped.
    assert!(
        text.contains("`````text\n````\n\\x1b]0;title\\x07\n`````"),
        "{text}"
    );
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn time_and_color_helpers() {
    assert_eq!(format_utc(0), "1970-01-01 00:00:00Z");
    assert_eq!(format_utc(1_790_943_136_408), "2026-10-02 12:12:16Z");
    assert_eq!(format_utc(951_782_400_000), "2000-02-29 00:00:00Z");
    assert_eq!(format_duration(850), "850 ms");
    assert_eq!(format_duration(164_000), "2m 44s");
    assert_eq!(format_duration(3_780_000), "1h 3m");
    assert_eq!(
        strip_color("\u{1b}[37;44m INFO \u{1b}[39;49m done \u{1b}[2J"),
        " INFO  done \u{1b}[2J"
    );
}
