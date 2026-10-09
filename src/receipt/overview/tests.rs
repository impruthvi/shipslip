use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

use super::*;
use crate::git::{Comparison, Git};
use crate::receipt::Flag;

const OLD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";
const OTHER: &str = "3333333333333333333333333333333333333333";

/// `<base>/receipts` and `<base>/trust.json`, removed on drop.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "shipslip-overview-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("receipts")).unwrap();
        Self(path)
    }

    fn receipts(&self) -> PathBuf {
        self.0.join("receipts")
    }

    fn trust(&self) -> PathBuf {
        self.0.join("trust.json")
    }

    fn approve(&self, repo_root: &str, env: &str) {
        crate::config::approve_trust(
            &self.trust(),
            Path::new(repo_root),
            env,
            crate::config::TrustSnapshot::default(),
        )
        .unwrap();
    }

    /// Saves under the project directory the receipt's own project maps to.
    fn save(&self, value: &Value) -> PathBuf {
        let dir = self
            .receipts()
            .join(safe_component(value["project"].as_str().unwrap()))
            .join(safe_component(value["target"]["env"].as_str().unwrap()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "{}-{}.json",
            value["started_at_ms"],
            value["run_id"].as_str().unwrap()
        ));
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }

    fn overview(&self) -> Overview {
        overview(&self.receipts(), &self.trust()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A finished, successful deploy of `NEW` from `OLD`.
fn run(n: u64, env: &str) -> Value {
    json!({
        "version": 1, "project": "app", "repo_root": "/work/app",
        "run_id": format!("{n:08x}-0"), "owner_pid": 1, "confirmed": true,
        "started_at_ms": 1000 * n, "finished_at_ms": 1000 * n + 500,
        "status": "Final", "phase": "Finished",
        "target": {
            "env": env, "production": false, "ssh_alias": "web1", "path": "/srv/app",
            "branch": "main", "steps": ["php artisan migrate --force", "php artisan optimize"],
            "maintenance": false
        },
        "run_plan": "Deploy", "from_sha": OLD, "target_sha": NEW, "commits": [],
        "recipe_hash": "x",
        "steps": [
            {"index": 0, "command": "git merge --ff-only", "status": "Ok", "exit_code": 0, "output": []},
            {"index": 1, "command": "php artisan migrate --force", "status": "Ok", "exit_code": 0, "output": []},
            {"index": 2, "command": "php artisan optimize", "status": "Ok", "exit_code": 0, "output": []}
        ],
        "maintenance_down_done": false, "maintenance_up_done": false,
        "maintenance_down_status": null, "maintenance_down_exit_code": null,
        "maintenance_up_status": null, "maintenance_up_exit_code": null,
        "app_left_down": false, "mutation_started": true, "outcome": "Succeeded",
        "server_head_at_end": null, "tree_dirty": null, "last_message": null
    })
}

/// A run that stopped before any step: nothing moved.
fn cancelled(n: u64, env: &str) -> Value {
    let mut value = run(n, env);
    value["outcome"] = json!("CancelledBeforeChanges");
    value["mutation_started"] = json!(false);
    for step in value["steps"].as_array_mut().unwrap() {
        step["status"] = json!("NotStarted");
    }
    value
}

fn cell<'a>(overview: &'a Overview, repo_root: &str, env: &str) -> &'a EnvCell {
    overview
        .repos
        .iter()
        .find(|row| row.repo_root == repo_root)
        .unwrap_or_else(|| panic!("no row for {repo_root}"))
        .envs
        .iter()
        .find(|cell| cell.env == env)
        .unwrap_or_else(|| panic!("no {env} cell for {repo_root}"))
}

fn recorded(sha: &str, complete: bool, evidence: Evidence, n: u64) -> CodeState {
    CodeState::Recorded {
        sha: sha.into(),
        complete,
        evidence,
        run_id: format!("{n:08x}-0"),
        at_ms: 1000 * u128::from(n) + 500,
    }
}

#[test]
fn fast_forward_status_decides_the_code() {
    let fixture = Fixture::new();
    fixture.save(&run(1, "staging"));
    let mut failed_later = run(2, "staging");
    failed_later["target_sha"] = json!(OTHER);
    failed_later["outcome"] = json!({"FailedAtStep": {"step": 2, "partial_update": false}});
    failed_later["steps"][2]["status"] = json!("Failed");
    fixture.save(&failed_later);

    let overview = fixture.overview();
    assert!(overview.problems.is_empty(), "{:?}", overview.problems);
    let staging = cell(&overview, "/work/app", "staging");
    assert_eq!(
        staging.code,
        recorded(OTHER, false, Evidence::FastForward, 2)
    );
    assert_eq!(staging.target.as_deref(), Some("web1:/srv/app"));
    let last = staging.last_attempt.as_ref().unwrap();
    assert_eq!(last.run_id, "00000002-0");
    assert_eq!(
        last.next_steps[0],
        "Step 2 failed and later steps did not run."
    );
    assert!(!staging.approved);
}

#[test]
fn runs_that_changed_nothing_keep_the_older_code() {
    let fixture = Fixture::new();
    fixture.save(&run(1, "staging"));
    let mut maintenance = run(2, "staging");
    maintenance["target_sha"] = json!(OTHER);
    maintenance["outcome"] = json!({"StoppedInMaintenance": "LockLost"});
    maintenance["app_left_down"] = json!(true);
    for step in maintenance["steps"].as_array_mut().unwrap() {
        step["status"] = json!("NotStarted");
    }
    fixture.save(&maintenance);
    fixture.save(&cancelled(3, "staging"));

    let overview = fixture.overview();
    let staging = cell(&overview, "/work/app", "staging");
    assert_eq!(staging.code, recorded(NEW, true, Evidence::FastForward, 1));
    let last = staging.last_attempt.as_ref().unwrap();
    assert_eq!(last.run_id, "00000003-0");
}

#[test]
fn maintenance_stop_is_the_last_attempt_with_its_flag() {
    let fixture = Fixture::new();
    fixture.save(&run(1, "staging"));
    let mut maintenance = run(2, "staging");
    maintenance["outcome"] = json!({"StoppedInMaintenance": "LockLost"});
    maintenance["app_left_down"] = json!(true);
    for step in maintenance["steps"].as_array_mut().unwrap() {
        step["status"] = json!("NotStarted");
    }
    fixture.save(&maintenance);

    let overview = fixture.overview();
    let staging = cell(&overview, "/work/app", "staging");
    assert_eq!(staging.code, recorded(NEW, true, Evidence::FastForward, 1));
    let badge = &staging.last_attempt.as_ref().unwrap().badge;
    assert!(badge.flags.contains(&Flag::AppLeftDown), "{badge:?}");
}

#[test]
fn crash_after_the_fast_forward_is_incomplete_and_unfinished() {
    let fixture = Fixture::new();
    let mut crashed = run(1, "staging");
    crashed["status"] = json!("InProgress");
    crashed["outcome"] = Value::Null;
    crashed["finished_at_ms"] = Value::Null;
    crashed["steps"][1]["status"] = json!("Running");
    crashed["steps"][2]["status"] = json!("Pending");
    fixture.save(&crashed);

    let overview = fixture.overview();
    let staging = cell(&overview, "/work/app", "staging");
    assert_eq!(
        staging.code,
        CodeState::Recorded {
            sha: NEW.into(),
            complete: false,
            evidence: Evidence::FastForward,
            run_id: "00000001-0".into(),
            at_ms: 1000,
        }
    );
    assert!(staging.last_attempt.as_ref().unwrap().badge.unfinished);
}

#[test]
fn a_fast_forward_that_did_not_end_ok_is_not_known_unless_observed() {
    let fixture = Fixture::new();
    fixture.save(&run(1, "staging"));
    let mut failed = run(2, "staging");
    failed["target_sha"] = json!(OTHER);
    failed["outcome"] = json!({"FailedAtStep": {"step": 0, "partial_update": false}});
    failed["steps"][0]["status"] = json!("Failed");
    let path = fixture.save(&failed);
    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        CodeState::NotKnown {
            run_id: Some("00000002-0".into()),
            reason: NotKnownReason::StepZero(ReceiptStepStatus::Failed),
        }
    );

    failed["server_head_at_end"] = json!(NEW);
    failed["tree_dirty"] = json!(true);
    fs::write(&path, serde_json::to_vec(&failed).unwrap()).unwrap();
    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        CodeState::Observed {
            sha: NEW.into(),
            dirty: Some(true),
            expected: OTHER.into(),
            run_id: "00000002-0".into(),
            outcome: Some(DeployOutcome::FailedAtStep {
                step: 0,
                partial_update: false,
            }),
            at_ms: 2500,
        }
    );

    let mut unknown = run(3, "staging");
    unknown["outcome"] = json!({"Unknown": {"step": 0, "reason": "connection lost"}});
    unknown["steps"][0]["status"] = json!("Unknown");
    fixture.save(&unknown);
    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        CodeState::NotKnown {
            run_id: Some("00000003-0".into()),
            reason: NotKnownReason::StepZero(ReceiptStepStatus::Unknown),
        }
    );
}

#[test]
fn an_observed_head_wins_over_a_successful_fast_forward() {
    let fixture = Fixture::new();
    let mut later_failure = run(1, "staging");
    later_failure["outcome"] = json!({"FailedAtStep": {"step": 2, "partial_update": false}});
    later_failure["steps"][2]["status"] = json!("Failed");
    later_failure["server_head_at_end"] = json!(OTHER);
    later_failure["tree_dirty"] = json!(false);
    fixture.save(&later_failure);

    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        CodeState::Observed {
            sha: OTHER.into(),
            dirty: Some(false),
            expected: NEW.into(),
            run_id: "00000001-0".into(),
            outcome: Some(DeployOutcome::FailedAtStep {
                step: 2,
                partial_update: false,
            }),
            at_ms: 1500,
        }
    );
}

#[test]
fn deploy_without_a_fast_forward_step_is_not_known() {
    let fixture = Fixture::new();
    let mut odd = run(1, "staging");
    odd["steps"].as_array_mut().unwrap().remove(0);
    fixture.save(&odd);

    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        CodeState::NotKnown {
            run_id: Some("00000001-0".into()),
            reason: NotKnownReason::NoStepZero,
        }
    );
}

#[test]
fn reruns_record_the_commit_checked_out_at_start() {
    let fixture = Fixture::new();
    let mut rerun = run(1, "staging");
    rerun["run_plan"] = json!("Rerun");
    rerun["steps"].as_array_mut().unwrap().remove(0);
    fixture.save(&rerun);
    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        recorded(NEW, true, Evidence::CheckedOutAtStart, 1)
    );

    let mut from_step = run(2, "staging");
    from_step["run_plan"] = json!({"FromStep": 2});
    from_step["steps"] = json!([
        {"index": 1, "command": "php artisan migrate --force", "status": "Skipped", "exit_code": null, "output": []},
        {"index": 2, "command": "php artisan optimize", "status": "Failed", "exit_code": 1, "output": []}
    ]);
    from_step["outcome"] = json!({"FailedAtStep": {"step": 2, "partial_update": false}});
    fixture.save(&from_step);
    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        recorded(NEW, false, Evidence::CheckedOutAtStart, 2)
    );

    // Refused before it started: preflight never confirmed the checkout.
    let mut refused = run(3, "staging");
    refused["run_plan"] = json!("Rerun");
    refused["target_sha"] = json!(OTHER);
    refused["mutation_started"] = json!(false);
    refused["outcome"] = json!("CancelledBeforeChanges");
    fixture.save(&refused);
    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        recorded(NEW, false, Evidence::CheckedOutAtStart, 2)
    );
}

#[test]
fn a_new_target_never_inherits_the_old_targets_code() {
    let fixture = Fixture::new();
    fixture.save(&run(1, "staging"));
    let mut moved = cancelled(2, "staging");
    moved["target"]["ssh_alias"] = json!("web2");
    fixture.save(&moved);

    let overview = fixture.overview();
    let staging = cell(&overview, "/work/app", "staging");
    assert_eq!(
        staging.code,
        CodeState::TargetChanged {
            run_id: "00000002-0".into(),
            target: "web2:/srv/app".into(),
        }
    );
    assert_eq!(staging.target.as_deref(), Some("web2:/srv/app"));
}

#[test]
fn walk_stops_after_twenty_runs_that_changed_nothing() {
    let fixture = Fixture::new();
    fixture.save(&run(1, "staging"));
    for n in 2..=22 {
        fixture.save(&cancelled(n, "staging"));
    }
    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        CodeState::NoCodeChangeInLast { records: 20 }
    );

    let fixture = Fixture::new();
    fixture.save(&cancelled(1, "staging"));
    assert_eq!(
        cell(&fixture.overview(), "/work/app", "staging").code,
        CodeState::NoCodeChangeInLast { records: 1 }
    );
}

#[test]
fn checkouts_sharing_a_project_directory_keep_their_own_cells() {
    let fixture = Fixture::new();
    let mut other = run(1, "staging");
    other["repo_root"] = json!("/old/app");
    other["target_sha"] = json!(OTHER);
    fixture.save(&other);
    for n in 2..=27 {
        fixture.save(&cancelled(n, "staging"));
    }
    fixture.save(&run(28, "staging"));

    let overview = fixture.overview();
    assert_eq!(overview.repos.len(), 2);
    assert_eq!(
        cell(&overview, "/old/app", "staging").code,
        recorded(OTHER, true, Evidence::FastForward, 1)
    );
    assert_eq!(
        cell(&overview, "/work/app", "staging").code,
        recorded(NEW, true, Evidence::FastForward, 28)
    );
    assert!(overview.repos.iter().all(|row| row.path_missing));
}

#[test]
fn unreadable_files_mark_cells_stale_and_are_never_skipped_quietly() {
    let fixture = Fixture::new();
    fixture.save(&run(1, "staging"));
    let mut other = run(2, "staging");
    other["repo_root"] = json!("/old/app");
    fixture.save(&other);
    let corrupt = fixture.receipts().join("app/staging/3000-00000003-0.json");
    fs::write(&corrupt, b"{ not json").unwrap();

    let overview = fixture.overview();
    assert_eq!(overview.problems.len(), 1);
    assert_eq!(overview.problems[0].path, corrupt);
    for repo_root in ["/work/app", "/old/app"] {
        let staging = cell(&overview, repo_root, "staging");
        assert!(matches!(staging.code, CodeState::Recorded { .. }));
        assert_eq!(
            staging.stale_hint,
            Some(StaleHint::UnattributedNewer {
                path: corrupt.clone()
            }),
            "{repo_root}"
        );
    }

    // A newer-version receipt is still attributed, but cannot decide.
    let mut newer = run(4, "staging");
    newer["version"] = json!(2);
    let newer = fixture.save(&newer);
    let staging = cell(&fixture.overview(), "/work/app", "staging").clone();
    assert_eq!(staging.code, recorded(NEW, true, Evidence::FastForward, 1));
    assert_eq!(
        staging.stale_hint,
        Some(StaleHint::NewestUnreadable { path: newer })
    );
}

#[test]
fn trust_only_checkouts_and_encoded_env_names_join_one_cell() {
    let fixture = Fixture::new();
    fixture.save(&run(1, "prod.eu"));
    fixture.approve("/work/app", "prod.eu");
    fixture.approve("/work/blog", "staging");

    let overview = fixture.overview();
    assert!(overview.problems.is_empty(), "{:?}", overview.problems);
    let app = overview
        .repos
        .iter()
        .find(|row| row.repo_root == "/work/app")
        .unwrap();
    assert_eq!(app.envs.len(), 1);
    assert_eq!(app.envs[0].env, "prod.eu");
    assert!(app.envs[0].approved);
    assert!(matches!(app.envs[0].code, CodeState::Recorded { .. }));

    let blog = overview
        .repos
        .iter()
        .find(|row| row.repo_root == "/work/blog")
        .unwrap();
    assert_eq!(blog.project, None);
    assert_eq!(blog.label, "blog");
    assert_eq!(blog.envs[0].code, CodeState::NeverDeployed);
    assert!(blog.envs[0].approved && blog.envs[0].last_attempt.is_none());
}

#[test]
fn missing_or_bad_inputs_become_problems_not_errors() {
    let fixture = Fixture::new();
    fixture.approve("/work/blog", "staging");
    fs::remove_dir_all(fixture.receipts()).unwrap();
    let overview = fixture.overview();
    assert_eq!(overview.repos.len(), 1);
    assert!(overview.problems.is_empty());

    fs::write(fixture.trust(), b"{ not json").unwrap();
    let overview = fixture.overview();
    assert!(overview.repos.is_empty());
    assert_eq!(overview.problems[0].path, fixture.trust());

    fs::write(fixture.trust(), br#"{"version": 2, "repositories": {}}"#).unwrap();
    assert_eq!(fixture.overview().problems.len(), 1);

    fs::remove_file(fixture.trust()).unwrap();
    assert_eq!(
        fixture.overview(),
        Overview {
            repos: vec![],
            problems: vec![]
        }
    );
}

#[cfg(unix)]
#[test]
fn unreadable_receipts_root_is_the_only_error() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fs::set_permissions(fixture.receipts(), fs::Permissions::from_mode(0o000)).unwrap();
    let result = overview(&fixture.receipts(), &fixture.trust());
    fs::set_permissions(fixture.receipts(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
}

#[test]
fn legacy_signature_history_is_not_a_project() {
    let fixture = Fixture::new();
    let legacy = fixture.receipts().join("signatures/app/staging");
    fs::create_dir_all(&legacy).unwrap();
    fs::write(legacy.join("history.json"), br#"["sig"]"#).unwrap();
    assert_eq!(
        fixture.overview(),
        Overview {
            repos: vec![],
            problems: vec![]
        }
    );

    // A real project named `signatures` shows only its own envs.
    let mut real = run(1, "production");
    real["project"] = json!("signatures");
    real["repo_root"] = json!("/work/signatures");
    fixture.save(&real);
    let overview = fixture.overview();
    assert!(overview.problems.is_empty(), "{:?}", overview.problems);
    assert_eq!(overview.repos.len(), 1);
    let envs: Vec<_> = overview.repos[0]
        .envs
        .iter()
        .map(|cell| cell.env.as_str())
        .collect();
    assert_eq!(envs, ["production"]);
}

#[cfg(unix)]
#[test]
fn lock_files_are_never_opened() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let path = fixture.save(&run(1, "staging"));
    let lock = path.with_extension("lock");
    fs::write(&lock, b"").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o000)).unwrap();

    let overview = fixture.overview();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(overview.problems.is_empty(), "{:?}", overview.problems);
    assert!(matches!(
        cell(&overview, "/work/app", "staging").code,
        CodeState::Recorded { .. }
    ));
}

#[test]
fn comparisons_carry_each_sides_caveat_and_refuse_unknown_code() {
    let fixture = Fixture::new();
    let cell = |env: &str, code: CodeState| EnvCell {
        env: env.into(),
        approved: false,
        code,
        last_attempt: None,
        stale_hint: None,
        target: None,
        newer_elsewhere: None,
    };
    let mut row = RepoRow {
        repo_root: fixture.0.to_string_lossy().into_owned(),
        project: Some("app".into()),
        label: "app".into(),
        path_missing: false,
        envs: vec![
            cell("staging", recorded(NEW, false, Evidence::FastForward, 1)),
            cell(
                "production",
                CodeState::Observed {
                    sha: NEW.into(),
                    dirty: Some(true),
                    expected: NEW.into(),
                    run_id: "00000002-0".into(),
                    outcome: None,
                    at_ms: 2500,
                },
            ),
            cell("prod.eu", recorded(NEW, true, Evidence::FastForward, 3)),
            cell(
                "qa",
                CodeState::NotKnown {
                    run_id: None,
                    reason: NotKnownReason::Unreadable,
                },
            ),
        ],
    };
    let git = Git::new(PathBuf::from("/nonexistent/git"));

    let both = compare_envs(&git, &row, "staging", "production");
    assert_eq!(both.result, Comparison::Same);
    assert_eq!(
        both.notes,
        [
            ("staging".to_string(), Qualifier::DeployIncomplete),
            ("production".to_string(), Qualifier::TreeDirty),
        ]
    );
    let one = compare_envs(&git, &row, "prod.eu", "staging");
    assert_eq!(
        one.notes,
        [("staging".to_string(), Qualifier::DeployIncomplete)]
    );
    assert!(compare_envs(&git, &row, "prod.eu", "prod.eu")
        .notes
        .is_empty());

    assert_eq!(
        compare_envs(&git, &row, "staging", "qa").result,
        Comparison::Unavailable("no recorded code on qa".into())
    );
    assert_eq!(
        compare_envs(&git, &row, "staging", "demo").result,
        Comparison::Unavailable("no demo environment".into())
    );
    row.path_missing = true;
    assert_eq!(
        compare_envs(&git, &row, "staging", "production").result,
        Comparison::Unavailable("checkout not found".into())
    );
}

#[test]
fn a_later_run_from_another_checkout_on_the_same_server_is_flagged() {
    let fixture = Fixture::new();
    let mut old = run(1, "staging");
    old["repo_root"] = json!("/old/app");
    fixture.save(&old);
    fixture.save(&run(2, "staging"));
    let mut elsewhere = run(3, "staging");
    elsewhere["repo_root"] = json!("/other/app");
    elsewhere["target"]["path"] = json!("/srv/other");
    fixture.save(&elsewhere);

    let overview = fixture.overview();
    assert_eq!(
        cell(&overview, "/old/app", "staging").newer_elsewhere,
        Some(NewerElsewhere {
            repo_root: "/work/app".into(),
            env: "staging".into(),
            run_id: "00000002-0".into(),
            started_at_ms: 2000,
        })
    );
    // The newest run on a server, and runs on other servers, are not flagged.
    assert_eq!(
        cell(&overview, "/work/app", "staging").newer_elsewhere,
        None
    );
    assert_eq!(
        cell(&overview, "/other/app", "staging").newer_elsewhere,
        None
    );
}
