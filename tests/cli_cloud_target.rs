//! A Cloud target is approved like any environment, and every command that
//! needs a server refuses it before starting ssh.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const CONFIG: &str = r#"[project]
name = "cloud-test"
stack = "laravel"

[env.production]
cloud_environment = "env-9f2c41d0"
production = true
"#;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("shipslip-cli-cloud-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("app/.git")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("app/.shipslip.toml"), CONFIG).unwrap();
        let ssh = root.join("bin/ssh");
        fs::write(
            &ssh,
            format!(
                "#!/bin/sh\necho called >> '{}'\nexit 255\n",
                root.join("ssh-called").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }

    fn slip(&self, args: &[&str], input: &str) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut child = Command::new(env!("CARGO_BIN_EXE_slip"))
            .args(args)
            .current_dir(self.root.join("app"))
            .env("HOME", self.root.join("home"))
            .env("PATH", path)
            .env_remove("SHIPSLIP_CONFIG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn ssh_called(&self) -> bool {
        self.root.join("ssh-called").exists()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn trust_shows_the_cloud_environment_id_and_asks_again_when_it_changes() {
    let fixture = Fixture::new("trust");
    let output = fixture.slip(&["trust", "production"], "production\n");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        stdout.contains("  cloud_environment: env-9f2c41d0\n  production: true\n"),
        "{stdout}"
    );
    assert!(stdout.contains("production: trusted"), "{stdout}");

    let output = fixture.slip(&["trust", "production"], "");
    assert!(text(&output.stdout).contains("production: already trusted"));

    fs::write(
        fixture.root.join("app/.shipslip.toml"),
        CONFIG.replace("env-9f2c41d0", "env-other"),
    )
    .unwrap();
    let output = fixture.slip(&["trust", "production"], "\n");
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("  cloud_environment: env-9f2c41d0 -> env-other"),
        "{stdout}"
    );
    assert!(stdout.contains("production: remains untrusted"), "{stdout}");
    assert!(!fixture.ssh_called());
}

#[test]
fn every_server_command_refuses_a_cloud_target_before_ssh() {
    for approved in [false, true] {
        for (name, args) in [
            ("deploy", vec!["deploy", "production"]),
            ("rerun", vec!["rerun", "production"]),
            ("from-step", vec!["from-step", "production", "1"]),
            ("attach", vec!["attach", "production"]),
            ("up", vec!["up", "production"]),
            ("break-lock", vec!["break-lock", "production"]),
        ] {
            let fixture = Fixture::new(&format!("{name}-{approved}"));
            if approved {
                fixture.slip(&["trust", "production"], "production\n");
            }
            let output = fixture.slip(&args, "");
            let stderr = text(&output.stderr);
            assert_eq!(output.status.code(), Some(2), "{name}: {stderr}");
            assert!(
                stderr.contains("`production` is a Cloud target. Cloud targets are record-only"),
                "{name}: {stderr}"
            );
            assert!(!fixture.ssh_called(), "{name}: ssh was started");
        }
    }
}

#[test]
fn slip_logs_says_it_does_not_support_cloud_targets_yet() {
    let fixture = Fixture::new("logs");
    let output = fixture.slip(&["logs", "production"], "");
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(
            "`production` is a Cloud target; `slip logs` does not support Cloud targets yet"
        ),
        "{stderr}"
    );
    assert!(!fixture.ssh_called());
}

#[test]
fn pull_refuses_a_server_target_and_an_unapproved_cloud_target_before_asking_for_a_token() {
    let fixture = Fixture::new("pull-refusals");
    fs::write(
        fixture.root.join("app/.shipslip.toml"),
        format!("{CONFIG}\n[env.staging]\nssh = \"staging-host\"\npath = \"/srv/app\"\nbranch = \"main\"\nproduction = false\n"),
    )
    .unwrap();
    for (env, message) in [
        (
            "staging",
            "`staging` is a server target; only a Cloud target can be pulled",
        ),
        (
            "production",
            "config for `production` is untrusted; run `slip trust production` first",
        ),
    ] {
        let output = fixture.slip(&["pull", env], "");
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{env}: {stderr}");
        assert!(stderr.contains(message), "{env}: {stderr}");
        assert!(!text(&output.stdout).contains("token"), "{env}");
    }
    assert!(!fixture.ssh_called());
}

#[test]
fn pull_asks_for_the_token_only_at_a_hidden_prompt() {
    let fixture = Fixture::new("pull-prompt");
    fixture.slip(&["trust", "production"], "production\n");
    let curl = fixture.root.join("bin/curl");
    fs::write(
        &curl,
        format!(
            "#!/bin/sh\necho called >> '{}'\nexit 7\n",
            fixture.root.join("curl-called").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
    let output = fixture.slip(&["pull", "production"], "7|piped-token\n");
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("hidden prompt, which needs an interactive terminal"),
        "{stderr}"
    );
    assert!(!fixture.root.join("curl-called").exists());
}

#[test]
fn receipts_lists_cloud_deployments_marked_as_not_run_by_shipslip() {
    let fixture = Fixture::new("pull-receipts");
    let receipts = if cfg!(target_os = "macos") {
        "home/Library/Application Support/Shipslip/receipts"
    } else {
        "home/.local/share/shipslip/receipts"
    };
    let dir = fixture.root.join(receipts).join("cloud-test/production");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("1763285400000-depl-9a1b2c3d.json"),
        r#"{"version": 3, "kind": "cloud", "project": "cloud-test", "env": "production",
            "cloud_environment": "env-9f2c41d0", "deployment_id": "depl-9a1b2c3d",
            "cloud_status": "deployment.succeeded", "branch": "main",
            "commit_hash": "4f9e2a1c0b7d4f9e2a1c0b7d4f9e2a1c0b7d4f9e",
            "commit_message": "Ship it", "commit_author": "Taylor", "failure_reason": null,
            "reported": {"started_at": "2025-11-16T09:30:00Z", "finished_at": "2025-11-16T09:31:00Z"},
            "pulled_at_ms": 1763285500000}"#,
    )
    .unwrap();
    let output = fixture.slip(&["receipts"], "");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(stdout.contains("depl-9a1  "), "{stdout}");
    assert!(stdout.contains("(Cloud)"), "{stdout}");
    assert!(stdout.contains("Cloud deployed 4f9e2a1c0b7d"), "{stdout}");
    assert!(
        stdout.contains("Succeeded  ⓘ Laravel Cloud ran it, not Shipslip"),
        "{stdout}"
    );

    let output = fixture.slip(&["receipts", "--json"], "");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["rows"][0]["state"], "cloud");
    assert_eq!(json["rows"][0]["receipt"]["commit_author"], "Taylor");
}
