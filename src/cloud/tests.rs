use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::*;
use crate::config::{ApprovedCloudTarget, CloudTarget};
use crate::read_only::Reader;
use crate::receipt::tests::TempRoot;
use crate::records::ReceiptListRow;

const TOKEN: &str = "7|cloudSECRETtoken";

/// Answers each list page with a fixed body and records what was asked.
#[derive(Default)]
struct FakeCloud {
    pages: HashMap<u32, Result<String, CloudError>>,
    asked: Mutex<Vec<String>>,
}

impl FakeCloud {
    fn listing(deployments: Vec<Value>) -> Self {
        let mut fake = Self::default();
        fake.pages.insert(1, Ok(page(deployments)));
        fake
    }

    fn failing(error: CloudError) -> Self {
        let mut fake = Self::default();
        fake.pages.insert(1, Err(error));
        fake
    }
}

impl CloudApi for FakeCloud {
    async fn list_deployments(&self, environment: &str, page: u32) -> Result<String, CloudError> {
        self.asked
            .lock()
            .unwrap()
            .push(format!("list {environment} {page}"));
        self.pages
            .get(&page)
            .cloned()
            .unwrap_or(Err(CloudError::NotFound))
    }

    async fn get_deployment(&self, deployment: &str) -> Result<String, CloudError> {
        self.asked.lock().unwrap().push(format!("get {deployment}"));
        Err(CloudError::NotFound)
    }

    async fn deployment_logs(&self, deployment: &str) -> Result<String, CloudError> {
        self.asked
            .lock()
            .unwrap()
            .push(format!("logs {deployment}"));
        Err(CloudError::NotFound)
    }
}

fn page(deployments: Vec<Value>) -> String {
    json!({
        "data": deployments,
        "links": {"first": null, "last": null, "prev": null, "next": null},
        "meta": {"current_page": 1, "from": 1, "last_page": 1, "links": [], "path": null,
                 "per_page": 100, "to": 1, "total": 1}
    })
    .to_string()
}

/// A deployment as the documented `DeploymentResource` shape has it.
fn deployment(id: &str, minute: u32, status: &str) -> Value {
    json!({
        "id": id,
        "type": "deployments",
        "attributes": {
            "slug": format!("slug-{id}"),
            "status": status,
            "branch_name": "main",
            "commit_hash": format!("{minute:0>40}"),
            "commit_message": "Ship the \u{1b}[31mthing\n<b>now</b>",
            "commit_author": "Taylor",
            "failure_reason": null,
            "php_major_version": "8.4",
            "build_command": "composer install",
            "node_version": "22",
            "uses_octane": false,
            "uses_hibernation": false,
            "hibernation_wake_up_interval": null,
            "started_at": format!("2026-11-16T09:{minute:02}:00.000000Z"),
            "finished_at": format!("2026-11-16T09:{minute:02}:42.000000Z")
        },
        "relationships": {"environment": {"data": {"type": "environments", "id": "env-9f2c41d0"}}},
        "links": {"self": {"href": format!("https://cloud.laravel.com/api/deployments/{id}")}}
    })
}

fn target() -> ApprovedCloudTarget {
    ApprovedCloudTarget::for_test(
        CloudTarget {
            env: "production".into(),
            cloud_environment: "env-9f2c41d0".into(),
            production: true,
        },
        "app",
    )
}

async fn pull_with(
    root: &TempRoot,
    api: &FakeCloud,
) -> (Result<PullReport, PullError>, Vec<PullEvent>) {
    let mut events = Vec::new();
    let result = pull(&target(), api, &root.0, |event| events.push(event)).await;
    (result, events)
}

fn listed(root: &TempRoot) -> Vec<ReceiptListRow> {
    Reader::new(&root.0, root.0.join("trust.json"))
        .receipts("app", None, Some(100))
        .unwrap()
        .rows
}

fn cloud_rows(root: &TempRoot) -> Vec<Value> {
    listed(root)
        .into_iter()
        .map(|row| {
            assert!(matches!(row, ReceiptListRow::Cloud { .. }), "{row:?}");
            serde_json::to_value(row).unwrap()
        })
        .collect()
}

#[tokio::test]
async fn first_pull_records_the_ten_most_recent_succeeded_deployments() {
    let root = TempRoot::new();
    // Newest first, as Laravel Cloud lists them; twelve so two are left out.
    let deployments: Vec<Value> = (0..12)
        .map(|n| {
            let status = if n == 3 {
                "build.running"
            } else {
                "deployment.succeeded"
            };
            deployment(&format!("depl-{:02}", 40 - n), 40 - n, status)
        })
        .collect();
    let api = FakeCloud::listing(deployments);
    let (result, events) = pull_with(&root, &api).await;
    let report = result.unwrap();

    assert_eq!(report.written, 9);
    assert_eq!(*api.asked.lock().unwrap(), ["list env-9f2c41d0 1"]);
    assert!(events.contains(&PullEvent::Skipped {
        deployment_id: "depl-37".into(),
        cloud_status: "build.running".into(),
    }));

    let rows = cloud_rows(&root);
    let ids: Vec<&str> = rows
        .iter()
        .map(|row| row["receipt"]["deployment_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "depl-40", "depl-39", "depl-38", "depl-36", "depl-35", "depl-34", "depl-33", "depl-32",
            "depl-31"
        ]
    );
    let newest = &rows[0];
    assert_eq!(newest["state"], "cloud");
    assert_eq!(newest["id"], "depl-40");
    let receipt = &newest["receipt"];
    assert_eq!(receipt["env"], "production");
    assert_eq!(receipt["cloud_environment"], "env-9f2c41d0");
    assert_eq!(receipt["cloud_status"], "deployment.succeeded");
    assert_eq!(
        receipt["outcome"],
        json!({"kind": "succeeded", "unfinished": false})
    );
    assert_eq!(receipt["branch"], "main");
    assert_eq!(receipt["commit_hash"], format!("{:0>40}", 40));
    assert_eq!(
        receipt["commit_message"],
        "Ship the \u{1b}[31mthing\n<b>now</b>"
    );
    assert_eq!(receipt["commit_author"], "Taylor");
    assert_eq!(
        receipt["reported_started_at"],
        "2026-11-16T09:40:00.000000Z"
    );
    assert_eq!(
        receipt["reported_finished_at"],
        "2026-11-16T09:40:42.000000Z"
    );
    assert_eq!(
        receipt["reported_started_at_ms"],
        json!(crate::receipt::cloud::parse_time_ms("2026-11-16T09:40:00Z").unwrap())
    );
    assert!(receipt["pulled_at_ms"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn cloud_receipts_list_beside_run_receipts_by_start_time() {
    let root = TempRoot::new();
    let mut run = crate::receipt::tests::receipt_json("aaaa0001-0", 0, "staging");
    let late = crate::receipt::cloud::parse_time_ms("2026-11-16T09:20:00Z").unwrap();
    run["started_at_ms"] = json!(late);
    crate::receipt::tests::write_receipt(&root.0, &run);
    let api = FakeCloud::listing(vec![
        deployment("depl-30", 30, "deployment.succeeded"),
        deployment("depl-10", 10, "deployment.succeeded"),
    ]);
    pull_with(&root, &api).await.0.unwrap();

    let order: Vec<String> = listed(&root)
        .into_iter()
        .map(|row| match row {
            ReceiptListRow::Cloud { receipt, .. } => receipt.deployment_id.raw().to_string(),
            ReceiptListRow::Receipt { receipt, .. } => receipt.run_id.raw().to_string(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(order, ["depl-30", "aaaa0001-0", "depl-10"]);
}

#[tokio::test]
async fn a_held_deployment_is_not_written_twice() {
    let root = TempRoot::new();
    let api = FakeCloud::listing(vec![deployment("depl-1", 1, "deployment.succeeded")]);
    pull_with(&root, &api).await.0.unwrap();
    let (result, events) = pull_with(&root, &api).await;
    assert_eq!(result.unwrap().written, 0);
    assert!(events.contains(&PullEvent::AlreadyHeld {
        deployment_id: "depl-1".into()
    }));
    assert_eq!(cloud_rows(&root).len(), 1);
}

#[tokio::test]
async fn each_receipt_is_a_whole_file_with_no_temp_left_behind() {
    let root = TempRoot::new();
    let api = FakeCloud::listing(vec![deployment("depl-1", 1, "deployment.succeeded")]);
    pull_with(&root, &api).await.0.unwrap();
    let dir = root.0.join("app/production");
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].ends_with("-depl-1.json"), "{names:?}");
    let written: Value =
        serde_json::from_slice(&std::fs::read(dir.join(&names[0])).unwrap()).unwrap();
    assert_eq!(written["kind"], "cloud");
    assert_eq!(written["version"], 3);
}

#[tokio::test]
async fn api_errors_end_the_pull_without_writing() {
    for error in [
        CloudError::TokenRejected,
        CloudError::PermissionMissing,
        CloudError::NotFound,
        CloudError::RateLimited,
        CloudError::CouldNotConnect,
        CloudError::ConnectionLost,
    ] {
        let root = TempRoot::new();
        let (result, _) = pull_with(&root, &FakeCloud::failing(error.clone())).await;
        assert!(
            matches!(result, Err(PullError::Cloud(ref got)) if *got == error),
            "{result:?}"
        );
        assert!(listed(&root).is_empty());
    }
}

#[tokio::test]
async fn a_response_that_cannot_be_read_writes_nothing() {
    let mut bad_id = deployment("depl-1", 1, "deployment.succeeded");
    bad_id["id"] = json!("../../escape");
    let mut missing = deployment("depl-2", 2, "deployment.succeeded");
    missing["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("commit_hash");
    for body in [
        "not json".to_string(),
        "{\"data\": 3}".to_string(),
        page(vec![bad_id]),
        page(vec![
            deployment("depl-3", 3, "deployment.succeeded"),
            missing,
        ]),
    ] {
        let root = TempRoot::new();
        let mut api = FakeCloud::default();
        api.pages.insert(1, Ok(body.clone()));
        let (result, _) = pull_with(&root, &api).await;
        assert!(
            matches!(result, Err(PullError::Cloud(CloudError::Unreadable))),
            "{body}: {result:?}"
        );
        assert!(listed(&root).is_empty(), "{body}");
    }
}

#[tokio::test]
async fn the_token_appears_in_no_receipt_event_or_error() {
    let root = TempRoot::new();
    let token = CloudToken::new(TOKEN.into()).unwrap();
    assert_eq!(format!("{token:?}"), "CloudToken([REDACTED])");
    let api = FakeCloud::listing(vec![deployment("depl-1", 1, "deployment.succeeded")]);
    let (result, events) = pull_with(&root, &api).await;
    let mut seen = format!("{result:?}{events:?}");
    for error in [CloudError::TokenRejected, CloudError::Unreadable] {
        seen.push_str(&PullError::Cloud(error).to_string());
    }
    for entry in walk(&root.0) {
        seen.push_str(&String::from_utf8_lossy(&std::fs::read(entry).unwrap()));
    }
    assert!(!seen.contains(TOKEN));
    assert!(!seen.contains("cloudSECRET"));
}

#[test]
fn tokens_must_be_one_printable_line() {
    assert!(CloudToken::new("12|abcDEF_-.~+/=".into()).is_ok());
    for bad in ["", " ", "a b", "a\nb", "a\"b", "a\\b", "é"] {
        assert!(CloudToken::new(bad.into()).is_err(), "{bad:?}");
    }
    assert!(CloudToken::new("a".repeat(4097)).is_err());
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}
