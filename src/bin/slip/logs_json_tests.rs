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
        (
            Box::new(logs::LogsError::Transport(TransportError::Connect(
                "closed session".into(),
            ))),
            json!({"kind":"connection_lost","message":"closed session"}),
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
        let record = logs_error_record(&TransportError::Connect(message.into()));
        assert_eq!(record.exit_code(), 1);
        let value = serde_json::to_value(record).unwrap();
        assert_eq!(value["kind"], "ssh_refused");
        assert_eq!(value["reason"], reason);
        assert_eq!(value["message"], message);
    }
}

pub(super) struct LogReply(pub(super) String);

impl shipslip::transport::Transport for LogReply {
    async fn run(
        &self,
        script: &str,
        output: tokio::sync::mpsc::Sender<String>,
        _line_limit: usize,
    ) -> Result<i32, TransportError> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let reply = if script.contains("@time") {
            format!("@time 1790812800 1790726400 2026-09-30 00:00:00 2026-09-30 +0000 +0000\n@marker missing\n@reflog missing\n@file 1 {} 1790812800 1 2026-10-01 00:00:00 checksum {}\n{}\n@end", self.0.len(), STANDARD.encode("/srv/app/storage/logs/laravel.log"), STANDARD.encode("storage/logs/laravel.log"))
        } else {
            let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            gzip.write_all(self.0.as_bytes()).unwrap();
            format!("@data 0\n{}\n@end", STANDARD.encode(gzip.finish().unwrap()))
        };
        for line in reply.lines() {
            output.send(line.into()).await.unwrap();
        }
        Ok(0)
    }

    async fn reconnect(&self) -> Result<(), TransportError> {
        panic!("log reads never reconnect")
    }
}

pub(super) async fn read(text: &str) -> logs::Snapshot {
    let target = DeployTarget {
        env: "production".into(),
        production: true,
        ssh_alias: "app".into(),
        path: "/srv/app".into(),
        branch: "main".into(),
        steps: Vec::new(),
        maintenance: false,
        watch_log: false,
        log: None,
        log_daily: false,
        smoke_url: None,
        timezone: None,
        logs: Default::default(),
    };
    logs::snapshot(&LogReply(text.into()), &target, &logs::Request::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn detail_json_keeps_long_entry_text_and_its_true_size_without_clipping() {
    let text = format!(
        "[2026-10-01 00:00:00] production.ERROR: RuntimeException: head {}\n#0 /srv/app/app/Job.php(17): Job->handle()\n{} tail",
        "é🙂".repeat(2000),
        "🙂é".repeat(2000),
    );
    let snapshot = read(&text).await;
    let groups = logs::group(&snapshot, Level::Error, None);
    let (record, code) =
        logs_json_record(&snapshot, &groups, Some("1"), Level::Error, None).unwrap();
    assert_eq!(code, 0);
    assert_eq!(record["latest"]["text"], text);
    assert_eq!(record["latest"]["clipped"], false);
    assert_eq!(record["latest"]["truncated"], false);
    assert_eq!(record["latest"]["bytes"], text.len());
    assert!(record.get("groups").is_none());
    assert!(record.get("overflow_groups").is_none());
    assert!(record.get("text_fields").is_none());
    assert!(record.get("clipped_fields").is_none());
    let (summary, code) = logs_json_record(&snapshot, &groups, None, Level::Error, None).unwrap();
    assert_eq!(code, 0);
    assert!(summary.get("text_fields").is_none());
    assert!(summary.get("clipped_fields").is_none());
}

#[tokio::test]
async fn detail_json_finds_a_group_by_row_short_id_or_full_id_and_reports_lookup_failures() {
    let text = (0..20)
        .map(|n| format!("[2026-10-01 02:00:00] production.ERROR: Problem{n}Exception: failed\n"))
        .collect::<String>();
    let snapshot = read(&text).await;
    let groups = logs::group(&snapshot, Level::Error, None);
    let (summary, code) = logs_json_record(&snapshot, &groups, None, Level::Error, None).unwrap();
    assert_eq!(code, 0);
    let rows = summary["groups"].as_array().unwrap();
    assert_eq!(rows.len(), 20);
    let id = rows[1]["id"].as_str().unwrap();
    let full_id = rows[1]["full_id"].as_str().unwrap();
    for query in ["2", id, &id.to_uppercase(), full_id] {
        let (record, code) =
            logs_json_record(&snapshot, &groups, Some(query), Level::Error, None).unwrap();
        assert_eq!(code, 0, "{query}");
        assert_eq!(record["schema"], 1);
        assert_eq!(record["group"], rows[1]);
        assert!(record.get("groups").is_none());
        assert!(record.get("overflow_groups").is_none());
        assert!(record["latest"]["text"]
            .as_str()
            .unwrap()
            .contains(rows[1]["exception"].as_str().unwrap()));
    }

    for query in ["0", "21", "zzzz"] {
        let (record, code) =
            logs_json_record(&snapshot, &groups, Some(query), Level::Error, None).unwrap();
        assert_eq!(code, 1, "{query}");
        assert_eq!(record, json!({"schema":1, "error":{"kind":"missing"}}));
    }

    // Twenty 16-hex-digit IDs must share a first digit.
    let sharing = |prefix: &str| {
        rows.iter()
            .filter(|row| row["full_id"].as_str().unwrap().starts_with(prefix))
            .map(|row| row["id"].clone())
            .collect::<Vec<_>>()
    };
    let prefix = (0..16)
        .map(|digit| format!("{digit:x}"))
        .find(|prefix| sharing(prefix).len() > 1)
        .unwrap();
    let (record, code) =
        logs_json_record(&snapshot, &groups, Some(&prefix), Level::Error, None).unwrap();
    assert_eq!(code, 2);
    assert_eq!(
        record,
        json!({"schema":1, "error":{"kind":"ambiguous", "ids":sharing(&prefix)}})
    );
}
