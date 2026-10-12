//! Receipts and trust files in a temp dir, written the way `slip` writes them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};
use shipslip::config::{approve_trust, TrustSnapshot};

pub const OLD: &str = "1111111111111111111111111111111111111111";
pub const NEW: &str = "2222222222222222222222222222222222222222";

/// `<base>/receipts` and `<base>/trust.json`, removed on drop.
pub struct Fixture(PathBuf);

impl Fixture {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "shipslip-app-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("receipts")).unwrap();
        Self(path)
    }

    /// A directory that exists, for receipts whose checkout must be on disk.
    pub fn dir(&self) -> &Path {
        &self.0
    }

    pub fn receipts(&self) -> PathBuf {
        self.0.join("receipts")
    }

    pub fn trust(&self) -> PathBuf {
        self.0.join("trust.json")
    }

    /// Approves the target [`run`] deploys to.
    pub fn approve(&self, repo_root: &str, env: &str) {
        let snapshot = TrustSnapshot {
            ssh_alias: "web1".into(),
            path: "/srv/app".into(),
            ..Default::default()
        };
        approve_trust(&self.trust(), Path::new(repo_root), env, snapshot).unwrap();
    }

    /// Saves a receipt under its project and env. Names must be plain
    /// (letters, digits, `-`, `_`): the library encodes anything else.
    pub fn save(&self, receipt: &Value) -> PathBuf {
        let dir = self
            .receipts()
            .join(receipt["project"].as_str().unwrap())
            .join(receipt["target"]["env"].as_str().unwrap());
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "{}-{}.json",
            receipt["started_at_ms"],
            receipt["run_id"].as_str().unwrap()
        ));
        fs::write(&path, serde_json::to_vec(receipt).unwrap()).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A finished, successful deploy of [`NEW`] from [`OLD`] to `env` of project
/// `app` at `/work/app`.
pub fn run(n: u64, env: &str) -> Value {
    json!({
        "version": 1, "project": "app", "repo_root": "/work/app",
        "run_id": format!("{n:08x}-0"), "owner_pid": 1, "confirmed": true,
        "started_at_ms": 1000 * n, "finished_at_ms": 1000 * n + 500,
        "status": "Final", "phase": "Finished",
        "target": {
            "env": env, "production": false, "ssh_alias": "web1", "path": "/srv/app",
            "branch": "main", "steps": ["php artisan migrate --force"],
            "maintenance": false
        },
        "run_plan": "Deploy", "from_sha": OLD, "target_sha": NEW, "commits": [],
        "recipe_hash": "x",
        "steps": [
            {"index": 0, "command": "git merge --ff-only", "status": "Ok", "exit_code": 0, "output": []},
            {"index": 1, "command": "php artisan migrate --force", "status": "Ok", "exit_code": 0, "output": []}
        ],
        "maintenance_down_done": false, "maintenance_up_done": false,
        "maintenance_down_status": null, "maintenance_down_exit_code": null,
        "maintenance_up_status": null, "maintenance_up_exit_code": null,
        "app_left_down": false, "mutation_started": true, "outcome": "Succeeded",
        "server_head_at_end": null, "tree_dirty": null, "last_message": null
    })
}

#[test]
fn library_reads_fixture_receipts() {
    let fx = Fixture::new();
    fx.approve("/work/app", "staging");
    fx.save(&run(1, "staging"));
    let overview = shipslip::receipt::overview(&fx.receipts(), &fx.trust()).unwrap();
    assert!(overview.problems.is_empty(), "{:?}", overview.problems);
    let cell = &overview.repos[0].envs[0];
    assert_eq!((cell.env.as_str(), cell.approved), ("staging", true));
    assert!(matches!(
        &cell.code,
        shipslip::receipt::CodeState::Recorded { sha, .. } if sha == NEW
    ));
}
