//! The CLI refuses untrusted configs and changed attach settings before SSH.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CONFIG: &str = r#"[project]
name = "trust-test"
stack = "laravel"

[env.staging]
ssh = "trust-test-staging"
path = "/srv/trust-test"
branch = "main"
production = false
maintenance = false

[recipe.deploy]
steps = ["php artisan migrate --force"]
"#;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("shipslip-cli-trust-{}-{name}", std::process::id()));
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

    fn slip(&self, args: &[&str]) -> std::process::Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_slip"))
            .args(args)
            .current_dir(self.root.join("app"))
            .env("HOME", self.root.join("home"))
            .env("PATH", path)
            .env_remove("SHIPSLIP_CONFIG")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    fn ssh_called(&self) -> bool {
        Path::new(&self.root.join("ssh-called")).exists()
    }

    fn trust(&self) {
        let trust = Command::new(env!("CARGO_BIN_EXE_slip"))
            .args(["trust", "staging"])
            .current_dir(self.root.join("app"))
            .env("HOME", self.root.join("home"))
            .env_remove("SHIPSLIP_CONFIG")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(b"staging\n")?;
                child.wait_with_output()
            })
            .unwrap();
        assert!(
            trust.status.success()
                && String::from_utf8_lossy(&trust.stdout).contains("staging: trusted"),
            "{}",
            String::from_utf8_lossy(&trust.stderr)
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn untrusted_config_refuses_every_server_command_before_ssh() {
    for (name, args) in [
        ("deploy", vec!["deploy", "staging"]),
        ("rerun", vec!["rerun", "staging"]),
        ("from-step", vec!["from-step", "staging", "1"]),
        ("attach", vec!["attach", "staging"]),
        ("up", vec!["up", "staging"]),
        ("break-lock", vec!["break-lock", "staging"]),
    ] {
        let fixture = Fixture::new(name);
        let output = fixture.slip(&args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{name}: {stderr}");
        assert!(
            stderr.contains("config for `staging` is untrusted; run `slip trust staging` first"),
            "{name}: {stderr}"
        );
        assert!(!fixture.ssh_called(), "{name}: ssh was started");
    }
}

#[test]
fn config_changed_after_trust_is_refused_before_ssh() {
    let fixture = Fixture::new("changed");
    fixture.trust();
    // The trusted config gets past the gate, so the fake ssh does detect a connection.
    let output = fixture.slip(&["deploy", "staging"]);
    assert!(
        fixture.ssh_called(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_file(fixture.root.join("ssh-called")).unwrap();
    fs::write(
        fixture.root.join("app/.shipslip.toml"),
        CONFIG.replace("migrate --force", "migrate:fresh --force"),
    )
    .unwrap();
    let output = fixture.slip(&["deploy", "staging"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("is untrusted"), "{stderr}");
    assert!(!fixture.ssh_called());
}

#[test]
fn attach_with_newly_trusted_different_settings_is_refused_before_ssh() {
    let fixture = Fixture::new("attach-changed");
    fixture.trust();
    let config = shipslip::config::LoadedConfig::load(&fixture.root.join("app"), None).unwrap();
    let receipt = serde_json::json!({
        "version": 1,
        "kind": "deploy",
        "project": config.project_name(),
        "repo_root": config.repo_root(),
        "run_id": "aabbccdd",
        "owner_pid": 0,
        "confirmed": true,
        "started_at_ms": 1,
        "status": "InProgress",
        "phase": { "Step": 1 },
        "target": config.target("staging").unwrap(),
        "run_plan": "Deploy",
        "from_sha": "a".repeat(40),
        "target_sha": "b".repeat(40),
        "commits": [],
        "recipe_hash": "c".repeat(64),
        "steps": [{
            "index": 1,
            "command": "php artisan migrate --force",
            "status": "Running",
            "output": []
        }],
        "maintenance_down_done": false,
        "maintenance_up_done": false,
        "app_left_down": false,
        "mutation_started": true
    });
    let receipts_root = if cfg!(target_os = "macos") {
        "home/Library/Application Support/Shipslip/receipts"
    } else {
        "home/.local/share/shipslip/receipts"
    };
    let receipts = fixture.root.join(receipts_root).join("trust-test/staging");
    fs::create_dir_all(&receipts).unwrap();
    let receipt_path = receipts.join("1-aabbccdd.json");
    let saved = serde_json::to_vec(&receipt).unwrap();
    fs::write(&receipt_path, &saved).unwrap();

    fs::write(
        fixture.root.join("app/.shipslip.toml"),
        CONFIG.replace("migrate --force", "migrate:fresh --force"),
    )
    .unwrap();
    fixture.trust();

    let output = fixture.slip(&["attach", "staging"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(
            "the unfinished run uses different deploy settings; inspect the receipt before recovery"
        ),
        "{stderr}"
    );
    assert!(!fixture.ssh_called(), "ssh was started: {stderr}");
    assert_eq!(fs::read(receipt_path).unwrap(), saved);
}
