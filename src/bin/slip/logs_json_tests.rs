use super::*;
use shipslip::transport::TransportError;

#[test]
fn every_logs_failure_has_a_schema_one_kind_and_the_text_commands_exit_code() {
    let cases: Vec<(Box<dyn Error>, &str, u8)> = vec![
        (Box::new(invalid_input("bad flag")), "invalid_input", 2),
        (
            Box::new(logs::LogsError::BadSince("bad time".into())),
            "invalid_input",
            2,
        ),
        (Box::new(logs::LogsError::TooOld), "invalid_input", 2),
        (
            Box::new(logs::LogsError::ZoneMissing("missing/zone".into())),
            "zone_missing",
            1,
        ),
        (
            Box::new(logs::LogsError::Outside {
                path: "../secret".into(),
                env: "production".into(),
            }),
            "log_path_not_trusted",
            1,
        ),
        (
            Box::new(logs::LogsError::Unavailable("bad reply".into())),
            "server_read_failed",
            1,
        ),
        (
            Box::new(logs::LogsError::Transport(TransportError::ConnectionLost(
                "lost".into(),
            ))),
            "connection_lost",
            1,
        ),
    ];
    for (error, kind, code) in cases {
        let record = logs_error_record(error.as_ref());
        assert_eq!(record.exit_code(), code);
        let value = serde_json::to_value(shipslip::records::versioned_error(record)).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["error"]["kind"], kind);
    }
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
