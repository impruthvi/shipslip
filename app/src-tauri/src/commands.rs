//! The commands the webview may call. Each runs its file and git work off
//! the UI thread. The UI sends only generations, positions and ids.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use shipslip::config::default_trust_path;
use shipslip::git::Git;
use shipslip::receipt::{default_receipts_root, overview, ReceiptError};
use tauri::async_runtime::spawn_blocking;

use crate::deploy_session::{
    CommandError, CurrentRuns, DeployReview, Ending, ErrorCode, OpenSsh, QuitRun, RecoveryReview,
    RunBatch, RunInfo, Sessions, TrustReview,
};
use crate::finish::{self, Unseen};
use crate::git_path;
use crate::state::SelectionError;
use crate::state::{with_git, AppState, CompareResult, LoadResult, ReceiptResult};

pub struct App {
    state: AppState,
    /// The receipts root and trust file, or why they could not be found.
    paths: Result<(PathBuf, PathBuf), String>,
    git: Option<Git>,
    sessions: Option<Arc<Sessions<OpenSsh>>>,
    unseen: Unseen,
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
        let sessions = paths
            .as_ref()
            .ok()
            .map(|(root, trust)| Sessions::new(root.clone(), trust.clone(), OpenSsh));
        Self {
            sessions,
            state: AppState::default(),
            paths,
            git,
            unseen: Unseen::default(),
        }
    }

    fn sessions(&self) -> Result<&Arc<Sessions<OpenSsh>>, CommandError> {
        self.sessions.as_ref().ok_or_else(|| {
            CommandError::new(
                ErrorCode::Local,
                self.paths
                    .as_ref()
                    .err()
                    .map(String::as_str)
                    .unwrap_or("App storage is unavailable."),
            )
        })
    }

    fn selection(&self, generation: u64, row: usize, env: &str) -> Result<PathBuf, CommandError> {
        self.state
            .resolve_target(generation, row, env)
            .map_err(|error| match error {
                SelectionError::Stale => CommandError::new(
                    ErrorCode::Stale,
                    "The map changed. Refresh it and select the environment again.",
                ),
                SelectionError::NotFound => CommandError::new(
                    ErrorCode::NotFound,
                    "This checkout or environment is no longer on the map.",
                ),
            })
    }

    /// Runs that would be cut short by quitting now.
    pub fn quit_runs(&self) -> Vec<QuitRun> {
        self.sessions
            .as_ref()
            .map(|sessions| sessions.quit_runs())
            .unwrap_or_default()
    }

    pub fn on_live_change(&self, observer: impl Fn(bool) + Send + Sync + 'static) {
        if let Some(sessions) = &self.sessions {
            sessions.set_live_observer(observer);
        }
    }

    pub fn on_finish(&self, observer: impl Fn(Ending) + Send + Sync + 'static) {
        if let Some(sessions) = &self.sessions {
            sessions.set_finish_observer(observer);
        }
    }

    pub fn unseen(&self) -> &Unseen {
        &self.unseen
    }

    pub fn invalidate_page(&self) {
        if let Some(sessions) = &self.sessions {
            sessions.invalidate_page();
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

#[tauri::command]
pub async fn prepare_deploy(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
    env: String,
    plan: shipslip::RunPlan,
) -> Result<DeployReview, CommandError> {
    let checkout = app.selection(generation, row, &env)?;
    app.sessions()?.prepare(checkout, env, plan).await
}

#[tauri::command]
pub async fn review_trust(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
    env: String,
) -> Result<TrustReview, CommandError> {
    let checkout = app.selection(generation, row, &env)?;
    app.sessions()?.review_trust(checkout, env).await
}

#[tauri::command]
pub async fn approve_trust(
    app: tauri::State<'_, Arc<App>>,
    handle: String,
    typed_env: String,
) -> Result<(), CommandError> {
    app.sessions()?.approve_trust(&handle, &typed_env).await
}

#[tauri::command]
pub async fn confirm_deploy(
    app: tauri::State<'_, Arc<App>>,
    handle: String,
    typed_env: Option<String>,
) -> Result<RunInfo, CommandError> {
    app.sessions()?.confirm(&handle, typed_env).await
}

#[tauri::command]
pub async fn cancel_deploy(
    app: tauri::State<'_, Arc<App>>,
    handle: String,
) -> Result<(), CommandError> {
    app.sessions()?.cancel(&handle).await
}

#[tauri::command]
pub async fn review_up(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
    env: String,
) -> Result<RecoveryReview, CommandError> {
    let checkout = app.selection(generation, row, &env)?;
    app.sessions()?.review_up(checkout, env).await
}

#[tauri::command]
pub async fn review_attach(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
    env: String,
) -> Result<RecoveryReview, CommandError> {
    let checkout = app.selection(generation, row, &env)?;
    app.sessions()?.review_attach(checkout, env).await
}

#[tauri::command]
pub async fn current_runs(app: tauri::State<'_, Arc<App>>) -> Result<CurrentRuns, CommandError> {
    Ok(app.sessions()?.current_runs())
}

/// Streams one run to this view. The view acknowledges each batch with `ack_run`
/// before the next is sent; the returned id names the subscription.
#[tauri::command]
pub fn subscribe_run(
    app: tauri::State<'_, Arc<App>>,
    handle: String,
    channel: tauri::ipc::Channel<RunBatch>,
) -> Result<String, CommandError> {
    app.sessions()?.subscribe_run(&handle, channel)
}

#[tauri::command]
pub fn ack_run(
    app: tauri::State<'_, Arc<App>>,
    handle: String,
    subscription: String,
    through: u64,
) -> Result<(), CommandError> {
    app.sessions()?.ack_run(&handle, &subscription, through)
}

#[tauri::command]
pub fn unsubscribe_run(
    app: tauri::State<'_, Arc<App>>,
    handle: String,
    subscription: String,
) -> Result<(), CommandError> {
    app.sessions()?.unsubscribe_run(&handle, &subscription)
}

/// The pane shows this run's receipt. A sync command runs on the main
/// thread, so it is ordered with the finish signal.
#[tauri::command]
pub fn receipt_seen(app: tauri::State<'_, Arc<App>>, window: tauri::WebviewWindow, run_id: String) {
    if let Some(show) = app.unseen.seen(&run_id, finish::focused(&window)) {
        finish::badge(&window, show);
    }
}

#[tauri::command]
pub fn quit_status(app: tauri::State<'_, Arc<App>>) -> Vec<QuitRun> {
    app.quit_runs()
}

/// Brings every live run to a safe point, waits for it, then exits.
#[tauri::command]
pub async fn detach_and_quit(
    app: tauri::State<'_, Arc<App>>,
    handle: tauri::AppHandle,
) -> Result<(), CommandError> {
    if let Some(sessions) = &app.sessions {
        sessions.begin_quit();
        sessions.wait_idle().await;
    }
    handle.exit(0);
    Ok(())
}

#[tauri::command]
pub fn quit_now(handle: tauri::AppHandle) {
    handle.exit(0);
}

/// Back or closing an operation abandons all pending reviews for this page.
/// Already-confirmed deployments keep their Rust owner.
#[tauri::command]
pub fn cancel_pending(app: tauri::State<'_, Arc<App>>) {
    app.invalidate_page();
}
