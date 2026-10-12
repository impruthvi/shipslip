//! The commands the webview may call. Each runs its file and git work off
//! the UI thread. The UI sends only generations, positions and ids.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use shipslip::config::default_trust_path;
use shipslip::git::Git;
use shipslip::receipt::{default_receipts_root, overview, ReceiptError};
use tauri::async_runtime::spawn_blocking;

use crate::git_path;
use crate::state::{with_git, AppState, CompareResult, LoadResult, ReceiptResult};

pub struct App {
    state: AppState,
    /// The receipts root and trust file, or why they could not be found.
    paths: Result<(PathBuf, PathBuf), String>,
    git: Option<Git>,
}

impl App {
    pub fn from_env() -> Self {
        let paths = default_receipts_root()
            .map_err(|error| error.to_string())
            .and_then(|root| {
                default_trust_path()
                    .map(|trust| (root, trust))
                    .map_err(|error| error.to_string())
            });
        Self::new(paths, git_path::resolve().map(Git::new))
    }

    fn new(paths: Result<(PathBuf, PathBuf), String>, git: Option<Git>) -> Self {
        Self {
            state: AppState::default(),
            paths,
            git,
        }
    }

    fn load_map(&self) -> LoadResult {
        self.state.load_map(now_ms, || match &self.paths {
            Ok((root, trust)) => overview(root, trust),
            Err(reason) => Err(ReceiptError::Invalid(reason.clone())),
        })
    }

    fn compare(&self, generation: u64, row: usize, a: usize, b: usize) -> CompareResult {
        self.state
            .compare(generation, row, a, b, with_git(self.git.as_ref()))
    }

    fn load_receipt(&self, generation: u64, id: usize) -> ReceiptResult {
        self.state.load_receipt(generation, id)
    }
}

#[tauri::command]
pub async fn load_map(app: tauri::State<'_, Arc<App>>) -> Result<LoadResult, String> {
    off_thread(&app, App::load_map).await
}

#[tauri::command]
pub async fn compare(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
    a: usize,
    b: usize,
) -> Result<CompareResult, String> {
    off_thread(&app, move |app| app.compare(generation, row, a, b)).await
}

#[tauri::command]
pub async fn load_receipt(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    id: usize,
) -> Result<ReceiptResult, String> {
    off_thread(&app, move |app| app.load_receipt(generation, id)).await
}

async fn off_thread<T: Send + 'static>(
    app: &Arc<App>,
    work: impl FnOnce(&App) -> T + Send + 'static,
) -> Result<T, String> {
    let app = Arc::clone(app);
    spawn_blocking(move || work(&app))
        .await
        .map_err(|_| "the request failed inside the app".to_string())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::fixture::{run, Fixture};
    use crate::receipt_view::SlipView;

    fn app(fx: &Fixture, git: Option<Git>) -> App {
        App::new(Ok((fx.receipts(), fx.trust())), git)
    }

    #[test]
    fn map_compare_and_receipt_work_together() {
        let fx = Fixture::new();
        let root = fx.dir().to_string_lossy().into_owned();
        for (n, env) in [(1, "staging"), (2, "production")] {
            let mut receipt = run(n, env);
            receipt["repo_root"] = json!(root);
            fx.save(&receipt);
        }
        let app = app(&fx, None);
        let generation = match app.load_map() {
            LoadResult::Loaded(loaded) => loaded.generation,
            other => panic!("{other:?}"),
        };
        match app.compare(generation, 0, 1, 0) {
            CompareResult::Compared { comparison, .. } => {
                assert_eq!(comparison.text, "cannot compare: git not found")
            }
            other => panic!("{other:?}"),
        }
        match app.load_receipt(generation, 0) {
            ReceiptResult::Opened { slip, .. } => assert!(matches!(slip, SlipView::Receipt(_))),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn missing_home_is_a_load_error_not_a_crash() {
        let app = App::new(Err("HOME is not set".into()), None);
        match app.load_map() {
            LoadResult::Failed { message, current } => {
                assert_eq!(message, "invalid receipt: HOME is not set");
                assert!(current.is_none());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn real_git_compares_recorded_commits() {
        let Some(git) = git_path::resolve().map(Git::new) else {
            return;
        };
        let fx = Fixture::new();
        let repo = fx.dir().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let sh = |script: &str| {
            let status = std::process::Command::new("sh")
                .args(["-c", script])
                .current_dir(&repo)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .unwrap();
            assert!(status.success(), "{script}");
        };
        sh("git init -q && git -c user.name=t -c user.email=t@t commit -q --allow-empty -m one");
        sh("git -c user.name=t -c user.email=t@t commit -q --allow-empty -m two");
        let head = |rev: &str| {
            let out = std::process::Command::new("git")
                .args(["rev-parse", rev])
                .current_dir(&repo)
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        let root = repo.to_string_lossy().into_owned();
        for (n, env, sha) in [
            (1, "staging", head("HEAD")),
            (2, "production", head("HEAD~1")),
        ] {
            let mut receipt = run(n, env);
            receipt["repo_root"] = json!(root);
            receipt["target_sha"] = json!(sha);
            fx.save(&receipt);
        }
        let app = app(&fx, Some(git));
        let LoadResult::Loaded(loaded) = app.load_map() else {
            panic!("load failed");
        };
        match app.compare(loaded.generation, 0, 1, 0) {
            CompareResult::Compared { comparison, .. } => {
                assert_eq!(comparison.text, "staging is 1 commit ahead of production");
            }
            other => panic!("{other:?}"),
        }
    }

    /// Prints what the app would show for this Mac's real receipts:
    /// `cargo test dump_real_map -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads this Mac's receipts"]
    fn dump_real_map() {
        let app = App::from_env();
        let LoadResult::Loaded(loaded) = app.load_map() else {
            panic!("{:?}", app.load_map());
        };
        let print = |label: &str, value: &dyn erased::Json| {
            println!("== {label}\n{}", value.pretty());
        };
        print("map", &*loaded.map);
        let columns = loaded.map.envs.len();
        for row in 0..loaded.map.rows.len() {
            for a in 0..columns {
                for b in (0..columns).filter(|b| *b != a) {
                    print(
                        &format!("compare row {row}: {a} vs {b}"),
                        &app.compare(loaded.generation, row, a, b),
                    );
                }
            }
        }
        for id in 0.. {
            match app.load_receipt(loaded.generation, id) {
                ReceiptResult::NotFound => break,
                result => print(&format!("receipt {id}"), &result),
            }
        }
    }

    mod erased {
        pub trait Json {
            fn pretty(&self) -> String;
        }
        impl<T: serde::Serialize> Json for T {
            fn pretty(&self) -> String {
                serde_json::to_string_pretty(self).unwrap()
            }
        }
    }
}
