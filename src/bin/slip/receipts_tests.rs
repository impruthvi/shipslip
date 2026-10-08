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
fn next_steps_use_only_recorded_facts() {
    let failed = receipt(|value| {
        value["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
        value["steps"][0]["status"] = json!("Failed");
    });
    let text = render_show(&failed);
    assert!(text.contains("Server HEAD at end: not recorded"));
    assert!(text.contains("`slip from-step staging 1`"));

    let unfinished = receipt(|value| {
        value["status"] = json!("InProgress");
        value["app_left_down"] = json!(true);
    });
    let lines = next_steps(&unfinished);
    assert_eq!(
        lines,
        [
            "The run did not finish. If no deploy is running, resume it from /work/app with `slip attach staging`.",
            "The app may still be in maintenance mode. After checking the server, `slip up staging` turns it off.",
        ]
    );
    assert!(render_show(&unfinished)
        .contains("Outcome:            Unfinished (last recorded: Succeeded)"));

    // Stopping after the last step leaves nothing to run.
    let stopped = receipt(|value| {
        value["outcome"] = json!({"StoppedAfterStep": {"step": 2, "reason": "Requested"}});
    });
    assert!(next_steps(&stopped).is_empty());
}
