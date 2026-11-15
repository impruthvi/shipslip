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
