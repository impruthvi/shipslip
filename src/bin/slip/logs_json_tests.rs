use super::*;
use serde_json::json;
use shipslip::transport::TransportError;

#[test]
fn every_logs_failure_has_a_schema_one_kind_and_the_text_commands_exit_code() {
    let cases: Vec<(Box<dyn Error>, serde_json::Value, u8)> = vec![
        (
            Box::new(invalid_input("bad flag")),
            json!({"kind":"invalid_input","message":"bad flag"}),
            2,
        ),
        (
            Box::new(logs::LogsError::BadSince("bad time".into())),
            json!({"kind":"invalid_input","message":"the server could not read `--since bad time` as a time"}),
            2,
        ),
        (
            Box::new(logs::LogsError::ZoneMissing("missing/zone".into())),
            json!({"kind":"zone_missing","zone":"missing/zone"}),
            1,
        ),
        (
            Box::new(logs::LogsError::Outside {
                path: "../secret".into(),
                env: "production".into(),
            }),
            json!({"kind":"log_path_not_trusted","path":"../secret","environment":"production"}),
            1,
        ),
        (
            Box::new(logs::LogsError::Unavailable("bad reply".into())),
            json!({"kind":"server_read_failed","message":"bad reply"}),
            1,
        ),
        (
            Box::new(logs::LogsError::Transport(TransportError::ConnectionLost(
                "lost".into(),
            ))),
            json!({"kind":"connection_lost","message":"lost"}),
            1,
        ),
    ];
    for (error, expected, code) in cases {
        let record = logs_error_record(error.as_ref());
        assert_eq!(record.exit_code(), code);
        let value = serde_json::to_value(shipslip::records::versioned_error(record)).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["error"], expected);
    }
    let too_old = logs_error_record(&logs::LogsError::TooOld);
    assert_eq!(too_old.exit_code(), 2);
    assert_eq!(
        serde_json::to_value(too_old).unwrap()["kind"],
        "invalid_input"
    );
    for (message, reason) in [
        ("Host key verification failed", "host_key_unknown"),
        ("REMOTE HOST IDENTIFICATION HAS CHANGED", "host_key_changed"),
        ("Permission denied (publickey)", "key_not_loaded"),
        ("ssh diagnostic", "other"),
    ] {
        for error in [
            Box::new(TransportError::Connect(message.into())) as Box<dyn Error>,
            Box::new(logs::LogsError::Transport(TransportError::Connect(
                message.into(),
            ))),
        ] {
            let record = logs_error_record(error.as_ref());
            assert_eq!(record.exit_code(), 1);
            let value = serde_json::to_value(record).unwrap();
            assert_eq!(value["kind"], "ssh_refused");
            assert_eq!(value["reason"], reason);
            assert_eq!(value["message"], message);
        }
    }
}
