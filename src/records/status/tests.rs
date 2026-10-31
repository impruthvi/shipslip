use std::path::Path;

use serde_json::{json, Value};

use crate::receipt::tests::{receipt_json, write_receipt, TempRoot};
use crate::records::{status, versioned, versioned_error};

fn query(root: &TempRoot) -> Value {
    serde_json::to_value(versioned(
        status(&root.0, &root.0.join("trust.json")).unwrap(),
    ))
    .unwrap()
}

#[test]
fn an_incomplete_deploy_keeps_the_recorded_code_separate_from_its_outcome() {
    let root = TempRoot::new();
    let mut run = receipt_json("aaaa0001-0", 1000, "staging");
    run["outcome"] = json!({"Unknown": {"step": 2, "reason": "lost connection"}});
    run["steps"][2]["status"] = json!("Unknown");
    write_receipt(&root.0, &run);
    let result = query(&root);
    let env = &result["repos"][0]["envs"][0];
    assert_eq!(env["mark"], "incomplete");
    assert_eq!(
        env["code"],
        json!({
            "kind": "recorded", "sha": "2222222222222222222222222222222222222222",
            "complete": false, "evidence": "fast_forward", "run_id": "aaaa0001-0", "at_ms": 165000
        })
    );
    assert_eq!(
        env["last_attempt"]["badge"]["outcome"],
        json!({
            "kind": "unknown", "step": 2, "unfinished": false
        })
    );
    assert_eq!(env["needs_attention"], true);
}

#[test]
fn a_fast_forward_with_an_unknown_result_does_not_claim_the_target_code() {
    let root = TempRoot::new();
    let mut run = receipt_json("aaaa0001-0", 1000, "staging");
    run["outcome"] = json!({"Unknown": {"step": 0, "reason": "lost connection"}});
    run["steps"][0]["status"] = json!("Unknown");
    write_receipt(&root.0, &run);
    let result = query(&root);
    let env = &result["repos"][0]["envs"][0];
    assert_eq!(env["mark"], "not_known");
    assert_eq!(
        env["code"],
        json!({
            "kind": "not_known", "run_id": "aaaa0001-0",
            "reason": {"kind": "step_zero", "status": "Unknown"}
        })
    );
    assert_eq!(env["needs_attention"], true);
}

#[test]
fn observed_code_preserves_the_observation_and_structured_outcome() {
    let root = TempRoot::new();
    let mut run = receipt_json("aaaa0001-0", 1000, "staging");
    run["outcome"] = json!({"FailedAtStep": {"step": 0, "partial_update": true}});
    run["steps"][0]["status"] = json!("Failed");
    run["server_head_at_end"] = json!("3333333333333333333333333333333333333333");
    run["tree_dirty"] = json!(true);
    write_receipt(&root.0, &run);
    let result = query(&root);
    let env = &result["repos"][0]["envs"][0];
    assert_eq!(env["mark"], "observed");
    assert_eq!(
        env["code"],
        json!({
            "kind": "observed", "sha": "3333333333333333333333333333333333333333",
            "dirty": true, "expected": "2222222222222222222222222222222222222222",
            "run_id": "aaaa0001-0", "at_ms": 165000,
            "outcome": {"kind": "failed_at_step", "step": 0, "partial_update": true, "unfinished": false}
        })
    );
}

#[test]
fn an_approval_without_a_receipt_has_no_code_and_no_attempt() {
    let root = TempRoot::new();
    crate::config::approve_trust(
        &root.0.join("trust.json"),
        Path::new("/missing/checkout"),
        "staging",
        crate::config::TrustSnapshot {
            ssh_alias: "app-staging".into(),
            path: "/var/www/app".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let result = query(&root);
    let env = &result["repos"][0]["envs"][0];
    assert_eq!(
        env,
        &json!({
            "env": "staging", "approved": true, "approved_target": "app-staging:/var/www/app",
            "mark": "no_code", "code": {"kind": "never_deployed"},
            "last_attempt": null, "code_run": null, "stale_hint": null,
            "target": null, "newer_elsewhere": null, "needs_attention": false
        })
    );
}

#[test]
fn a_newer_unreadable_receipt_is_reported_without_discarding_older_evidence() {
    let root = TempRoot::new();
    write_receipt(&root.0, &receipt_json("aaaa0001-0", 1000, "staging"));
    let corrupt = write_receipt(&root.0, &receipt_json("bbbb0002-0", 2000, "staging"));
    std::fs::write(&corrupt, "not json").unwrap();
    let result = query(&root);
    let env = &result["repos"][0]["envs"][0];
    assert_eq!(env["mark"], "recorded");
    assert_eq!(env["last_attempt"]["run_id"], "aaaa0001-0");
    assert_eq!(
        env["stale_hint"],
        json!({"kind": "unattributed_newer", "path": corrupt})
    );
    assert_eq!(env["needs_attention"], true);
    assert_eq!(result["problems"][0]["kind"], "receipt");
    assert_eq!(result["problems"][0]["path"], json!(corrupt));
    assert_eq!(
        result["problems"][0]["cells"],
        json!([{"repo_root": "/repo", "env": "staging"}])
    );
}

#[test]
fn a_newer_receipt_version_is_attributed_but_cannot_replace_recorded_code() {
    let root = TempRoot::new();
    write_receipt(&root.0, &receipt_json("aaaa0001-0", 1000, "staging"));
    let mut newer = receipt_json("bbbb0002-0", 2000, "staging");
    newer["version"] = json!(99);
    let path = write_receipt(&root.0, &newer);
    let result = query(&root);
    let env = &result["repos"][0]["envs"][0];
    assert_eq!(env["mark"], "recorded");
    assert_eq!(env["last_attempt"]["run_id"], "aaaa0001-0");
    assert_eq!(
        env["stale_hint"],
        json!({"kind": "newest_unreadable", "path": path})
    );
    assert_eq!(result["problems"], json!([]));
}

#[test]
fn a_newer_run_elsewhere_is_a_fact_not_a_replacement_for_this_checkouts_code() {
    let root = TempRoot::new();
    write_receipt(&root.0, &receipt_json("aaaa0001-0", 1000, "staging"));
    let mut other = receipt_json("bbbb0002-0", 2000, "production");
    other["repo_root"] = json!("/other/checkout");
    other["target_sha"] = json!("3333333333333333333333333333333333333333");
    write_receipt(&root.0, &other);
    let result = query(&root);
    let repo = result["repos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|repo| repo["repo_root"] == "/repo")
        .unwrap();
    let env = &repo["envs"][0];
    assert_eq!(env["mark"], "recorded");
    assert_eq!(
        env["code"]["sha"],
        "2222222222222222222222222222222222222222"
    );
    assert_eq!(
        env["newer_elsewhere"],
        json!({
            "repo_root": "/other/checkout", "env": "production", "run_id": "bbbb0002-0", "started_at_ms": 2000
        })
    );
    assert_eq!(env["needs_attention"], true);
}

#[test]
fn status_text_is_raw_and_requires_a_frontend_to_choose_an_escape() {
    let root = TempRoot::new();
    let hostile = "a\u{1b}]0;x\u{7}`<b>\nc";
    let mut run = receipt_json("aaaa0001-0", 1000, hostile);
    run["project"] = json!(hostile);
    run["target"]["path"] = json!(hostile);
    write_receipt(&root.0, &run);
    let status = status(&root.0, &root.0.join("trust.json")).unwrap();
    assert_eq!(status.repos[0].label.raw(), hostile);
    assert_eq!(
        status.repos[0].envs[0].env.field(),
        "a\\x1b]0;x\\x07`<b>\\x0ac"
    );
    let result = serde_json::to_value(versioned(status)).unwrap();
    assert_eq!(result["repos"][0]["label"], hostile);
    assert_eq!(result["repos"][0]["envs"][0]["env"], hostile);
    assert_eq!(
        result["repos"][0]["envs"][0]["target"],
        format!("app-staging:{hostile}")
    );
}

#[test]
fn an_unreadable_receipts_root_is_a_versioned_error() {
    let root = TempRoot::new();
    let file = root.0.join("not-a-directory");
    std::fs::write(&file, "x").unwrap();
    let error = status(&file, &root.0.join("trust.json")).unwrap_err();
    let result = serde_json::to_value(versioned_error(error)).unwrap();
    assert_eq!(result["schema"], 1);
    assert_eq!(result["error"]["kind"], "unreadable");
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not-a-directory"));
}
