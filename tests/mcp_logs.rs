//! Exercise JSON-RPC with a Reader backed by temp records and a fake transport.
#[allow(dead_code)]
#[path = "../src/bin/slip/mcp.rs"]
mod mcp;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use shipslip::config::{approve_trust, LoadedConfig};
use shipslip::read_only::{ApprovedTarget, Connector, Reader};
use shipslip::transport::{Transport, TransportError};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

struct Fixture {
    root: PathBuf,
    calls: Arc<AtomicUsize>,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "shipslip-mcp-logs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".shipslip.toml"), "[project]\nname='app'\nstack='laravel'\n[env.production]\nssh='app'\npath='/srv/app'\nbranch='main'\nproduction=true\n[recipe.deploy]\nsteps=['true']\n").unwrap();
        Self {
            root: std::fs::canonicalize(root).unwrap(),
            calls: Arc::default(),
        }
    }
    fn approve(&self) {
        let config = LoadedConfig::load(&self.root, None).unwrap();
        approve_trust(
            &self.root.join("trust.json"),
            config.repo_root(),
            "production",
            config.trust_snapshot("production").unwrap(),
        )
        .unwrap();
    }
    fn reader(&self, text: &str, failure: Option<TransportError>) -> Reader<FakeConnector> {
        Reader::with_connector(
            self.root.join("receipts"),
            self.root.join("trust.json"),
            FakeConnector {
                text: text.into(),
                calls: self.calls.clone(),
                failure,
            },
        )
    }
    fn arguments(&self) -> Value {
        json!({"checkout":self.root,"environment":"production"})
    }
    async fn exchange(
        &self,
        requests: Vec<Value>,
        text: &str,
        failure: Option<TransportError>,
        read_logs: bool,
    ) -> Vec<Value> {
        let lines = requests
            .iter()
            .map(|request| format!("{request}\n"))
            .collect::<String>();
        let mut output = Vec::new();
        mcp::serve(
            self.reader(text, failure),
            read_logs,
            lines.as_bytes(),
            &mut output,
        )
        .await
        .unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct FakeConnector {
    text: String,
    calls: Arc<AtomicUsize>,
    failure: Option<TransportError>,
}
impl Connector for FakeConnector {
    type Connection = FakeTransport;
    async fn connect(&self, target: &ApprovedTarget) -> Result<FakeTransport, TransportError> {
        assert_eq!(target.ssh_alias(), "app");
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error @ TransportError::Connect(_)) = &self.failure {
            return Err(error.clone());
        }
        Ok(FakeTransport {
            text: self.text.clone(),
            failure: self.failure.clone(),
        })
    }
}
struct FakeTransport {
    text: String,
    failure: Option<TransportError>,
}
impl Transport for FakeTransport {
    async fn run(
        &self,
        script: &str,
        output: Sender<String>,
        _limit: usize,
    ) -> Result<i32, TransportError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let reply = if script.contains("@time") {
            format!("@time 1790812800 1790726400 2026-09-30 00:00:00 2026-09-30 +0000 +0000\n@marker missing\n@reflog missing\n@file 1 {} 1790812800 1 2026-10-01 00:00:00 checksum {}\n{}\n@end", self.text.len(), STANDARD.encode("/srv/app/storage/logs/laravel.log"), STANDARD.encode("storage/logs/laravel.log"))
        } else {
            let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            gzip.write_all(self.text.as_bytes()).unwrap();
            format!("@data 0\n{}\n@end", STANDARD.encode(gzip.finish().unwrap()))
        };
        for line in reply.lines() {
            output.send(line.into()).await.unwrap();
        }
        Ok(0)
    }
    async fn reconnect(&self) -> Result<(), TransportError> {
        panic!("log reads never retry")
    }
}

fn call(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments,"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}})
}
fn body(response: &Value) -> &Value {
    let result = &response["result"];
    assert_eq!(
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
        result["structuredContent"]
    );
    &result["structuredContent"]
}

#[tokio::test]
async fn initialize_ping_and_eof_work_without_per_request_metadata() {
    let fixture = Fixture::new();
    let responses = fixture.exchange(vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
    ], "", None, false).await;
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2026-07-28");
    assert_eq!(responses[0]["result"]["capabilities"], json!({"tools":{}}));
    assert!(responses[1].get("error").is_none());
    assert_eq!(responses[2]["result"]["tools"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn unlisted_unknown_and_unapproved_targets_never_connect() {
    let fixture = Fixture::new();
    let response = fixture
        .exchange(vec![call("logs", fixture.arguments())], "", None, true)
        .await;
    assert_eq!(body(&response[0])["error"]["kind"], "invalid_input");
    fixture.approve();
    let mut unknown = fixture.arguments();
    unknown["environment"] = json!("missing");
    let response = fixture
        .exchange(vec![call("logs", unknown)], "", None, true)
        .await;
    assert_eq!(body(&response[0])["error"]["kind"], "invalid_input");
    let config = fixture.root.join(".shipslip.toml");
    std::fs::write(
        &config,
        std::fs::read_to_string(&config)
            .unwrap()
            .replace("ssh='app'", "ssh='changed-host'"),
    )
    .unwrap();
    for tool in ["logs", "log_group"] {
        let mut arguments = fixture.arguments();
        if tool == "log_group" {
            arguments["id"] = json!("1");
        }
        let response = fixture
            .exchange(vec![call(tool, arguments)], "", None, true)
            .await;
        assert_eq!(response[0]["result"]["isError"], true);
        assert_eq!(
            body(&response[0])["error"],
            json!({"kind":"not_approved","environment":"production"})
        );
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unapproved_environment_in_a_listed_checkout_never_connects() {
    let fixture = Fixture::new();
    let config = fixture.root.join(".shipslip.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(
        "[env.staging]\nssh='staging'\npath='/srv/staging'\nbranch='main'\nproduction=false\n",
    );
    std::fs::write(&config, text).unwrap();
    fixture.approve();
    for tool in ["logs", "log_group"] {
        let mut arguments = fixture.arguments();
        arguments["environment"] = json!("staging");
        if tool == "log_group" {
            arguments["id"] = json!("1");
        }
        let response = fixture
            .exchange(vec![call(tool, arguments)], "", None, true)
            .await;
        assert_eq!(response[0]["result"]["isError"], true);
        assert_eq!(
            body(&response[0])["error"],
            json!({"kind":"not_approved","environment":"staging"})
        );
    }
    std::fs::write(&config, "[project\n").unwrap();
    let response = fixture
        .exchange(vec![call("logs", fixture.arguments())], "", None, true)
        .await;
    assert_eq!(
        body(&response[0])["error"],
        json!({"kind":"not_approved","environment":"production"})
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_listed_checkout_that_is_not_the_repository_root_never_connects() {
    let fixture = Fixture::new();
    let checkout = fixture.root.join("nested");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::copy(
        fixture.root.join(".shipslip.toml"),
        checkout.join(".shipslip.toml"),
    )
    .unwrap();
    let config = LoadedConfig::load(&checkout, None).unwrap();
    // A stale approval can list a directory that is now inside another checkout.
    approve_trust(
        &fixture.root.join("trust.json"),
        &checkout,
        "production",
        config.trust_snapshot("production").unwrap(),
    )
    .unwrap();
    for tool in ["logs", "log_group"] {
        let mut arguments = fixture.arguments();
        arguments["checkout"] = json!(checkout);
        if tool == "log_group" {
            arguments["id"] = json!("1");
        }
        let response = fixture
            .exchange(vec![call(tool, arguments)], "", None, true)
            .await;
        assert_eq!(response[0]["result"]["isError"], true);
        assert_eq!(
            body(&response[0])["error"],
            json!({"kind":"invalid_input","message":"checkout must be the repository root"})
        );
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unreadable_release_map_never_connects() {
    let fixture = Fixture::new();
    fixture.approve();
    // A file in place of the receipts directory fails even when run as root.
    std::fs::write(fixture.root.join("receipts"), "not a directory").unwrap();
    for tool in ["logs", "log_group"] {
        let mut arguments = fixture.arguments();
        if tool == "log_group" {
            arguments["id"] = json!("1");
        }
        let response = fixture
            .exchange(vec![call(tool, arguments)], "", None, true)
            .await;
        assert_eq!(response[0]["result"]["isError"], true);
        let error = &body(&response[0])["error"];
        assert_eq!(error["kind"], "invalid_input");
        assert!(error["message"]
            .as_str()
            .unwrap()
            .starts_with("cannot read release map:"));
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn each_log_call_takes_a_fresh_snapshot_and_full_ids_cross_calls() {
    let fixture = Fixture::new();
    fixture.approve();
    let text = "[2026-10-01 00:00:00] production.ERROR: RuntimeException: failed at /srv/app/app/Job.php:17\n#0 /srv/app/app/Job.php(17): Job->handle()\n";
    let response = fixture
        .exchange(vec![call("logs", fixture.arguments())], text, None, true)
        .await;
    let summary = body(&response[0]);
    assert_eq!(response[0]["result"]["isError"], false);
    assert_eq!(summary["groups"].as_array().unwrap().len(), 1);
    let id = summary["groups"][0]["full_id"].clone();
    for query in [id.clone(), json!("1"), summary["groups"][0]["id"].clone()] {
        let mut arguments = fixture.arguments();
        arguments["id"] = query;
        let response = fixture
            .exchange(vec![call("log_group", arguments)], text, None, true)
            .await;
        assert_eq!(body(&response[0])["group"]["full_id"], id);
        assert_eq!(body(&response[0])["latest"]["frames"][0]["line"], 17);
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn long_entries_keep_head_tail_original_bytes_and_frames_with_a_raised_budget() {
    let fixture = Fixture::new();
    fixture.approve();
    let text = format!(
        "[2026-10-01 00:00:00] production.ERROR: RuntimeException: head {}\n#0 /srv/app/app/Job.php(17): Job->handle()\n{} tail",
        "é🙂".repeat(2000),
        "🙂é".repeat(2000),
    );
    for (budget, limit, clipped) in [(None, 16384, true), (Some(262144), 262144, false)] {
        let mut arguments = fixture.arguments();
        arguments["id"] = json!("1");
        if let Some(budget) = budget {
            arguments["max_text_bytes"] = json!(budget);
        }
        let responses = fixture
            .exchange(vec![call("log_group", arguments)], &text, None, true)
            .await;
        let detail = body(&responses[0]);
        assert_eq!(responses[0]["result"]["isError"], false);
        assert_eq!(detail["latest"]["bytes"], text.len());
        assert_eq!(detail["latest"]["clipped"], clipped);
        assert_eq!(detail["latest"]["truncated"], false);
        assert_eq!(
            detail["latest"]["frames"],
            json!([{"index":0,"file":"/srv/app/app/Job.php","line":17,
                "call":"Job->handle()","vendor":false,"chain":0}])
        );
        let returned = detail["latest"]["text"].as_str().unwrap();
        assert!(returned.len() <= limit);
        if clipped {
            assert!(
                returned.len() >= limit - 8,
                "only UTF-8 boundaries reduce the budget"
            );
            let (head, tail) = returned.split_once("\n… [clipped] …\n").unwrap();
            assert!(!head.is_empty() && !tail.is_empty());
            assert!(text.starts_with(head));
            assert!(text.ends_with(tail));
            assert!(
                !returned.contains("Job.php"),
                "frame was in the omitted text"
            );
        } else {
            assert_eq!(returned, text);
        }
        assert!(detail.get("text_fields").is_none());
        assert!(detail.get("clipped_fields").is_none());
    }
}

#[tokio::test]
async fn both_log_tools_budget_scalar_server_text_with_original_size_metadata() {
    let fixture = Fixture::new();
    fixture.approve();
    let filter = "filteré🙂".repeat(200);
    let text = format!("[2026-10-01 00:00:00] production.ERROR: RuntimeException: {filter}");
    for tool in ["logs", "log_group"] {
        let mut arguments = fixture.arguments();
        arguments["max_text_bytes"] = json!(1024);
        arguments["text"] = json!(filter);
        if tool == "log_group" {
            arguments["id"] = json!("1");
        }
        let responses = fixture
            .exchange(vec![call(tool, arguments)], &text, None, true)
            .await;
        let record = body(&responses[0]);
        assert_eq!(responses[0]["result"]["isError"], false);
        let returned = record["filter"]["text"].as_str().unwrap();
        assert!(returned.len() <= 1024);
        let (head, tail) = returned.split_once("\n… [clipped] …\n").unwrap();
        assert!(!head.is_empty() && !tail.is_empty());
        assert!(filter.starts_with(head));
        assert!(filter.ends_with(tail));
        let metadata = record["clipped_fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["path"] == "/filter/text")
            .unwrap();
        assert_eq!(
            metadata,
            &json!({"path":"/filter/text","bytes":filter.len()})
        );
        assert!(record["filter"].get("clipped").is_none());
        assert!(record.get("text_fields").is_none());
    }
}

#[tokio::test]
async fn same_request_keeps_the_nearest_five_entries_per_side_and_clips_their_text() {
    let fixture = Fixture::new();
    fixture.approve();
    let mut text = String::new();
    for index in 0..8 {
        text.push_str(&format!("[2026-10-01 00:00:0{index}] production.INFO: before {} {{\"request_id\":\"request-one\"}}\n", "é".repeat(1200)));
    }
    text.push_str("[2026-10-01 00:02:00] production.ERROR: CapException: chosen {\"request_id\":\"request-one\"}\n");
    for index in 0..8 {
        text.push_str(&format!("[2026-10-01 00:03:0{index}] production.INFO: after {} {{\"request_id\":\"request-one\"}}\n", "🙂".repeat(1200)));
    }
    let mut arguments = fixture.arguments();
    arguments["id"] = json!("1");
    arguments["max_text_bytes"] = json!(1024);
    let responses = fixture
        .exchange(vec![call("log_group", arguments)], &text, None, true)
        .await;
    let detail = body(&responses[0]);
    assert_eq!(responses[0]["result"]["isError"], false);
    assert_eq!(detail["same_request"]["kind"], "shared");
    assert_eq!(detail["same_request"]["more"], 6);
    let before = detail["same_request"]["before"].as_array().unwrap();
    let after = detail["same_request"]["after"].as_array().unwrap();
    assert_eq!(before.len(), 5);
    assert_eq!(after.len(), 5);
    assert_eq!(before[0]["time"], "2026-10-01 00:00:03");
    assert_eq!(before[4]["time"], "2026-10-01 00:00:07");
    assert_eq!(after[0]["time"], "2026-10-01 00:03:00");
    assert_eq!(after[4]["time"], "2026-10-01 00:03:04");
    for entry in before.iter().chain(after) {
        let returned = entry["text"].as_str().unwrap();
        let time = entry["time"].as_str().unwrap();
        let original = text.lines().find(|line| line.contains(time)).unwrap();
        assert_eq!(entry["clipped"], true);
        assert_eq!(entry["bytes"], original.len());
        assert!(returned.len() <= 1024);
        let (head, tail) = returned.split_once("\n… [clipped] …\n").unwrap();
        assert!(original.starts_with(head));
        assert!(original.ends_with(tail));
    }
    assert!(detail.get("clipped_fields").is_none());
}

#[tokio::test]
async fn both_log_tools_reject_invalid_text_budgets_before_connecting() {
    let fixture = Fixture::new();
    fixture.approve();
    for tool in ["logs", "log_group"] {
        for budget in [
            json!(-1),
            json!(0),
            json!(1023),
            json!(262145),
            json!(u64::MAX),
            json!("4096"),
            json!(1.5),
            json!(true),
            Value::Null,
        ] {
            let mut arguments = fixture.arguments();
            arguments["max_text_bytes"] = budget;
            if tool == "log_group" {
                arguments["id"] = json!("1");
            }
            let response = fixture
                .exchange(vec![call(tool, arguments)], "", None, true)
                .await;
            assert_eq!(response[0]["error"]["code"], -32602);
            assert!(response[0].get("result").is_none());
        }
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn both_log_tools_budget_error_messages_and_report_the_original_bytes() {
    let fixture = Fixture::new();
    fixture.approve();
    let message = format!("connection lost: {} tail", "é🙂".repeat(4000));
    for tool in ["logs", "log_group"] {
        let mut arguments = fixture.arguments();
        arguments["max_text_bytes"] = json!(1024);
        if tool == "log_group" {
            arguments["id"] = json!("1");
        }
        let responses = fixture
            .exchange(
                vec![call(tool, arguments)],
                "",
                Some(TransportError::ConnectionLost(message.clone())),
                true,
            )
            .await;
        let record = body(&responses[0]);
        assert_eq!(responses[0]["result"]["isError"], true);
        assert_eq!(record["error"]["kind"], "connection_lost");
        let returned = record["error"]["message"].as_str().unwrap();
        assert!(returned.len() <= 1024);
        let (head, tail) = returned.split_once("\n… [clipped] …\n").unwrap();
        assert!(message.starts_with(head));
        assert!(message.ends_with(tail));
        assert_eq!(
            record["clipped_fields"],
            json!([{"path":"/error/message", "bytes":message.len()}])
        );
        assert!(record.get("text_fields").is_none());
    }
}

#[tokio::test]
async fn dropped_connections_are_tool_errors_without_reconnection() {
    let fixture = Fixture::new();
    fixture.approve();
    let response = fixture
        .exchange(
            vec![call("logs", fixture.arguments())],
            "",
            Some(TransportError::ConnectionLost("dropped".into())),
            true,
        )
        .await;
    assert_eq!(response[0]["result"]["isError"], true);
    assert_eq!(
        body(&response[0])["error"],
        json!({"kind":"connection_lost","message":"dropped"})
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ssh_refusals_are_tool_errors_with_a_reason() {
    let fixture = Fixture::new();
    fixture.approve();
    let message = "Host key verification failed.";
    for tool in ["logs", "log_group"] {
        let mut arguments = fixture.arguments();
        if tool == "log_group" {
            arguments["id"] = json!("1");
        }
        let response = fixture
            .exchange(
                vec![call(tool, arguments)],
                "",
                Some(TransportError::Connect(message.into())),
                true,
            )
            .await;
        assert_eq!(response[0]["result"]["isError"], true);
        assert_eq!(
            body(&response[0])["error"],
            json!({"kind":"ssh_refused","reason":"host_key_unknown","message":message})
        );
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unrecognized_samples_obey_the_default_and_requested_text_budget() {
    let fixture = Fixture::new();
    fixture.approve();
    let text = "unrecognized worker output\n".repeat(1000);
    for (budget, expected_size, clipped) in [(None, 16384, true), (Some(262144), text.len(), false)]
    {
        let mut arguments = fixture.arguments();
        if let Some(budget) = budget {
            arguments["max_text_bytes"] = json!(budget);
        }
        let response = fixture
            .exchange(vec![call("logs", arguments)], &text, None, true)
            .await;
        let sample = &body(&response[0])["unrecognized_samples"][0];
        assert!(body(&response[0]).get("clipped_fields").is_none());
        assert_eq!(sample["bytes"], text.len());
        assert_eq!(sample["clipped"], clipped);
        assert_eq!(sample["text"].as_str().unwrap().len(), expected_size);
        let returned = sample["text"].as_str().unwrap();
        if clipped {
            let (head, tail) = returned.split_once("\n… [clipped] …\n").unwrap();
            assert!(!head.is_empty() && !tail.is_empty());
            assert!(text.starts_with(head));
            assert!(text.ends_with(tail));
        } else {
            assert_eq!(returned, text);
        }
    }
    let response = fixture
        .exchange(vec![call("logs", fixture.arguments())], &text, None, false)
        .await;
    assert_eq!(response[0]["error"]["code"], -32602);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn log_records_match_the_cli_builders_serialized_json() {
    let fixture = Fixture::new();
    fixture.approve();
    let text = "[2026-10-01 00:00:00] production.ERROR: RuntimeException: failed at /srv/app/app/Job.php:17\n";
    let config = LoadedConfig::load(&fixture.root, None).unwrap();
    let snapshot = shipslip::logs::snapshot(
        &FakeTransport {
            text: text.into(),
            failure: None,
        },
        &config.target("production").unwrap(),
        &shipslip::logs::Request::default(),
    )
    .await
    .unwrap();
    let groups = shipslip::logs::group(&snapshot, shipslip::logs::Level::Error, None);
    let expected = serde_json::to_value(shipslip::records::versioned(
        shipslip::records::logs_summary(&snapshot, &groups, shipslip::logs::Level::Error, None),
    ))
    .unwrap();
    let response = fixture
        .exchange(vec![call("logs", fixture.arguments())], text, None, true)
        .await;
    assert_eq!(*body(&response[0]), expected);
    let mut arguments = fixture.arguments();
    arguments["id"] = json!("1");
    let expected = serde_json::to_value(shipslip::records::versioned(
        shipslip::records::logs_detail(
            &snapshot,
            &groups.groups[0],
            shipslip::logs::Level::Error,
            None,
        ),
    ))
    .unwrap();
    let response = fixture
        .exchange(vec![call("log_group", arguments)], text, None, true)
        .await;
    assert_eq!(*body(&response[0]), expected);
}
