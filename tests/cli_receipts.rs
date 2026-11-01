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

    fn list(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_slip"))
            .arg("receipts")
            .args(args)
            .current_dir(self.0.join("app"))
            .env("HOME", self.0.join("home"))
            .env("TZ", "UTC")
            .env_remove("SHIPSLIP_CONFIG")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    fn status(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_slip"))
            .arg("status")
            .args(args)
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
fn status_json_stdout_is_versioned_without_records() {
    let fixture = Fixture::new();
    let output = fixture.status(&["--json"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"schema": 1, "repos": [], "problems": []})
    );
}

#[test]
fn status_json_errors_match_text_exit_codes() {
    let fixture = Fixture::new();
    let root = fixture.receipts().ancestors().nth(2).unwrap().to_path_buf();
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    fs::write(&root, "not a directory").unwrap();
    let text = fixture.status(&[]);
    let output = fixture.status(&["--json"]);
    assert_eq!(text.status.code(), Some(1));
    assert_eq!(output.status.code(), text.status.code());
    assert!(output.stderr.is_empty(), "{output:?}");
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["schema"], 1);
    assert_eq!(error["error"]["kind"], "unreadable");
}

#[test]
fn status_json_reports_path_errors_before_reading_records() {
    let output = Command::new(env!("CARGO_BIN_EXE_slip"))
        .args(["status", "--json"])
        .env_remove("HOME")
        .env_remove("SHIPSLIP_CONFIG")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty(), "{output:?}");
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["schema"], 1);
    assert_eq!(error["error"]["kind"], "unreadable");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("HOME is not set"));
}

#[test]
fn status_json_argument_errors_preserve_the_text_exit_code() {
    let fixture = Fixture::new();
    for args in [vec!["--bogus"], vec!["--compare", "staging", "production"]] {
        let text = fixture.status(&args);
        let mut json_args = args;
        json_args.insert(0, "--json");
        let output = fixture.status(&json_args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty(), "{output:?}");
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["schema"], 1);
        assert_eq!(error["error"]["kind"], "invalid_input");
        if json_args.contains(&"--bogus") {
            assert_eq!(output.status.code(), text.status.code());
        }
    }
}

#[test]
fn status_json_rejects_inherited_or_explicit_config_as_versioned_errors() {
    let fixture = Fixture::new();
    for explicit in [false, true] {
        let invoke = |json: bool| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_slip"));
            if explicit {
                command.args(["--config", "project.toml"]);
            } else {
                command.env("SHIPSLIP_CONFIG", "project.toml");
            }
            command.arg("status");
            if json {
                command.arg("--json");
            }
            command
                .current_dir(fixture.0.join("app"))
                .env("HOME", fixture.0.join("home"))
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap()
        };
        let text = invoke(false);
        let output = invoke(true);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.status.code(), text.status.code());
        assert!(output.stderr.is_empty(), "{output:?}");
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["schema"], 1);
        assert_eq!(error["error"]["kind"], "invalid_input");
        assert!(error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("omit --config"));
    }
}

#[test]
fn receipt_list_json_argument_errors_preserve_the_text_exit_code() {
    let fixture = Fixture::new();
    for (args, message) in [
        (vec!["--bogus"], "unknown receipts option --bogus"),
        (
            vec!["staging", "production"],
            "receipts accepts at most one environment name",
        ),
    ] {
        let text = fixture.list(&args);
        let mut json_args = args;
        json_args.insert(0, "--json");
        let output = fixture.list(&json_args);
        assert_eq!(text.status.code(), Some(2), "{text:?}");
        assert!(text.stdout.is_empty(), "{text:?}");
        assert_eq!(text.stderr, format!("slip: {message}\n").as_bytes());
        assert_eq!(output.status.code(), text.status.code(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"schema": 1, "error": {"kind": "invalid_input", "message": message}})
        );
    }
}

#[test]
fn receipt_show_json_argument_errors_preserve_the_text_exit_code() {
    let fixture = Fixture::new();
    for (args, message) in [
        (vec!["show"], "receipts show requires a receipt ID"),
        (
            vec!["show", "a", "b"],
            "receipts show accepts one receipt ID",
        ),
        (
            vec!["show", "ID", "--bogus"],
            "unknown receipts show option --bogus",
        ),
    ] {
        let text = fixture.list(&args);
        let mut json_args = args;
        json_args.push("--json");
        let output = fixture.list(&json_args);
        assert_eq!(text.status.code(), Some(2), "{text:?}");
        assert!(text.stdout.is_empty(), "{text:?}");
        assert_eq!(text.stderr, format!("slip: {message}\n").as_bytes());
        assert_eq!(output.status.code(), text.status.code(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"schema": 1, "error": {"kind": "invalid_input", "message": message}})
        );
    }
}

#[test]
fn receipt_show_json_format_conflicts_are_versioned_errors() {
    let fixture = Fixture::new();
    for format in ["--md", "--with-details"] {
        let text = fixture.list(&["show", "ID", format]);
        let output = fixture.list(&["show", "ID", "--json", format]);
        assert_eq!(text.status.code(), Some(2), "{text:?}");
        assert_eq!(output.status.code(), text.status.code(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"schema": 1, "error": {
                "kind": "invalid_input",
                "message": "--json cannot be combined with --md or --with-details"
            }})
        );
    }
}

#[test]
fn receipt_json_reports_discovered_inherited_and_explicit_config_errors() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("app/bad.toml"), "[broken").unwrap();
    fs::remove_file(fixture.0.join("app/.shipslip.toml")).unwrap();
    for config in [None, Some("missing.toml"), Some("bad.toml")] {
        for explicit in [false, true] {
            for args in [vec![], vec!["staging"], vec!["show", "ID"]] {
                let invoke = |json: bool| {
                    let mut command = Command::new(env!("CARGO_BIN_EXE_slip"));
                    command.env_remove("SHIPSLIP_CONFIG");
                    if let Some(config) = config {
                        if explicit {
                            command.args(["--config", config]);
                        } else {
                            command.env("SHIPSLIP_CONFIG", config);
                        }
                    }
                    command.arg("receipts").args(&args);
                    if json {
                        command.arg("--json");
                    }
                    command
                        .current_dir(fixture.0.join("app"))
                        .env("HOME", fixture.0.join("home"))
                        .stdin(std::process::Stdio::null())
                        .output()
                        .unwrap()
                };
                let text = invoke(false);
                let output = invoke(true);
                assert_eq!(text.status.code(), Some(1), "{text:?}");
                assert!(text.stdout.is_empty(), "{text:?}");
                assert_eq!(output.status.code(), text.status.code(), "{output:?}");
                assert!(output.stderr.is_empty(), "{output:?}");
                let error: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(error["schema"], 1);
                assert_eq!(error["error"]["kind"], "unreadable");
                let message = error["error"]["message"].as_str().unwrap();
                assert_eq!(text.stderr, format!("slip: {message}\n").as_bytes());
                assert!(
                    message.contains(match config {
                        None => "no `.shipslip.toml` found",
                        Some("missing.toml") => "could not access `missing.toml`",
                        Some(_) => "TOML parse error",
                    }),
                    "{message}"
                );
            }
        }
    }
}

#[test]
fn receipt_json_reports_path_errors_before_reading_records() {
    let fixture = Fixture::new();
    for args in [vec![], vec!["staging"], vec!["show", "ID"]] {
        let invoke = |json: bool| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_slip"));
            command.arg("receipts").args(&args);
            if json {
                command.arg("--json");
            }
            command
                .current_dir(fixture.0.join("app"))
                .env_remove("HOME")
                .env_remove("SHIPSLIP_CONFIG")
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap()
        };
        let text = invoke(false);
        let output = invoke(true);
        assert_eq!(text.status.code(), Some(1), "{text:?}");
        assert!(text.stdout.is_empty(), "{text:?}");
        assert_eq!(output.status.code(), text.status.code(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["schema"], 1);
        assert_eq!(error["error"]["kind"], "unreadable");
        let message = error["error"]["message"].as_str().unwrap();
        assert!(message.contains("HOME is not set"), "{message}");
        assert_eq!(text.stderr, format!("slip: {message}\n").as_bytes());
    }
}

#[test]
fn receipt_json_early_error_messages_preserve_untrusted_text() {
    let fixture = Fixture::new();
    let option = "--bad`\u{1b}[31m\u{7}\n\t";
    for (args, message) in [
        (
            vec![option, "--json"],
            format!("unknown receipts option {option}"),
        ),
        (
            vec!["show", "ID", option, "--json"],
            format!("unknown receipts show option {option}"),
        ),
    ] {
        let output = fixture.list(&args);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"schema": 1, "error": {"kind": "invalid_input", "message": message}})
        );
        assert_eq!(
            output.stdout.iter().filter(|&&byte| byte == b'\n').count(),
            1
        );
    }
}

#[test]
fn receipt_help_describes_json_for_list_and_show() {
    let fixture = Fixture::new();
    let output = fixture.list(&["--help"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let help = String::from_utf8(output.stdout).unwrap();
    let receipts = help.split("Receipts options:\n").nth(1).unwrap();
    let json_option = receipts
        .lines()
        .find(|line| line.contains("--json"))
        .unwrap();
    assert!(json_option.contains("list"), "{json_option}");
    assert!(json_option.contains("show ID"), "{json_option}");
}

#[test]
fn receipt_list_json_stdout_is_versioned_for_default_and_all_environment_forms() {
    let fixture = Fixture::new();
    for args in [vec!["--json"], vec!["staging", "--all", "--json"]] {
        let output = fixture.list(&args);
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"schema": 1, "rows": [], "older": 0})
        );
    }
    fixture.save("aabbccdd-0", &json!({"version": 99}));
    for args in [vec!["--json"], vec!["staging", "--all", "--json"]] {
        let output = fixture.list(&args);
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let list: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(list["schema"], 1);
        assert_eq!(list["rows"][0]["state"], "newer");
        assert_eq!(list["rows"][0]["id"], "aabbccdd");
    }
}

#[test]
fn receipt_list_json_errors_match_text_exit_codes() {
    let fixture = Fixture::new();
    let root = fixture
        .receipts()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    fs::write(&root, "not a directory").unwrap();
    let text = fixture.list(&[]);
    let output = fixture.list(&["--json"]);
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    assert_eq!(output.status.code(), text.status.code(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["schema"], 1);
    assert_eq!(error["error"]["kind"], "unreadable");
}

#[test]
fn receipt_list_json_applies_the_default_limit_and_all_environment_filter() {
    let fixture = Fixture::new();
    for index in 0..21 {
        fixture.save(&format!("{index:08x}-0"), &json!({"version": 99}));
    }
    let other = fixture.receipts().parent().unwrap().join("production");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("2000-ffffffff-0.json"), r#"{"version":99}"#).unwrap();

    let output = fixture.list(&["--json"]);
    assert!(output.status.success(), "{output:?}");
    let list: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(list["schema"], 1);
    assert_eq!(list["rows"].as_array().unwrap().len(), 20);
    assert_eq!(list["rows"][0]["env"], "production");
    assert_eq!(list["older"], 2);

    let output = fixture.list(&["staging", "--all", "--json"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let list: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(list["schema"], 1);
    assert_eq!(list["rows"].as_array().unwrap().len(), 21);
    assert!(list["rows"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["env"] == "staging"));
    assert_eq!(list["older"], 0);
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
fn receipt_json_and_terminal_list_preserve_the_unknown_run_fixture() {
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
    let output = fixture.list(&[]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "Receipts for app, newest first\n\n",
            "ID        STARTED (UTC)     ENV      FROM → TARGET                OUTCOME\n",
            "aabbccdd  1970-01-01 00:00  staging  aaaaaaaaaaaa → bbbbbbbbbbbb  Unknown at step 1  ⓘ log not recorded  ⓘ smoke not recorded\n",
            "\nShow one: slip receipts show ID [--md]\n",
        )
    );
}
