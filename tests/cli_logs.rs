use serde_json::Value;
use std::process::Command;

#[test]
fn invalid_since_is_reported_before_loading_a_config_or_connecting() {
    let missing =
        std::env::temp_dir().join(format!("shipslip-since-config-{}", std::process::id()));
    for since in ["yesterday", "31d"] {
        let output = Command::new(env!("CARGO_BIN_EXE_slip"))
            .args([
                "--config",
                missing.to_str().unwrap(),
                "logs",
                "production",
                "--json",
                "--since",
                since,
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{since}");
        assert!(output.stderr.is_empty(), "{since}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["kind"], "invalid_input", "{since}");
    }
}

#[cfg(unix)]
#[test]
fn invalid_since_is_rejected_without_starting_ssh() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("shipslip-since-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let config = root.join(".shipslip.toml");
    std::fs::write(&config, "[project]\nname='fixture'\nstack='laravel'\n[env.production]\nssh='server'\npath='/srv/app'\nbranch='main'\nproduction=true\ntimezone='Asia/Kolkata'\n").unwrap();
    let ssh = root.join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\ntouch \"$SSH_CALLED\"\nexit 1\n").unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let called = root.join("ssh-called");
    let path = std::env::join_paths(
        std::iter::once(root.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    for since in ["yesterday", "31d", "2000-01-01", "2000-01-01 14:00"] {
        let output = Command::new(env!("CARGO_BIN_EXE_slip"))
            .env("PATH", &path)
            .env("SSH_CALLED", &called)
            .args([
                "--config",
                config.to_str().unwrap(),
                "logs",
                "production",
                "--json",
                "--since",
                since,
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{since}");
        assert!(output.stderr.is_empty(), "{since}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["kind"], "invalid_input", "{since}");
        assert!(!called.exists(), "{since}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn logs_json_rejects_mixed_output_and_reports_early_errors_on_stdout() {
    for args in [
        vec!["logs", "production", "--json", "--raw"],
        vec!["logs", "production", "--json", "--channels"],
        vec!["logs", "production", "1", "--json", "--md"],
        vec!["logs", "production", "--json", "--level", "fatal"],
        vec!["logs", "--json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_slip"))
            .env_remove("SHIPSLIP_CONFIG")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["error"]["kind"], "invalid_input");
    }
}

#[test]
fn logs_json_preserves_the_exit_code_for_a_missing_config() {
    let missing = std::env::temp_dir().join(format!("shipslip-no-config-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_slip"))
        .args([
            "--config",
            missing.to_str().unwrap(),
            "logs",
            "production",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["kind"], "unreadable");
}

#[test]
fn logs_json_reports_an_unknown_environment_as_invalid_input() {
    struct Dir(std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = Dir(std::env::temp_dir().join(format!("shipslip-logs-env-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let config = dir.0.join(".shipslip.toml");
    std::fs::write(&config, "[project]\nname = 'fixture'\nstack = 'laravel'\n[env.staging]\nssh = 'server'\npath = '/srv/app'\nbranch = 'main'\nproduction = false\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_slip"))
        .env_remove("SHIPSLIP_CONFIG")
        .args([
            "--config",
            config.to_str().unwrap(),
            "logs",
            "production",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["error"]["kind"], "invalid_input");
    assert!(value["error"]["message"]
        .as_str()
        .unwrap()
        .contains("production"));
}
