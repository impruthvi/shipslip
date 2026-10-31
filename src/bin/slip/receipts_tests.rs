use super::*;
use serde_json::json;
use std::path::PathBuf;

fn args(values: &[&str]) -> Result<Args, String> {
    Args::parse(
        &values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())
}

#[test]
fn parses_list_and_show_forms() {
    assert_eq!(
        args(&[]),
        Ok(Args::List {
            env: None,
            all: false
        })
    );
    assert_eq!(
        args(&["staging", "--all"]),
        Ok(Args::List {
            env: Some("staging".into()),
            all: true
        })
    );
    assert_eq!(
        args(&["show", "18dab54b", "--md", "--with-details"]),
        Ok(Args::Show {
            id: "18dab54b".into(),
            md: true,
            details: true
        })
    );
    assert_eq!(
        args(&["show", "--with-details", "18dab54b"]),
        Err("--with-details only applies with --md".into())
    );
    assert_eq!(
        args(&["show"]),
        Err("receipts show requires a receipt ID".into())
    );
    assert_eq!(
        args(&["show", "a", "b"]),
        Err("receipts show accepts one receipt ID".into())
    );
    assert_eq!(
        args(&["staging", "production"]),
        Err("receipts accepts at most one environment name".into())
    );
    assert_eq!(
        args(&["--json"]),
        Err("unknown receipts option --json".into())
    );
}

fn receipt(changes: impl FnOnce(&mut serde_json::Value)) -> Receipt {
    let mut value = json!({
        "version": 1, "project": "app", "repo_root": "/work/app", "run_id": "aaaa0001-0",
        "owner_pid": 1, "confirmed": true, "started_at_ms": 1000, "finished_at_ms": 5000,
        "status": "Final", "phase": "Finished",
        "target": {
            "env": "staging", "production": false, "ssh_alias": "app", "path": "/srv/app",
            "branch": "main", "steps": ["php artisan migrate --force", "php artisan optimize"],
            "maintenance": false
        },
        "run_plan": "Deploy", "from_sha": "1111111", "target_sha": "2222222", "commits": [],
        "recipe_hash": "x",
        "steps": [
            {"index": 1, "command": "php artisan migrate --force", "status": "Ok", "exit_code": 0, "output": []},
            {"index": 2, "command": "php artisan optimize", "status": "Ok", "exit_code": 0, "output": []}
        ],
        "maintenance_down_done": false, "maintenance_up_done": false,
        "maintenance_down_status": null, "maintenance_down_exit_code": null,
        "maintenance_up_status": null, "maintenance_up_exit_code": null,
        "app_left_down": false, "mutation_started": true, "outcome": "Succeeded",
        "server_head_at_end": null, "tree_dirty": null, "last_message": null
    });
    changes(&mut value);
    serde_json::from_value(value).unwrap()
}

#[test]
fn show_escapes_server_text_and_cannot_forge_fields() {
    let text = render_show(&receipt(|value| {
        value["steps"][0]["output"] = json!(["\u{1b}[32mok\u{1b}[0m \u{1b}]0;pwned\u{7}"]);
        value["warnings"] = json!(["disk full\nOutcome:            Deploy run succeeded."]);
    }));
    assert!(!text.contains('\u{1b}'), "{text}");
    assert!(!text.contains('\u{7}'));
    assert!(text.contains("        ok \\x1b]0;pwned\\x07"), "{text}");
    assert_eq!(text.matches("\nOutcome:").count(), 1, "{text}");
    assert!(text.contains("disk full\\x0aOutcome:"));
}

#[test]
fn show_gives_a_finished_step_duration_only() {
    let untimed = render_show(&receipt(|_| {}));
    assert!(
        untimed.contains("1  php artisan migrate --force (exit 0)\n"),
        "{untimed}"
    );

    let text = render_show(&receipt(|value| {
        value["steps"][0]["started_at_ms"] = json!(1500);
        value["steps"][0]["finished_at_ms"] = json!(3500);
    }));
    assert!(
        text.contains("1  php artisan migrate --force (exit 0) in 2s\n"),
        "{text}"
    );
}

#[test]
fn show_gives_a_start_without_an_end_as_end_not_known() {
    let started = 1_790_943_136_408u128;
    let text = render_show(&receipt(|value| {
        value["steps"][0]["status"] = json!("Unknown");
        value["steps"][0]["exit_code"] = json!(null);
        value["steps"][0]["started_at_ms"] = json!(started as u64);
        value["steps"][1]["started_at_ms"] = json!(started as u64 + 4000);
    }));
    let first = time_of_day(started);
    assert!(first.len() >= 8 && first.as_bytes()[2] == b':', "{first}");
    assert!(
        text.contains(&format!(
            "1  php artisan migrate --force, started {first}, end not known\n"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "2  php artisan optimize (exit 0), started {}, end not known\n",
            time_of_day(started + 4000)
        )),
        "{text}"
    );
}

#[test]
fn show_counts_commits_past_the_preview_list() {
    let text = render_show(&receipt(|value| {
        value["commits"] = serde_json::json!(["2222222 Add billing"]);
        value["more_commits"] = serde_json::json!(3000);
    }));
    assert!(
        text.contains("Commits:\n  2222222 Add billing\n  and 3000 more commits\n"),
        "{text}"
    );
}

#[test]
fn up_runs_are_labelled_in_the_list() {
    let listing = receipt::Listing {
        rows: vec![
            Listed::Receipt {
                id: "aaaa0002".into(),
                path: PathBuf::from("/r/b.json"),
                receipt: Box::new(receipt(|value| {
                    value["kind"] = json!("up");
                    value["from_sha"] = json!("2222222");
                })),
            },
            Listed::Receipt {
                id: "aaaa0001".into(),
                path: PathBuf::from("/r/a.json"),
                receipt: Box::new(receipt(|_| {})),
            },
        ],
        older: 0,
    };
    let text = render_list("app", None, &listing);
    let row = |id| text.lines().find(|line| line.starts_with(id)).unwrap();
    assert!(row("aaaa0002").contains("brought up at 2222222"), "{text}");
    assert!(!row("aaaa0002").contains('→'), "{text}");
    assert!(row("aaaa0001").contains("1111111 → 2222222"), "{text}");
}

#[test]
fn list_rows_stay_on_one_line() {
    let listing = receipt::Listing {
        rows: vec![
            Listed::Receipt {
                id: "aaaa0001".into(),
                path: PathBuf::from("/r/a.json"),
                receipt: Box::new(receipt(|value| {
                    value["target"]["env"] = json!("staging\nbbbb0002  forged");
                })),
            },
            Listed::Unreadable {
                id: "cccc0003".into(),
                env: "staging".into(),
                started_at_ms: None,
                path: PathBuf::from("/r/c.json"),
                reason: "expected value\nat line 1".into(),
            },
        ],
        older: 4,
    };
    let text = render_list("app", None, &listing);
    assert_eq!(
        text.lines().filter(|line| line.contains("forged")).count(),
        1
    );
    assert!(text.contains("staging\\x0abbbb0002  forged"));
    assert!(text.contains("unreadable: expected value\\x0aat line 1"));
    assert!(text.contains("4 older receipts; use --all to list them."));
    // The header names the zone the times are in.
    let header = text.lines().find(|line| line.starts_with("ID ")).unwrap();
    let zone = header
        .split("STARTED (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .unwrap();
    assert!(!zone.is_empty() && !zone.contains(' '), "{header}");
    assert_eq!(
        render_list(
            "app",
            Some("staging"),
            &receipt::Listing {
                rows: vec![],
                older: 0
            }
        ),
        "No receipts for app in staging yet.\n"
    );
}

#[test]
fn show_prints_next_steps_from_the_receipt() {
    let failed = receipt(|value| {
        value["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
        value["steps"][0]["status"] = json!("Failed");
    });
    let text = render_show(&failed);
    assert!(text.contains("Server HEAD at end: not recorded"));
    assert!(text.contains("If the cause was on the server"), "{text}");
    assert!(text.contains("`slip from-step staging 1`"));
    assert!(text.contains("If it needs a code change, push the fix and run `slip deploy staging`."));

    let unfinished = receipt(|value| {
        value["status"] = json!("InProgress");
        value["app_left_down"] = json!(true);
    });
    let text = render_show(&unfinished);
    assert!(text.contains("Outcome:            Unfinished (last recorded: Succeeded)"));
    assert!(text.contains(
        "At the time of this run:\n  The run did not finish. If no deploy is running, resume it from /work/app with `slip attach staging`.\n  The app may still be in maintenance mode."
    ), "{text}");

    let succeeded = render_show(&receipt(|_| {}));
    assert!(
        !succeeded.contains("At the time of this run:"),
        "{succeeded}"
    );
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "shipslip-cli-receipts-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }

    fn save(&self, started_at_ms: u64, receipt: &Receipt) {
        let dir = self.0.join("app").join("staging");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{started_at_ms}-{}.json", receipt.run_id));
        std::fs::write(path, serde_json::to_vec(receipt).unwrap()).unwrap();
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn up_to_date_hint_points_at_a_failed_last_run() {
    let root = TempRoot::new("failed");
    assert_eq!(up_to_date_hint(&root.0, "app", "staging"), None);

    let failed = receipt(|value| {
        value["run_id"] = json!("bbbb0002-0");
        value["outcome"] = json!({"FailedAtStep": {"step": 2, "partial_update": false}});
        value["app_left_down"] = json!(true);
    });
    root.save(2000, &failed);
    // A later run that stopped before changing anything does not hide it.
    let cancelled = receipt(|value| {
        value["run_id"] = json!("cccc0003-0");
        value["outcome"] = json!("CancelledBeforeChanges");
        value["mutation_started"] = json!(false);
    });
    root.save(3000, &cancelled);

    let hint = up_to_date_hint(&root.0, "app", "staging").unwrap();
    assert_eq!(
        hint.lines().collect::<Vec<_>>(),
        [
            "The last run here (bbbb0002) did not succeed: Failed at step 2. See `slip receipts show bbbb0002`.",
            "If the cause was on the server (permissions, .env, database), fix it there, then `slip from-step staging 2` runs the remaining steps on the deployed commit.",
            "If it needs a code change, push the fix and run `slip deploy staging`.",
            "The app may still be in maintenance mode; `slip up staging` turns it off.",
        ]
    );
}

#[test]
fn up_to_date_hint_is_silent_after_a_successful_run() {
    let root = TempRoot::new("ok");
    root.save(1000, &receipt(|_| {}));
    assert_eq!(up_to_date_hint(&root.0, "app", "staging"), None);
}

fn failed_deploy_for_up_hint() -> Receipt {
    receipt(|value| {
        value["outcome"] = json!({"FailedAtStep": {"step": 2, "partial_update": false}});
        value["app_left_down"] = json!(true);
    })
}

fn up_for_hint(changes: impl FnOnce(&mut serde_json::Value)) -> Receipt {
    receipt(|value| {
        value["version"] = json!(2);
        value["kind"] = json!("up");
        value["run_id"] = json!("bbbb0002-0");
        value["steps"] = json!([]);
        value["maintenance_up_done"] = json!(true);
        value["maintenance_up_status"] = json!("Ok");
        value["maintenance_up_exit_code"] = json!(0);
        changes(value);
    })
}

#[test]
fn up_to_date_hint_keeps_failed_deploy_recovery_after_successful_up() {
    let root = TempRoot::new("failed-then-up");
    root.save(1000, &failed_deploy_for_up_hint());
    root.save(2000, &up_for_hint(|_| {}));

    let hint = up_to_date_hint(&root.0, "app", "staging").unwrap();
    assert!(
        hint.contains("(aaaa0001) did not succeed: Failed at step 2"),
        "{hint}"
    );
    assert!(hint.contains("`slip from-step staging 2`"), "{hint}");
    assert!(
        hint.contains("push the fix and run `slip deploy staging`"),
        "{hint}"
    );
    assert!(!hint.contains("maintenance mode"), "{hint}");
}

#[test]
fn up_to_date_hint_does_not_clear_maintenance_for_another_target() {
    for (field, value) in [("ssh_alias", "other-app"), ("path", "/srv/other")] {
        let root = TempRoot::new(&format!("up-other-{field}"));
        root.save(1000, &failed_deploy_for_up_hint());
        root.save(2000, &up_for_hint(|up| up["target"][field] = json!(value)));

        let hint = up_to_date_hint(&root.0, "app", "staging").unwrap();
        assert!(hint.contains("`slip from-step staging 2`"), "{hint}");
        assert!(hint.contains("may still be in maintenance mode"), "{hint}");
    }
}

#[test]
fn up_to_date_hint_keeps_failed_and_unfinished_up_recovery() {
    for unfinished in [false, true] {
        let root = TempRoot::new(if unfinished {
            "unfinished-up"
        } else {
            "failed-up"
        });
        root.save(1000, &failed_deploy_for_up_hint());
        root.save(
            2000,
            &up_for_hint(|up| {
                up["app_left_down"] = json!(true);
                up["maintenance_up_done"] = json!(false);
                up["maintenance_up_status"] = json!(if unfinished { "Unknown" } else { "Failed" });
                up["maintenance_up_exit_code"] = json!(null);
                if unfinished {
                    up["status"] = json!("InProgress");
                    up["phase"] = json!("MaintenanceUp");
                    up["finished_at_ms"] = json!(null);
                    up["outcome"] = json!(null);
                } else {
                    up["outcome"] = json!({"FailedAtStep": {"step": 0, "partial_update": false}});
                }
            }),
        );

        let hint = up_to_date_hint(&root.0, "app", "staging").unwrap();
        assert!(hint.contains("`slip from-step staging 2`"), "{hint}");
        assert!(hint.contains("slip receipts show bbbb0002"), "{hint}");
        if unfinished {
            assert!(hint.contains("`slip attach staging`"), "{hint}");
        } else {
            assert!(
                hint.contains("After checking the server, `slip up staging` tries again"),
                "{hint}"
            );
        }
    }
}

#[test]
fn up_to_date_hint_uses_latest_up_result_for_maintenance() {
    let root = TempRoot::new("latest-up");
    root.save(1000, &failed_deploy_for_up_hint());
    root.save(2000, &up_for_hint(|_| {}));
    root.save(
        3000,
        &up_for_hint(|up| {
            up["run_id"] = json!("cccc0003-0");
            up["app_left_down"] = json!(true);
            up["maintenance_up_done"] = json!(false);
            up["maintenance_up_status"] = json!("Unknown");
            up["maintenance_up_exit_code"] = json!(null);
            up["outcome"] = json!({"Unknown": {"step": 0, "reason": "connection lost"}});
        }),
    );

    let hint = up_to_date_hint(&root.0, "app", "staging").unwrap();
    assert!(hint.contains("`slip from-step staging 2`"), "{hint}");
    assert!(hint.contains("slip receipts show cccc0003"), "{hint}");
    assert!(
        hint.contains("Check the server before running anything again"),
        "{hint}"
    );
    assert!(!hint.contains("turns it off"), "{hint}");
}

#[test]
fn up_to_date_hint_finds_deploy_beyond_ten_up_receipts() {
    let root = TempRoot::new("many-up");
    root.save(1000, &failed_deploy_for_up_hint());
    for index in 0..11 {
        root.save(
            2000 + index,
            &up_for_hint(|up| {
                up["run_id"] = json!(format!("bbbb{index:04x}-0"));
            }),
        );
    }
    let hint = up_to_date_hint(&root.0, "app", "staging").unwrap();
    assert!(hint.contains("`slip from-step staging 2`"), "{hint}");
    assert!(!hint.contains("maintenance mode"), "{hint}");
}

#[test]
fn up_to_date_hint_preserves_up_recovery_without_a_deploy() {
    let root = TempRoot::new("only-up");
    root.save(1000, &up_for_hint(|_| {}));
    assert_eq!(up_to_date_hint(&root.0, "app", "staging"), None);

    root.save(
        2000,
        &up_for_hint(|up| {
            up["run_id"] = json!("cccc0003-0");
            up["app_left_down"] = json!(true);
            up["maintenance_up_done"] = json!(false);
            up["maintenance_up_status"] = json!("Unknown");
            up["maintenance_up_exit_code"] = json!(null);
            up["outcome"] = json!({"Unknown": {"step": 0, "reason": "connection lost"}});
        }),
    );
    let hint = up_to_date_hint(&root.0, "app", "staging").unwrap();
    assert!(hint.contains("slip receipts show cccc0003"), "{hint}");
    assert!(
        hint.contains("Check the server before running anything again"),
        "{hint}"
    );
    assert!(!hint.contains("from-step"), "{hint}");
}

#[test]
fn up_to_date_hint_does_not_apply_up_before_the_latest_deploy() {
    let root = TempRoot::new("up-before-deploy");
    root.save(1000, &up_for_hint(|_| {}));
    root.save(2000, &failed_deploy_for_up_hint());
    let hint = up_to_date_hint(&root.0, "app", "staging").unwrap();
    assert!(hint.contains("`slip from-step staging 2`"), "{hint}");
    assert!(hint.contains("may still be in maintenance mode"), "{hint}");

    root.save(
        3000,
        &receipt(|value| value["run_id"] = json!("dddd0004-0")),
    );
    assert_eq!(up_to_date_hint(&root.0, "app", "staging"), None);
}

#[test]
fn up_to_date_hint_does_not_hide_unknown_up_with_success_on_another_target() {
    let root = TempRoot::new("unknown-up-other-success");
    root.save(1000, &failed_deploy_for_up_hint());
    root.save(
        2000,
        &up_for_hint(|up| {
            up["app_left_down"] = json!(true);
            up["maintenance_up_done"] = json!(false);
            up["maintenance_up_status"] = json!("Unknown");
            up["maintenance_up_exit_code"] = json!(null);
            up["outcome"] = json!({"Unknown": {"step": 0, "reason": "connection lost"}});
        }),
    );
    root.save(
        3000,
        &up_for_hint(|up| {
            up["run_id"] = json!("cccc0003-0");
            up["target"]["ssh_alias"] = json!("other-app");
        }),
    );

    let hint = up_to_date_hint(&root.0, "app", "staging").unwrap();
    assert!(hint.contains("`slip from-step staging 2`"), "{hint}");
    assert!(hint.contains("slip receipts show bbbb0002"), "{hint}");
    assert!(
        hint.contains("Check the server before running anything again"),
        "{hint}"
    );
    assert!(!hint.contains("turns it off"), "{hint}");
}

#[test]
fn standalone_up_shows_its_saved_output_once() {
    let up = receipt(|value| {
        value["version"] = json!(2);
        value["kind"] = json!("up");
        value["maintenance_up_status"] = json!("Failed");
        value["maintenance_up_exit_code"] = json!(7);
        value["outcome"] = json!({"FailedAtStep": {"step": 0, "partial_update": false}});
        value["steps"] = json!([{
            "index": 0, "command": "php artisan up", "status": "Failed",
            "exit_code": 7, "output": ["up failed\u{1b}[2J"]
        }]);
    });
    let text = render_show(&up);
    assert!(text.contains("up failed\\x1b[2J"), "{text}");
    assert!(!text.contains("Maintenance off:"), "{text}");
    assert_eq!(text.matches("✓ ok").count(), 0, "{text}");
}
