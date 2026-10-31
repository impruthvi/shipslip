//! Receipt JSON is the shared record, including typed errors and text exit codes.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "shipslip-cli-receipts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("app/.git")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        fs::write(
            root.join("app/.shipslip.toml"),
            r#"
[project]
name = "app"
stack = "laravel"
[env.staging]
ssh = "app"
path = "/srv/app"
branch = "main"
production = false
maintenance = false
[recipe.deploy]
steps = ["php artisan optimize"]
"#,
        )
        .unwrap();
        Self(root)
    }

    fn receipts(&self) -> PathBuf {
        let home = self.0.join("home");
        if cfg!(target_os = "macos") {
            home.join("Library/Application Support/Shipslip/receipts/app/staging")
        } else {
            home.join(".local/share/shipslip/receipts/app/staging")
        }
    }

    fn save(&self, id: &str, value: &Value) {
        fs::create_dir_all(self.receipts()).unwrap();
        fs::write(
            self.receipts().join(format!("1000-{id}.json")),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }

    fn show(&self, id: &str, json: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_slip"));
        command.args(["receipts", "show", id]);
        if json {
            command.arg("--json");
        }
        command
            .current_dir(self.0.join("app"))
            .env("HOME", self.0.join("home"))
            .env_remove("SHIPSLIP_CONFIG")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn receipt_json_errors_match_text_exit_codes() {
    let fixture = Fixture::new();
    for (id, code, error) in [
        ("deadbeef", 2, json!({"kind": "missing"})),
        (
            "aaaa",
            2,
            json!({"kind": "ambiguous", "ids": ["aaaa0002", "aaaa0001"]}),
        ),
        (
            "aaaa0001",
            2,
            json!({"kind": "newer_version", "version": 99}),
        ),
        ("aaaa0002", 1, json!({"kind": "unreadable"})),
    ] {
        fixture.save("aaaa0001-0", &json!({"version": 99}));
        fixture.save("aaaa0002-0", &json!({"version": 1}));
        let text = fixture.show(id, false);
        let output = fixture.show(id, true);
        assert_eq!(text.status.code(), Some(code), "{text:?}");
        assert_eq!(output.status.code(), text.status.code(), "{output:?}");
        let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
        if error["kind"] == "unreadable" {
            let message = actual["error"]
                .as_object_mut()
                .unwrap()
                .remove("message")
                .unwrap();
            assert!(message.as_str().unwrap().contains("invalid receipt"));
        }
        assert_eq!(actual, json!({"schema": 1, "error": error}));
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn receipt_json_stdout_is_a_versioned_record_with_unknown_outcome_and_raw_text() {
    let fixture = Fixture::new();
    let config = shipslip::config::LoadedConfig::load(&fixture.0.join("app"), None).unwrap();
    fixture.save(
        "aabbccdd-0",
        &json!({
            "version": 1, "project": "app", "repo_root": "/work/app", "run_id": "aabbccdd-0",
            "owner_pid": 1, "confirmed": true, "started_at_ms": 1000, "finished_at_ms": 2000,
            "status": "Final", "phase": "Finished", "target": config.target("staging").unwrap(),
            "run_plan": "Deploy", "from_sha": "a".repeat(40), "target_sha": "b".repeat(40),
            "commits": [], "recipe_hash": "c".repeat(64),
            "steps": [{"index": 1, "command": "php artisan optimize", "status": "Unknown",
                "started_at_ms": 1500, "output": ["\u{1b}[31m`<b>`\u{7}"]}],
            "maintenance_down_done": false, "maintenance_up_done": false,
            "app_left_down": false, "mutation_started": true,
            "outcome": {"Unknown": {"step": 1, "reason": "connection lost"}}
        }),
    );
    let output = fixture.show("aabbccdd", true);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record["schema"], 1);
    assert_eq!(record["run_id"], "aabbccdd-0");
    assert_eq!(
        record["outcome"],
        json!({"kind": "unknown", "step": 1, "unfinished": false})
    );
    assert_eq!(
        record["steps"][0],
        json!({
            "kind": "step", "index": 1, "command": "php artisan optimize", "status": "unknown",
            "exit_code": null, "started_at_ms": 1500, "finished_at_ms": null, "duration_ms": null,
            "output": ["\u{1b}[31m`<b>`\u{7}"]
        })
    );
}
