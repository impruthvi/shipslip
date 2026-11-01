use serde_json::Value;
use std::process::Command;

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
