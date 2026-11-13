//! The commands the webview may call. Each runs its file and git work off
//! the UI thread. The UI sends only generations, positions and ids.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use shipslip::config::default_trust_path;
use shipslip::git::Git;
use shipslip::receipt::{default_receipts_root, ReceiptError};
use shipslip::records::status;
use tauri::async_runtime::spawn_blocking;
use tauri::Manager;

use crate::add_project::{self, Answers, ConfigAdded, ConfigText, FolderView};
use crate::connection_check::{self, CheckView, Lines};
use crate::deploy_session::{
    CommandError, CurrentRuns, DeployEntry, DeployReview, Ending, ErrorCode, LiveRuns, OpenSsh,
    PrepareProgress, QuitRun, RecoveryReview, RunBatch, RunInfo, RunStop, Sessions, TrustReview,
};
use crate::finish::{self, Unseen};
use crate::git_path;
use crate::receipt_changes::{self, ReceiptStamp};
use crate::ssh_alias::{self, AliasCheck, HostBlock, SshAliases};
use crate::state::SelectionError;
use crate::state::{
    with_git, AppState, CheckoutReceipts, CompareResult, LoadResult, ReceiptResult,
};
use shipslip::config::{InitAnswers, InitPlan};
use shipslip::projects::{self, RemoveError};
use shipslip::ssh_aliases::HostDraft;

fn selection_error(error: SelectionError) -> CommandError {
    match error {
        SelectionError::Stale => CommandError::new(
            ErrorCode::Stale,
            "The map was read again since this was selected.",
        ),
        SelectionError::NotFound => CommandError::new(
            ErrorCode::NotFound,
            "This checkout or environment is no longer on the map.",
        ),
    }
}

pub struct App {
    state: AppState,
    receipt_stamp: Mutex<Option<ReceiptStamp>>,
    /// The receipts root and trust file, or why they could not be found.
    paths: Result<(PathBuf, PathBuf), String>,
    git: Option<Git>,
    sessions: Option<Arc<Sessions<OpenSsh>>>,
    unseen: Unseen,
    /// The folder last picked in the Add project sheet; only it can be added.
    picked: Mutex<Option<PathBuf>>,
    /// The config last shown for approval; only it can be written.
    rendered: Mutex<Option<(InitPlan, InitAnswers)>>,
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
            receipt_stamp: Mutex::new(None),
            paths,
            git,
            unseen: Unseen::default(),
            picked: Mutex::new(None),
            rendered: Mutex::new(None),
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
            .map_err(selection_error)
    }

    /// The checkout whose config to open: found by one of its environments,
    /// or by the checkout alone when its config can't be read.
    fn config_checkout(
        &self,
        generation: u64,
        row: usize,
        env: Option<&str>,
    ) -> Result<PathBuf, CommandError> {
        match env {
            Some(env) => self.selection(generation, row, env),
            None => self
                .state
                .resolve_checkout(generation, row)
                .map_err(selection_error),
        }
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

    /// Whether exiting now could leave a preview's server lock behind.
    pub fn previews_open(&self) -> bool {
        self.sessions
            .as_ref()
            .is_some_and(|sessions| sessions.previews_open())
    }

    fn load_map(&self) -> LoadResult {
        self.state.load_map(now_ms, || match &self.paths {
            Ok((root, trust)) => {
                let stamp = receipt_changes::read(root).ok();
                let overview = status(root, trust)?;
                *self
                    .receipt_stamp
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = stamp;
                Ok(crate::state::with_checkout_configs(overview, trust))
            }
            Err(reason) => Err(ReceiptError::Invalid(reason.clone()).into()),
        })
    }

    fn receipts_changed(&self) -> Result<bool, String> {
        let (root, _) = self.paths.as_ref().map_err(Clone::clone)?;
        let stamp = receipt_changes::read(root).map_err(|error| error.to_string())?;
        let previous = self
            .receipt_stamp
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(previous.as_ref() != Some(&stamp)
            || self
                .state
                .journal_claims_changed()
                .map_err(|error| error.to_string())?)
    }

    fn compare(&self, generation: u64, row: usize, a: usize, b: usize) -> CompareResult {
        self.state
            .compare(generation, row, a, b, with_git(self.git.as_ref()))
    }

    fn load_receipt(&self, generation: u64, id: usize) -> ReceiptResult {
        self.state.load_receipt(generation, id)
    }

    fn trust_path(&self) -> Result<&Path, CommandError> {
        self.paths
            .as_ref()
            .map(|(_, trust)| trust.as_path())
            .map_err(|reason| CommandError::new(ErrorCode::Local, reason.as_str()))
    }

    /// Reads a folder picked in the Add project sheet and keeps it for
    /// [`Self::add_picked`]. `None` is a panel closed without a pick, which
    /// keeps the folder picked before, as the sheet does.
    fn inspect_picked(&self, folder: Option<PathBuf>) -> Result<FolderView, CommandError> {
        let trust = self.trust_path()?;
        let Some(folder) = folder else {
            return Ok(FolderView::Cancelled);
        };
        let view = add_project::inspect(&folder, trust, self.git.as_ref());
        *self.picked.lock().unwrap_or_else(PoisonError::into_inner) = Some(folder);
        *self.rendered.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Ok(view)
    }

    /// A "Setup not finished" checkout from the map, picked again.
    fn reopen_setup(&self, generation: u64, row: usize) -> Result<FolderView, CommandError> {
        let checkout = self
            .state
            .resolve_checkout(generation, row)
            .map_err(selection_error)?;
        self.inspect_picked(Some(checkout))
    }

    /// Drops a map row's checkout from the projects list when this Mac
    /// recorded nothing for it. Nothing in its folder is touched.
    fn remove_checkout(&self, generation: u64, row: usize) -> Result<(), CommandError> {
        let (root, trust) = self
            .paths
            .as_ref()
            .map_err(|reason| CommandError::new(ErrorCode::Local, reason.as_str()))?;
        let checkout = self
            .state
            .resolve_checkout(generation, row)
            .map_err(selection_error)?;
        projects::remove(trust, root, &checkout).map_err(|error| match error {
            RemoveError::Recorded(_) => CommandError::new(
                ErrorCode::Local,
                "This Mac has receipts or approvals for this checkout, so it stays in the sidebar.",
            ),
            other => CommandError::new(
                ErrorCode::Local,
                format!("Couldn't remove this checkout: {other}."),
            ),
        })
    }

    fn picked(&self) -> Result<PathBuf, CommandError> {
        self.picked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                CommandError::new(ErrorCode::NotFound, "Choose the project's folder first.")
            })
    }

    fn add_picked(&self) -> Result<String, CommandError> {
        let trust = self.trust_path()?;
        add_project::add(&self.picked()?, trust, now_ms())
            .map_err(|message| CommandError::new(ErrorCode::Local, message))
    }

    /// Renders the config for the picked folder and keeps it for
    /// [`Self::add_rendered`].
    fn render_config(&self, answers: Answers) -> Result<ConfigText, CommandError> {
        self.trust_path()?;
        let folder = self.picked()?;
        let answers = InitAnswers::from(answers);
        let mut rendered = self.rendered.lock().unwrap_or_else(PoisonError::into_inner);
        *rendered = None;
        let (view, plan) = add_project::render(&folder, &answers)
            .map_err(|message| CommandError::new(ErrorCode::Local, message))?;
        *rendered = plan.map(|plan| (plan, answers));
        Ok(view)
    }

    /// Writes the config last rendered, approves it and lists the checkout.
    /// A config once written is spent; one not written can be tried again.
    fn add_rendered(&self) -> Result<ConfigAdded, CommandError> {
        let trust = self.trust_path()?;
        let mut rendered = self.rendered.lock().unwrap_or_else(PoisonError::into_inner);
        let (plan, answers) = rendered.as_ref().ok_or_else(|| {
            CommandError::new(
                ErrorCode::NotFound,
                "Answer the environment questions first.",
            )
        })?;
        let added = add_project::add_with_config(plan, answers, trust, now_ms())
            .map_err(|message| CommandError::new(ErrorCode::Local, message))?;
        *rendered = None;
        Ok(added)
    }

    /// The connection check for the answers last rendered.
    fn rendered_check(&self) -> Result<shipslip::connection_check::Request, CommandError> {
        let rendered = self.rendered.lock().unwrap_or_else(PoisonError::into_inner);
        let (_, answers) = rendered.as_ref().ok_or_else(|| {
            CommandError::new(
                ErrorCode::NotFound,
                "Answer the environment questions first.",
            )
        })?;
        Ok(connection_check::request_for_answers(answers))
    }

    fn checkout_receipts(&self, generation: u64, row: usize) -> Result<CheckoutReceipts, String> {
        let (root, _) = self.paths.as_ref().map_err(Clone::clone)?;
        Ok(self
            .state
            .checkout_receipts(generation, row, root, now_ms()))
    }
}

#[tauri::command]
pub async fn load_map(app: tauri::State<'_, Arc<App>>) -> Result<LoadResult, String> {
    off_thread(&app, App::load_map).await
}

#[tauri::command]
pub async fn receipts_changed(app: tauri::State<'_, Arc<App>>) -> Result<bool, String> {
    off_thread(&app, App::receipts_changed).await?
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

#[tauri::command]
pub async fn checkout_receipts(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
) -> Result<CheckoutReceipts, String> {
    off_thread(&app, move |app| app.checkout_receipts(generation, row)).await?
}

/// Opens the macOS folder panel and reads the folder picked.
#[tauri::command]
pub async fn pick_folder(
    app: tauri::State<'_, Arc<App>>,
    handle: tauri::AppHandle,
) -> Result<FolderView, CommandError> {
    use tauri_plugin_dialog::DialogExt;
    let app = Arc::clone(&app);
    spawn_blocking(move || {
        let picked = handle
            .dialog()
            .file()
            .set_title("Choose the project's folder")
            .blocking_pick_folder()
            .and_then(|path| path.into_path().ok());
        app.inspect_picked(picked)
    })
    .await
    .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

/// Adds the picked folder's checkout to the projects list. Returns the
/// checkout's path as the map spells it.
#[tauri::command]
pub async fn add_checkout(app: tauri::State<'_, Arc<App>>) -> Result<String, CommandError> {
    let app = Arc::clone(&app);
    spawn_blocking(move || app.add_picked())
        .await
        .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

/// Opens the sheet on a "Setup not finished" checkout: picks its folder again.
#[tauri::command]
pub async fn reopen_setup(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
) -> Result<FolderView, CommandError> {
    let app = Arc::clone(&app);
    spawn_blocking(move || app.reopen_setup(generation, row))
        .await
        .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

/// Drops a checkout with nothing recorded from the sidebar.
#[tauri::command]
pub async fn remove_checkout(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
) -> Result<(), CommandError> {
    let app = Arc::clone(&app);
    spawn_blocking(move || app.remove_checkout(generation, row))
        .await
        .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

/// The `.shipslip.toml` the answers would write for the picked folder, or
/// the answers that break a rule.
#[tauri::command]
pub async fn render_config(
    app: tauri::State<'_, Arc<App>>,
    answers: Answers,
) -> Result<ConfigText, CommandError> {
    let app = Arc::clone(&app);
    spawn_blocking(move || app.render_config(answers))
        .await
        .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

/// "Approve and add" for a new config: writes the config last rendered,
/// approves it and lists the checkout.
#[tauri::command]
pub async fn add_with_config(app: tauri::State<'_, Arc<App>>) -> Result<ConfigAdded, CommandError> {
    let app = Arc::clone(&app);
    spawn_blocking(move || app.add_rendered())
        .await
        .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

/// The aliases in `~/.ssh/config` with what each resolves to, and the
/// `~/.ssh/id_*` keys. Reads only.
#[tauri::command]
pub async fn ssh_aliases() -> Result<SshAliases, CommandError> {
    spawn_blocking(|| {
        let dir = shipslip::ssh_aliases::ssh_dir()
            .ok_or_else(|| CommandError::new(ErrorCode::Local, "HOME is not set."))?;
        Ok(ssh_alias::aliases(&dir.join("config"), &dir, |alias| {
            shipslip::ssh_aliases::resolve(alias, None)
        }))
    })
    .await
    .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

/// The `Host` block the "None of these fit" fields make.
#[tauri::command]
pub fn ssh_host_block(draft: HostDraft) -> HostBlock {
    ssh_alias::host_block(&draft)
}

/// Check again: resolves the draft's alias with `ssh -G`.
#[tauri::command]
pub async fn check_ssh_alias(draft: HostDraft) -> Result<AliasCheck, CommandError> {
    spawn_blocking(move || {
        let listed = shipslip::ssh_aliases::user_config()
            .and_then(|config| shipslip::ssh_aliases::list(&config).ok())
            .unwrap_or_default();
        ssh_alias::check(&draft, &listed, |alias| {
            shipslip::ssh_aliases::resolve(alias, None)
        })
    })
    .await
    .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))
}

/// Runs the read-only connection check for one environment of a checkout
/// on the map, approved or not, sending the lines as each probe moves.
#[tauri::command]
pub async fn check_connection(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
    env: String,
    lines: tauri::ipc::Channel<CheckView>,
) -> Result<CheckView, CommandError> {
    let checkout = app.selection(generation, row, &env)?;
    let request = spawn_blocking(move || connection_check::request_for(&checkout, &env))
        .await
        .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
        .map_err(|message| CommandError::new(ErrorCode::Local, message))?;
    Ok(run_check(&request, &lines).await)
}

/// Runs the read-only connection check for the answers last rendered in the
/// Add project sheet, before anything is written or approved.
#[tauri::command]
pub async fn check_rendered(
    app: tauri::State<'_, Arc<App>>,
    lines: tauri::ipc::Channel<CheckView>,
) -> Result<CheckView, CommandError> {
    let request = app.rendered_check()?;
    Ok(run_check(&request, &lines).await)
}

async fn run_check(
    request: &shipslip::connection_check::Request,
    channel: &tauri::ipc::Channel<CheckView>,
) -> CheckView {
    let mut lines = Lines::new(request);
    let _ = channel.send(lines.view());
    shipslip::connection_check::run(
        &shipslip::connection_check::SystemSsh::default(),
        request,
        |event| {
            lines.apply(&event);
            let _ = channel.send(lines.view());
        },
    )
    .await;
    lines.view()
}

/// Opens `~/.ssh/config` in the default text editor for the user to paste
/// the block. Shipslip never creates or writes it.
#[tauri::command]
pub async fn open_ssh_config() -> Result<(), CommandError> {
    spawn_blocking(|| {
        let config = shipslip::ssh_aliases::user_config()
            .ok_or_else(|| CommandError::new(ErrorCode::Local, "HOME is not set."))?;
        open_ssh_config_with(&config, |path| {
            std::process::Command::new("/usr/bin/open")
                .arg("-t")
                .arg(path)
                .status()
                .is_ok_and(|status| status.success())
        })
    })
    .await
    .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

fn open_ssh_config_with(config: &Path, open: impl Fn(&Path) -> bool) -> Result<(), CommandError> {
    if !config.is_file() {
        return Err(CommandError::new(
            ErrorCode::NotFound,
            "~/.ssh/config does not exist yet. Create it in a text editor, paste the block, save, then check again.",
        ));
    }
    if open(config) {
        Ok(())
    } else {
        Err(CommandError::new(
            ErrorCode::Local,
            "Couldn't open ~/.ssh/config in a text editor.",
        ))
    }
}

/// Shows the checkout's folder in Finder.
#[tauri::command]
pub async fn open_folder(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
) -> Result<(), CommandError> {
    let checkout = app
        .state
        .resolve_checkout(generation, row)
        .map_err(selection_error)?;
    spawn_blocking(move || {
        open_folder_with(&checkout, |path| {
            std::process::Command::new("/usr/bin/open")
                .arg(path)
                .status()
                .is_ok_and(|status| status.success())
        })
    })
    .await
    .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

fn open_folder_with(checkout: &Path, open: impl Fn(&Path) -> bool) -> Result<(), CommandError> {
    if !checkout.is_dir() {
        return Err(CommandError::new(
            ErrorCode::NotFound,
            "This checkout's folder is no longer on this Mac.",
        ));
    }
    if open(checkout) {
        Ok(())
    } else {
        Err(CommandError::new(
            ErrorCode::Local,
            format!("Couldn't open {} in Finder.", checkout.display()),
        ))
    }
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
    fn receipt_changes_are_compared_with_the_last_map_read() {
        let fx = Fixture::new();
        let app = app(&fx, None);
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        assert!(!app.receipts_changed().unwrap());

        fx.save(&run(1, "production"));
        assert!(app.receipts_changed().unwrap());
        assert!(app.receipts_changed().unwrap());
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        assert!(!app.receipts_changed().unwrap());
    }

    #[test]
    fn receipt_changes_notice_a_terminal_exit_without_a_journal_write() {
        let fx = Fixture::new();
        let mut saved = run(1, "production");
        saved["status"] = json!("InProgress");
        saved["outcome"] = serde_json::Value::Null;
        let path = fx.save(&saved);
        let journal = shipslip::receipt::ReceiptJournal::load(&path).unwrap();
        journal.claim().unwrap();
        let app = app(&fx, None);
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        assert!(!app.receipts_changed().unwrap());
        let before = std::fs::read(&path).unwrap();

        drop(journal);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(app.receipts_changed().unwrap());
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        assert!(!app.receipts_changed().unwrap());

        let journal = shipslip::receipt::ReceiptJournal::load(&path).unwrap();
        journal.claim().unwrap();
        assert!(app.receipts_changed().unwrap());
    }

    #[test]
    fn receipt_changes_find_atomic_journal_updates_and_deletions_in_existing_environments() {
        let fx = Fixture::new();
        let path = fx.save(&run(1, "production"));
        let directory = std::fs::File::open(path.parent().unwrap()).unwrap();
        let old_times =
            std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(10));
        directory.set_times(old_times).unwrap();
        let app = app(&fx, None);
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        assert!(!app.receipts_changed().unwrap());

        let replacement = path.with_extension("tmp");
        std::fs::copy(&path, &replacement).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert!(app.receipts_changed().unwrap());
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        assert!(!app.receipts_changed().unwrap());

        directory.set_times(old_times).unwrap();
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        std::fs::remove_file(path).unwrap();
        assert!(app.receipts_changed().unwrap());
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        assert!(!app.receipts_changed().unwrap());
    }

    #[test]
    fn receipt_changes_find_new_environments_and_removed_projects() {
        let fx = Fixture::new();
        fx.save(&run(1, "production"));
        let app = app(&fx, None);
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));

        fx.save(&run(2, "staging"));
        assert!(app.receipts_changed().unwrap());
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        assert!(!app.receipts_changed().unwrap());

        std::fs::remove_dir_all(fx.receipts()).unwrap();
        assert!(app.receipts_changed().unwrap());
    }

    #[test]
    fn receipt_check_errors_do_not_replace_the_last_successful_map_read() {
        let fx = Fixture::new();
        let app = app(&fx, None);
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));

        std::fs::remove_dir_all(fx.receipts()).unwrap();
        std::fs::write(fx.receipts(), "not a directory").unwrap();
        assert!(app.receipts_changed().is_err());
        assert!(matches!(
            app.load_map(),
            LoadResult::Failed {
                current: Some(_),
                ..
            }
        ));

        std::fs::remove_file(fx.receipts()).unwrap();
        fx.save(&run(1, "production"));
        assert!(app.receipts_changed().unwrap());
        assert!(matches!(app.load_map(), LoadResult::Loaded(_)));
        assert!(!app.receipts_changed().unwrap());
    }

    #[test]
    fn open_config_tries_the_toml_app_then_the_text_editor() {
        let fx = Fixture::new();
        let path = fx.dir().join(".shipslip.toml");
        std::fs::write(&path, "[project]\n").unwrap();
        let tried = std::sync::Mutex::new(Vec::new());
        let result = open_config_with(&path, |args, opened| {
            assert_eq!(opened, path);
            tried.lock().unwrap().push(args.join(" "));
            !args.is_empty()
        });
        assert!(result.is_ok());
        assert_eq!(*tried.lock().unwrap(), ["", "-t"]);

        let error = open_config_with(&path, |_, _| false).unwrap_err();
        assert_eq!(error.code, ErrorCode::Local);
        assert!(error.message.contains(".shipslip.toml"));
    }

    #[test]
    fn open_config_refuses_a_checkout_without_a_config_and_opens_nothing() {
        let fx = Fixture::new();
        let error = open_config_with(&fx.dir().join(".shipslip.toml"), |_, _| {
            panic!("nothing to open")
        })
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn open_ssh_config_opens_the_file_and_never_creates_it() {
        let fx = Fixture::new();
        let config = fx.dir().join("config");
        let error = open_ssh_config_with(&config, |_| panic!("nothing to open")).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(!config.exists());

        std::fs::write(&config, "Host web1\n").unwrap();
        assert!(open_ssh_config_with(&config, |path| path == config).is_ok());
        let error = open_ssh_config_with(&config, |_| false).unwrap_err();
        assert_eq!(error.code, ErrorCode::Local);
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "Host web1\n");
    }

    #[test]
    fn a_config_that_cannot_be_read_is_found_by_its_checkout_alone() {
        let fx = Fixture::new();
        let root = fx.checkout(
            "blog",
            "[project]\nname = \"blog\"\nstack = \"laravel\"\n\
             [env.staging]\nssh = \"web1\"\npath = \"/srv/blog\"\nbranch = \"main\"\nproduction = false\n",
        );
        crate::add_project::add(&root, &fx.trust(), 0).unwrap();
        std::fs::write(root.join(".shipslip.toml"), "[project\n").unwrap();
        let app = app(&fx, None);
        let LoadResult::Loaded(loaded) = app.load_map() else {
            panic!("not loaded");
        };
        assert!(loaded.map.rows[0].config_unreadable.is_some());
        assert_eq!(
            app.config_checkout(loaded.generation, 0, None).unwrap(),
            root
        );
        let error = app
            .config_checkout(loaded.generation, 0, Some("staging"))
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound, "no cell names it");
        let error = app
            .config_checkout(loaded.generation + 1, 0, None)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Stale);
    }

    #[test]
    fn open_folder_opens_the_checkout_and_says_when_it_cannot() {
        let fx = Fixture::new();
        assert!(open_folder_with(fx.dir(), |path| path == fx.dir()).is_ok());
        let error = open_folder_with(fx.dir(), |_| false).unwrap_err();
        assert_eq!(error.code, ErrorCode::Local);
        let error =
            open_folder_with(&fx.dir().join("gone"), |_| panic!("nothing to open")).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn checkout_receipts_need_the_receipts_folder() {
        let app = App::new(Err("no home folder".into()), None);
        assert_eq!(app.checkout_receipts(1, 0).unwrap_err(), "no home folder");
    }

    #[test]
    fn only_the_folder_picked_last_is_added_and_a_closed_panel_keeps_it() {
        let fx = Fixture::new();
        let app = app(&fx, None);
        let error = app.add_picked().unwrap_err();
        assert_eq!(error.message, "Choose the project's folder first.");

        let crm = fx.checkout(
            "crm",
            "[project]\nname = \"crm\"\nstack = \"laravel\"\n\
             [env.staging]\nssh = \"web1\"\npath = \"/srv/crm\"\nbranch = \"main\"\nproduction = false\n",
        );
        assert!(matches!(
            app.inspect_picked(Some(crm.clone())).unwrap(),
            FolderView::Config { .. }
        ));
        assert!(matches!(
            app.inspect_picked(None).unwrap(),
            FolderView::Cancelled
        ));
        assert_eq!(app.add_picked().unwrap(), crm.to_string_lossy());

        let loose = fx.dir().join("loose");
        std::fs::create_dir_all(&loose).unwrap();
        app.inspect_picked(Some(loose)).unwrap();
        assert!(app
            .add_picked()
            .unwrap_err()
            .message
            .contains("not inside a git repository"));
    }

    fn answers(env: &str) -> Answers {
        serde_json::from_value(serde_json::json!({
            "env": env, "sshAlias": "web1", "path": "/srv/crm", "branch": "main",
            "production": false, "maintenance": true, "smokeUrl": "",
        }))
        .unwrap()
    }

    #[test]
    fn only_the_config_rendered_last_is_written_and_only_once() {
        let fx = Fixture::new();
        let app = app(&fx, None);
        let crm = fx.dir().join("crm");
        std::fs::create_dir_all(crm.join(".git")).unwrap();
        let crm = std::fs::canonicalize(crm).unwrap();
        assert_eq!(
            app.add_rendered().unwrap_err().message,
            "Answer the environment questions first."
        );
        assert!(matches!(
            app.inspect_picked(Some(crm.clone())).unwrap(),
            FolderView::NewConfig { .. }
        ));

        app.render_config(answers("staging")).unwrap();
        assert!(matches!(
            app.render_config(answers("bad name")).unwrap(),
            ConfigText::Invalid { .. }
        ));
        assert!(app.add_rendered().is_err(), "invalid answers keep nothing");

        app.render_config(answers("qa")).unwrap();
        // Picking a folder again drops the config shown for the last one.
        app.inspect_picked(Some(crm.clone())).unwrap();
        assert!(app.add_rendered().is_err());

        app.render_config(answers("qa")).unwrap();
        let added = app.add_rendered().unwrap();
        assert_eq!(added.checkout, crm.to_string_lossy());
        let written = std::fs::read_to_string(crm.join(".shipslip.toml")).unwrap();
        assert!(written.contains("[env.qa]"), "{written}");
        assert!(app.add_rendered().is_err(), "spent");
    }

    #[test]
    fn the_check_for_the_add_sheet_uses_the_answers_last_rendered() {
        let fx = Fixture::new();
        let app = app(&fx, None);
        let crm = fx.dir().join("crm");
        std::fs::create_dir_all(crm.join(".git")).unwrap();
        assert_eq!(
            app.rendered_check().unwrap_err().message,
            "Answer the environment questions first."
        );
        app.inspect_picked(Some(std::fs::canonicalize(crm).unwrap()))
            .unwrap();
        app.render_config(answers("qa")).unwrap();
        let request = app.rendered_check().unwrap();
        assert_eq!(
            (
                request.alias.as_str(),
                request.path.as_str(),
                request.branch.as_str()
            ),
            ("web1", "/srv/crm", "main")
        );
        assert_eq!(request.log_path, "storage/logs/laravel.log");
    }

    #[test]
    fn reopen_setup_picks_the_map_rows_checkout() {
        let fx = Fixture::new();
        let app = app(&fx, None);
        let crm = fx.dir().join("crm");
        std::fs::create_dir_all(crm.join(".git")).unwrap();
        let crm = std::fs::canonicalize(crm).unwrap();
        add_project::add(&crm, &fx.trust(), 1).unwrap();
        let LoadResult::Loaded(loaded) = app.load_map() else {
            panic!("map");
        };
        assert!(loaded.map.rows[0].setup_not_finished);

        let view = app.reopen_setup(loaded.generation, 0).unwrap();
        assert!(matches!(view, FolderView::NewConfig { .. }), "{view:?}");
        assert_eq!(app.add_picked().unwrap(), crm.to_string_lossy());
        assert_eq!(
            app.reopen_setup(loaded.generation + 1, 0).unwrap_err().code,
            ErrorCode::Stale
        );
    }

    #[test]
    fn remove_checkout_drops_a_row_with_nothing_recorded_and_leaves_its_folder() {
        let fx = Fixture::new();
        let app = app(&fx, None);
        let crm = fx.dir().join("crm");
        std::fs::create_dir_all(crm.join(".git")).unwrap();
        std::fs::write(crm.join("README.md"), "crm\n").unwrap();
        let crm = std::fs::canonicalize(crm).unwrap();
        add_project::add(&crm, &fx.trust(), 1).unwrap();
        let LoadResult::Loaded(loaded) = app.load_map() else {
            panic!("map");
        };
        assert!(loaded.map.rows[0].removable);
        assert_eq!(
            app.remove_checkout(loaded.generation + 1, 0)
                .unwrap_err()
                .code,
            ErrorCode::Stale
        );

        app.remove_checkout(loaded.generation, 0).unwrap();

        let LoadResult::Loaded(after) = app.load_map() else {
            panic!("map");
        };
        assert!(after.map.rows.is_empty());
        assert_eq!(
            std::fs::read_to_string(crm.join("README.md")).unwrap(),
            "crm\n"
        );
    }

    #[test]
    fn remove_checkout_refuses_a_row_with_anything_recorded() {
        let fx = Fixture::new();
        let app = app(&fx, None);
        let crm = fx.dir().join("crm");
        std::fs::create_dir_all(crm.join(".git")).unwrap();
        let crm = std::fs::canonicalize(crm).unwrap();
        add_project::add(&crm, &fx.trust(), 1).unwrap();
        fx.approve(&crm.to_string_lossy(), "staging");
        let LoadResult::Loaded(loaded) = app.load_map() else {
            panic!("map");
        };
        assert!(!loaded.map.rows[0].removable);

        let error = app.remove_checkout(loaded.generation, 0).unwrap_err();

        assert_eq!(
            error.message,
            "This Mac has receipts or approvals for this checkout, so it stays in the sidebar."
        );
        let LoadResult::Loaded(after) = app.load_map() else {
            panic!("map");
        };
        assert_eq!(after.map.rows.len(), 1);
    }

    #[test]
    fn remove_checkout_says_why_when_what_was_recorded_cannot_be_read() {
        let fx = Fixture::new();
        let app = app(&fx, None);
        let crm = fx.dir().join("crm");
        std::fs::create_dir_all(crm.join(".git")).unwrap();
        let crm = std::fs::canonicalize(crm).unwrap();
        add_project::add(&crm, &fx.trust(), 1).unwrap();
        let LoadResult::Loaded(loaded) = app.load_map() else {
            panic!("map");
        };
        std::fs::write(fx.trust(), "not json").unwrap();

        let error = app.remove_checkout(loaded.generation, 0).unwrap_err();

        assert!(
            error.message.starts_with(
                "Couldn't remove this checkout: could not read what this Mac recorded"
            ),
            "{}",
            error.message
        );
        assert_eq!(
            projects::list(&projects::path_beside(&fx.trust()))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn add_project_needs_app_storage() {
        let app = App::new(Err("no home folder".into()), None);
        assert_eq!(
            app.inspect_picked(None).unwrap_err().message,
            "no home folder"
        );
        assert_eq!(app.add_picked().unwrap_err().message, "no home folder");
        assert_eq!(
            app.remove_checkout(1, 0).unwrap_err().message,
            "no home folder"
        );
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
        assert_eq!(app.receipts_changed().unwrap_err(), "HOME is not set");
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
    progress: tauri::ipc::Channel<PrepareProgress>,
) -> Result<DeployReview, CommandError> {
    let checkout = app.selection(generation, row, &env)?;
    app.sessions()?
        .prepare_with_progress(checkout, env, plan, move |phase| {
            let _ = progress.send(phase);
        })
        .await
}

#[tauri::command]
pub async fn deploy_entry(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
    env: String,
) -> Result<DeployEntry, CommandError> {
    let checkout = app.selection(generation, row, &env)?;
    Ok(app.sessions()?.deploy_entry(checkout, env).await)
}

/// Opens the checkout's `.shipslip.toml` in the user's editor. A change still
/// has to pass the trust review before any deploy uses it. Without `env`,
/// the checkout alone finds it, e.g. when the config can't be read.
#[tauri::command]
pub async fn open_config(
    app: tauri::State<'_, Arc<App>>,
    generation: u64,
    row: usize,
    env: Option<String>,
) -> Result<(), CommandError> {
    let checkout = app.config_checkout(generation, row, env.as_deref())?;
    spawn_blocking(move || {
        open_config_with(&checkout.join(".shipslip.toml"), |args, path| {
            std::process::Command::new("/usr/bin/open")
                .args(args)
                .arg(path)
                .status()
                .is_ok_and(|status| status.success())
        })
    })
    .await
    .map_err(|e| CommandError::new(ErrorCode::Local, e.to_string()))?
}

/// The app set for `.toml` files, else the default text editor.
fn open_config_with(
    path: &Path,
    open: impl Fn(&[&str], &Path) -> bool,
) -> Result<(), CommandError> {
    if !path.is_file() {
        return Err(CommandError::new(
            ErrorCode::NotFound,
            "This checkout has no .shipslip.toml.",
        ));
    }
    if open(&[], path) || open(&["-t"], path) {
        Ok(())
    } else {
        Err(CommandError::new(
            ErrorCode::Local,
            format!("Couldn't open {} in an editor.", path.display()),
        ))
    }
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

/// Live runs for the map's cells and toolbar, polled while the window is open.
#[tauri::command]
pub fn live_runs(app: tauri::State<'_, Arc<App>>) -> Result<LiveRuns, CommandError> {
    Ok(app.sessions()?.live_runs())
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
pub fn stop_run(
    app: tauri::State<'_, Arc<App>>,
    handle: String,
    stop: RunStop,
) -> Result<(), CommandError> {
    app.sessions()?.stop_run(&handle, stop)
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
    exit(&handle);
    Ok(())
}

/// How long exiting waits for open previews to release their server locks.
/// A lock left behind goes stale and `slip break-lock` clears it.
const RELEASE_LIMIT: Duration = Duration::from_secs(5);

/// Exits once every open preview has released its server lock, or after
/// `RELEASE_LIMIT`.
pub fn exit(handle: &tauri::AppHandle) {
    let app = handle.state::<Arc<App>>().inner().clone();
    let Some(sessions) = app.sessions.clone().filter(|_| app.previews_open()) else {
        handle.exit(0);
        return;
    };
    let late = handle.clone();
    std::thread::spawn(move || {
        std::thread::sleep(RELEASE_LIMIT);
        late.exit(0);
    });
    let done = handle.clone();
    tauri::async_runtime::spawn(async move {
        sessions.close_previews().await;
        done.exit(0);
    });
}

#[tauri::command]
pub fn quit_now(app: tauri::State<'_, Arc<App>>, handle: tauri::AppHandle) {
    if let Some(sessions) = &app.sessions {
        sessions.close_hard();
    }
    exit(&handle);
}

/// Back or closing an operation abandons all pending reviews for this page.
/// Already-confirmed deployments keep their Rust owner.
#[tauri::command]
pub fn cancel_pending(app: tauri::State<'_, Arc<App>>) {
    app.invalidate_page();
}
