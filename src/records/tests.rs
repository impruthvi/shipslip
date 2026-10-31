use serde_json::{json, Value};

use super::*;
use crate::receipt::tests::{receipt_json, up_json, write_receipt, TempRoot};

fn deploy() -> Value {
    receipt_json("aaaa0001-0", 1000, "staging")
}

fn query(value: Value) -> ReceiptRecord {
    let root = TempRoot::new();
    let mut located = value.clone();
    located["target"]["env"] = json!("staging");
    let path = write_receipt(&root.0, &located);
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    receipt(&root.0, "app", value["run_id"].as_str().unwrap()).unwrap()
}

fn with_outcome(outcome: Value) -> ReceiptRecord {
    let mut value = deploy();
    value["outcome"] = outcome;
    query(value)
}

fn to_json(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

fn row(kind: Value, command: &str, status: &str, exit_code: Value) -> Value {
    let mut row = json!({
        "command": command, "status": status, "exit_code": exit_code,
        "started_at_ms": null, "finished_at_ms": null, "duration_ms": null,
        "output": []
    });
    for (key, value) in kind.as_object().unwrap() {
        row[key] = value.clone();
    }
    row
}

fn text(text: &str) -> Value {
    json!({"text": text})
}

fn command(args: &[&str]) -> Value {
    json!({"command": args})
}

#[test]
fn untrusted_text_serializes_raw_and_escapes_only_when_asked() {
    let hostile = Untrusted::new("a\u{1b}]0;x\u{7}`<b>\nc");
    assert_eq!(to_json(&hostile), json!("a\u{1b}]0;x\u{7}`<b>\nc"));
    assert_eq!(hostile.raw(), "a\u{1b}]0;x\u{7}`<b>\nc");
    assert_eq!(hostile.escaped(), "a\\x1b]0;x\\x07`<b>\nc");
    assert_eq!(hostile.field(), "a\\x1b]0;x\\x07`<b>\\x0ac");
}

#[test]
fn json_carries_the_schema_number() {
    #[derive(Serialize)]
    struct Record {
        id: &'static str,
    }
    #[derive(Serialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum Error {
        Ambiguous { ids: Vec<&'static str> },
    }
    assert_eq!(
        to_json(versioned(Record { id: "aaaa0001" })),
        json!({"schema": 1, "id": "aaaa0001"})
    );
    assert_eq!(
        to_json(versioned_error(Error::Ambiguous {
            ids: vec!["aaaa0001", "aaaa0002"]
        })),
        json!({"schema": 1, "error": {"kind": "ambiguous", "ids": ["aaaa0001", "aaaa0002"]}})
    );
}

#[test]
fn deploy_rows_wrap_the_steps_in_maintenance_down_and_up() {
    let receipt = query(deploy());
    let mut migrate = row(
        json!({"kind": "step", "index": 1}),
        "php artisan migrate --force",
        "ok",
        json!(0),
    );
    migrate["output"] = json!(["Migrated"]);
    assert_eq!(
        to_json(receipt.steps),
        json!([
            row(
                json!({"kind": "maintenance_down"}),
                "php artisan down",
                "ok",
                json!(0)
            ),
            row(
                json!({"kind": "step", "index": 0}),
                "git merge --ff-only 2222222222222222222222222222222222222222",
                "ok",
                json!(0)
            ),
            migrate,
            row(
                json!({"kind": "step", "index": 2}),
                "php artisan optimize",
                "ok",
                json!(0)
            ),
            row(
                json!({"kind": "maintenance_up"}),
                "php artisan up",
                "ok",
                json!(0)
            ),
        ])
    );
}

#[test]
fn rerun_without_maintenance_has_only_its_steps() {
    let mut value = deploy();
    value["run_plan"] = json!({"FromStep": 2});
    value["target"]["maintenance"] = json!(false);
    for field in ["maintenance_down_status", "maintenance_up_status"] {
        value[field] = Value::Null;
    }
    value["steps"][0]["status"] = json!("Skipped");
    value["steps"][1]["status"] = json!("Skipped");
    let rows = to_json(query(value).steps);
    let kinds: Vec<_> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["kind"].clone(),
                row["index"].clone(),
                row["status"].clone(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            (json!("step"), json!(0), json!("skipped")),
            (json!("step"), json!(1), json!("skipped")),
            (json!("step"), json!(2), json!("ok")),
        ]
    );
}

#[test]
fn rerun_with_maintenance_and_deploy_without_it() {
    let kinds = |value: Value| -> Vec<Value> {
        to_json(query(value).steps)
            .as_array()
            .unwrap()
            .iter()
            .map(|row| json!([row["kind"], row["index"]]))
            .collect()
    };
    let mut rerun = deploy();
    rerun["run_plan"] = json!("Rerun");
    rerun["steps"][0]["status"] = json!("Skipped");
    assert_eq!(
        kinds(rerun),
        [
            json!(["maintenance_down", null]),
            json!(["step", 0]),
            json!(["step", 1]),
            json!(["step", 2]),
            json!(["maintenance_up", null]),
        ]
    );

    let mut plain = deploy();
    for field in ["maintenance_down_status", "maintenance_up_status"] {
        plain[field] = Value::Null;
    }
    assert_eq!(
        kinds(plain),
        [json!(["step", 0]), json!(["step", 1]), json!(["step", 2])]
    );
}

#[test]
fn up_runs_have_no_maintenance_up_row() {
    let mut value = up_json("bbbb0002-0", 2000);
    value["steps"] = json!([{
        "index": 0, "command": "php artisan up", "status": "Failed",
        "exit_code": 7, "output": ["up failed"]
    }]);
    value["maintenance_up_status"] = json!("Failed");
    value["maintenance_up_exit_code"] = json!(7);
    let mut expected = row(
        json!({"kind": "step", "index": 0}),
        "php artisan up",
        "failed",
        json!(7),
    );
    expected["output"] = json!(["up failed"]);
    assert_eq!(to_json(query(value).steps), json!([expected]));
}

#[test]
fn step_times_give_a_duration_only_when_both_ends_were_seen() {
    let mut value = deploy();
    value["maintenance_down_started_at_ms"] = json!(500);
    value["maintenance_down_finished_at_ms"] = json!(900);
    value["steps"][1]["status"] = json!("Unknown");
    value["steps"][1]["exit_code"] = Value::Null;
    value["steps"][1]["started_at_ms"] = json!(2_000);
    value["steps"][2]["status"] = json!("NotStarted");
    value["steps"][2]["exit_code"] = Value::Null;
    value["maintenance_up_status"] = json!("Unknown");
    value["maintenance_up_exit_code"] = Value::Null;
    value["maintenance_up_started_at_ms"] = json!(9_000);
    let rows = to_json(query(value).steps);
    let times: Vec<_> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            json!([
                row["status"],
                row["started_at_ms"],
                row["finished_at_ms"],
                row["duration_ms"]
            ])
        })
        .collect();
    assert_eq!(
        times,
        [
            json!(["ok", 500, 900, 400]),
            json!(["ok", null, null, null]),
            json!(["unknown", 2_000, null, null]),
            json!(["not_started", null, null, null]),
            json!(["unknown", 9_000, null, null]),
        ]
    );
}

#[test]
fn hostile_step_text_reaches_the_rows_raw() {
    let mut value = deploy();
    value["steps"][1]["command"] = json!("php artisan `id` <b>\u{1b}[2J");
    value["steps"][1]["output"] = json!(["\u{1b}[31mred\u{1b}[0m \u{202e}x"]);
    let rows = to_json(query(value).steps);
    assert_eq!(rows[2]["command"], "php artisan `id` <b>\u{1b}[2J");
    assert_eq!(
        rows[2]["output"],
        json!(["\u{1b}[31mred\u{1b}[0m \u{202e}x"])
    );
}

#[test]
fn outcome_kinds_and_the_unfinished_marker() {
    let cases = [
        (json!("Succeeded"), json!({"kind": "succeeded"})),
        (
            json!({"FailedAtStep": {"step": 2, "partial_update": true}}),
            json!({"kind": "failed_at_step", "step": 2, "partial_update": true}),
        ),
        (
            json!({"StoppedAfterStep": {"step": 1, "reason": "LockLost"}}),
            json!({"kind": "stopped_after_step", "step": 1}),
        ),
        (
            json!({"StoppedInMaintenance": "LockLost"}),
            json!({"kind": "stopped_in_maintenance"}),
        ),
        (
            json!({"Unknown": {"step": 1, "reason": "connection lost"}}),
            json!({"kind": "unknown", "step": 1}),
        ),
        (
            json!("CancelledBeforeChanges"),
            json!({"kind": "cancelled_before_changes"}),
        ),
        (
            json!({"AbortedBeforeChanges": "LockLost"}),
            json!({"kind": "aborted_before_changes"}),
        ),
        (Value::Null, json!({"kind": "not_recorded"})),
    ];
    for (outcome, expected) in cases {
        let mut finished = expected.clone();
        finished["unfinished"] = json!(false);
        assert_eq!(
            to_json(with_outcome(outcome.clone()))["outcome"],
            finished,
            "{outcome}"
        );

        let mut value = deploy();
        value["outcome"] = outcome.clone();
        value["status"] = json!("InProgress");
        let mut unfinished = expected;
        unfinished["unfinished"] = json!(true);
        assert_eq!(to_json(query(value))["outcome"], unfinished, "{outcome}");
    }
}

#[test]
fn flags_carry_their_numbers() {
    let mut value = deploy();
    value["watch"]["status"] = json!("Partial");
    value["watch"]["new_errors"] = json!([{
        "exception": "RuntimeException", "file": "app/Foo.php", "count": 2,
        "variants": [], "overflow_variants": 0
    }]);
    value["watch"]["overflow_groups"] = json!(2);
    value["smoke"] = json!({"Failed": {"status": 500, "reason": "HTTP 500"}});
    value["app_left_down"] = json!(true);
    assert_eq!(
        to_json(query(value))["flags"],
        json!([
            {"kind": "new_errors", "groups": 3},
            {"kind": "log_not_fully_observed"},
            {"kind": "smoke_failed", "status": 500},
            {"kind": "app_left_down"},
        ])
    );

    let mut value = deploy();
    value["watch"] = Value::Null;
    value["smoke"] = json!({"Failed": {"status": null, "reason": "timed out"}});
    assert_eq!(
        to_json(query(value))["flags"],
        json!([{"kind": "log_not_recorded"}, {"kind": "smoke_failed", "status": null}])
    );

    let mut value = deploy();
    value["smoke"] = Value::Null;
    assert_eq!(
        to_json(query(value))["flags"],
        json!([{"kind": "smoke_not_recorded"}])
    );
}

#[test]
fn next_steps_for_each_outcome_that_has_them() {
    let server_fix = |step: &str| {
        json!([
            text(
                "If the cause was on the server (permissions, .env, database), fix it there, then "
            ),
            command(&["slip", "from-step", "staging", step]),
            text(" runs the remaining steps on the deployed commit."),
        ])
    };
    let code_fix = json!([
        text("If it needs a code change, push the fix and run "),
        command(&["slip", "deploy", "staging"]),
        text("."),
    ]);
    let cases = [
        (json!("Succeeded"), json!([])),
        (json!("CancelledBeforeChanges"), json!([])),
        (
            json!({"FailedAtStep": {"step": 0, "partial_update": true}}),
            json!([[text(
                "The fast-forward failed. Check the server checkout before deploying again."
            )]]),
        ),
        (
            json!({"FailedAtStep": {"step": 2, "partial_update": false}}),
            json!([
                [text("Step 2 failed and later steps did not run.")],
                server_fix("2"),
                code_fix,
            ]),
        ),
        (
            json!({"StoppedAfterStep": {"step": 1, "reason": "Requested"}}),
            json!([[
                text("Steps after 1 did not run. "),
                command(&["slip", "from-step", "staging", "2"]),
                text(" runs them on the deployed commit."),
            ]]),
        ),
        (
            json!({"StoppedAfterStep": {"step": 2, "reason": "LockLost"}}),
            json!([]),
        ),
        (
            json!({"StoppedInMaintenance": "LockLost"}),
            json!([[text(
                "No deploy steps ran, so the code on the server did not change."
            )]]),
        ),
        (
            json!({"Unknown": {"step": 1, "reason": "connection lost"}}),
            json!([[text(
                "The result of step 1 is unknown. Check the server before running anything again."
            )]]),
        ),
    ];
    for (outcome, steps) in cases {
        assert_eq!(
            to_json(with_outcome(outcome.clone()).next_steps),
            json!({"copy_safe": true, "steps": steps}),
            "{outcome}"
        );
    }
}

#[test]
fn unfinished_runs_new_errors_and_maintenance_have_next_steps() {
    let mut value = deploy();
    value["status"] = json!("InProgress");
    value["app_left_down"] = json!(true);
    value["watch"]["new_errors"] = json!([{
        "exception": "RuntimeException", "file": null, "count": 1,
        "variants": [], "overflow_variants": 0
    }]);
    assert_eq!(
        to_json(query(value).next_steps),
        json!({"copy_safe": true, "steps": [
            [
                text("The run did not finish. If no deploy is running, resume it from /repo with "),
                command(&["slip", "attach", "staging"]),
                text("."),
            ],
            [
                text("The log showed 1 new error group after this run. "),
                command(&["slip", "logs", "staging"]),
                text(" shows the log."),
            ],
            [
                text("The app may still be in maintenance mode. After checking the server, "),
                command(&["slip", "up", "staging"]),
                text(" turns it off."),
            ],
        ]})
    );
}

#[test]
fn up_next_steps_point_at_attach_or_another_up() {
    let mut running = up_json("aaaa0001-0", 1000);
    running["status"] = json!("InProgress");
    running["outcome"] = Value::Null;
    running["app_left_down"] = json!(true);
    assert_eq!(
        to_json(query(running).next_steps)["steps"],
        json!([[
            text("The run did not finish. If no deploy is running, resume it from /repo with "),
            command(&["slip", "attach", "staging"]),
            text("."),
        ]])
    );

    let mut failed = up_json("bbbb0002-0", 2000);
    failed["outcome"] = json!({"FailedAtStep": {"step": 0, "partial_update": false}});
    failed["app_left_down"] = json!(true);
    assert_eq!(
        to_json(query(failed).next_steps)["steps"],
        json!([[
            text("The app may still be in maintenance mode. After checking the server, "),
            command(&["slip", "up", "staging"]),
            text(" tries again."),
        ]])
    );

    let mut unknown = up_json("cccc0003-0", 3000);
    unknown["outcome"] = json!({"Unknown": {"step": 0, "reason": "connection lost"}});
    assert_eq!(
        to_json(query(unknown).next_steps)["steps"],
        json!([[text(
            "Check the server before running anything again; the deploy lock was kept."
        )]])
    );
}

#[test]
fn copy_is_unsafe_for_an_odd_env_name_or_checkout_path() {
    let failed = |env: &str, repo_root: &str| {
        let mut value = receipt_json("aaaa0001-0", 1000, env);
        value["repo_root"] = json!(repo_root);
        value["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
        query(value).next_steps
    };
    assert!(failed("prod.eu_1-a", "/work/it's my app").copy_safe);
    for env in [
        "prod;rm",
        "prod$(id)",
        "pro duction",
        "prod`x`",
        "stag\ning",
        "",
    ] {
        assert!(!failed(env, "/repo").copy_safe, "{env:?}");
    }
    for repo_root in [
        "/work/`slip deploy production`",
        "/work/a\u{1b}]0;x\u{7}b",
        "/work/a\nb",
    ] {
        assert!(!failed("staging", repo_root).copy_safe, "{repo_root:?}");
    }
}

#[test]
fn hostile_env_and_checkout_reach_next_steps_raw() {
    let mut value = receipt_json("aaaa0001-0", 1000, "stag\u{1b}[2J`x`");
    value["status"] = json!("InProgress");
    value["repo_root"] = json!("/work/<b>\u{202e}");
    let steps = to_json(query(value).next_steps);
    assert_eq!(steps["copy_safe"], false);
    assert_eq!(
        steps["steps"][0],
        json!([
            text("The run did not finish. If no deploy is running, resume it from /work/<b>\u{202e} with "),
            command(&["slip", "attach", "stag\u{1b}[2J`x`"]),
            text("."),
        ])
    );
}

#[test]
fn summary_and_started_by_keep_server_text_raw() {
    let mut value = deploy();
    value["outcome"] = json!({"Unknown": {"step": 1, "reason": "lost \u{1b}[2J"}});
    value["actor"] =
        json!({"user": "ana", "git_name": "Ana\u{7}", "git_email": "ana@example.test"});
    let receipt = query(value);
    assert_eq!(
        receipt.summary.unwrap().raw(),
        "Outcome of step 1 is unknown: lost \u{1b}[2J"
    );
    assert_eq!(
        receipt.started_by.unwrap().raw(),
        "Ana\u{7} <ana@example.test> (local user ana)"
    );
    assert_eq!(receipt.started_by_name.unwrap().raw(), "Ana\u{7}");
}

#[test]
fn receipt_query_reads_a_saved_deploy_as_a_versioned_record() {
    let root = TempRoot::new();
    let path = write_receipt(&root.0, &deploy());
    let record = receipt(&root.0, "app", "AAAA").unwrap();
    let record = to_json(versioned(record));
    let mut migrate = row(
        json!({"kind": "step", "index": 1}),
        "php artisan migrate --force",
        "ok",
        json!(0),
    );
    migrate["output"] = json!(["Migrated"]);
    assert_eq!(
        record,
        json!({
            "schema": 1,
            "path": path.to_string_lossy(), "run_id": "aaaa0001-0", "kind": "deploy",
            "project": "app", "repo_root": "/repo",
            "target": {"env": "staging", "production": false, "ssh_alias": "app-staging",
                "path": "/var/www/app", "branch": "main", "log_path": "storage/logs/laravel.log"},
            "run_plan": "Deploy", "started_at_ms": 1000, "finished_at_ms": 165000,
            "duration_ms": 164000, "started_by": null, "started_by_name": null,
            "from_sha": "1111111111111111111111111111111111111111",
            "target_sha": "2222222222222222222222222222222222222222",
            "server_head_at_end": null, "tree_dirty": null,
            "commits": ["2222222 Add billing"], "more_commits": 0, "recipe_hash": "abc",
            "confirmed": true, "mutation_started": true,
            "outcome": {"kind": "succeeded", "unfinished": false},
            "summary": "Deploy run succeeded.", "flags": [],
            "steps": [
                row(json!({"kind": "maintenance_down"}), "php artisan down", "ok", json!(0)),
                row(json!({"kind": "step", "index": 0}), "git merge --ff-only 2222222222222222222222222222222222222222", "ok", json!(0)),
                migrate,
                row(json!({"kind": "step", "index": 2}), "php artisan optimize", "ok", json!(0)),
                row(json!({"kind": "maintenance_up"}), "php artisan up", "ok", json!(0))
            ],
            "watch": {"status": "Complete", "log_path": "storage/logs/laravel.log", "duration_ms": 120000,
                "baseline_signatures": 3, "history_signatures": 2, "observed_lines": 10, "parsed_lines": 10,
                "dropped_view_lines": 0, "truncated_entries": 0, "new_errors": [], "overflow_groups": 0,
                "overflow_signatures": 0, "warnings": []},
            "smoke": {"kind": "passed", "status": 200, "latency_ms": 80},
            "notes": [], "next_steps": {"copy_safe": true, "steps": []}
        })
    );
}

#[test]
fn receipt_query_reports_missing_ambiguous_newer_and_unreadable() {
    let root = TempRoot::new();
    assert_eq!(
        to_json(receipt(&root.0, "app", "missing").unwrap_err()),
        json!({"kind": "missing"})
    );
    assert_eq!(
        to_json(receipt(&root.0, "app", "").unwrap_err()),
        json!({"kind": "missing"})
    );
    write_receipt(&root.0, &receipt_json("aaaa0001-0", 1000, "staging"));
    write_receipt(&root.0, &receipt_json("aaaa0002-0", 2000, "production"));
    assert_eq!(
        to_json(receipt(&root.0, "app", "aaaa").unwrap_err()),
        json!({"kind": "ambiguous", "ids": ["aaaa0002", "aaaa0001"]})
    );
    let mut newer = deploy();
    newer["version"] = json!(99);
    newer["status"] = json!("FutureStatus");
    write_receipt(&root.0, &newer);
    assert_eq!(
        to_json(receipt(&root.0, "app", "aaaa0001").unwrap_err()),
        json!({"kind": "newer_version", "version": 99})
    );
    let path = write_receipt(&root.0, &deploy());
    std::fs::write(&path, b"{ invalid json").unwrap();
    let error = receipt(&root.0, "app", "aaaa0001").unwrap_err();
    let expected = format!(
        "invalid receipt `{}`: key must be a string at line 1 column 3",
        path.display()
    );
    assert_eq!(
        to_json(error),
        json!({"kind": "unreadable", "message": expected})
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        to_json(receipt_at(&path).unwrap_err()),
        json!({"kind": "missing"})
    );
    // Directory traversal errors are unreadable, rather than guessed missing.
    std::fs::write(root.0.join("other"), "not a directory").unwrap();
    assert!(matches!(
        receipt(&root.0, "other", "aaaa"),
        Err(ReceiptQueryError::Unreadable { .. })
    ));
}

#[cfg(unix)]
#[test]
fn receipt_query_reports_a_file_without_read_permission_as_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    let root = TempRoot::new();
    let path = write_receipt(&root.0, &deploy());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = receipt(&root.0, "app", "aaaa0001");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    match result {
        Err(ReceiptQueryError::Unreadable { message }) => {
            assert!(message.raw().contains("Permission denied"), "{message:?}");
        }
        other => panic!("{other:?}"),
    }
}
