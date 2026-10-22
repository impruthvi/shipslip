//! Desktop-owned deployment sessions. Only Rust owns previews, SSH transports
//! and approval snapshots. UI requests carry map positions or opaque tickets.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use shipslip::config::{self, LoadedConfig, TrustSnapshot, TrustStatus, TrustedTarget};
use shipslip::interrupt::{on_interrupt, Interrupt, Stage};
use shipslip::logs::escape_field;
use shipslip::receipt::{self, ReceiptJournal, ReceiptStatus, ReceiptStepStatus};
use shipslip::transport::ConnectFailure;
use shipslip::transport::{SshTransport, Transport, TransportError};
use shipslip::{Confirmation, ExecutionHandle, PrepareError, Preview, RunEvents, RunPlan};

mod events;
use events::RunFeed;
pub use events::{QuitRun, RunBatch, RunSnapshot};

// Keep terminal delivery available across the completion/reload race, with
// bounded history. Active runs are never evicted.
const RECENT_RUNS: usize = 8;

pub trait Connector: Send + Sync + 'static {
    type Remote: Transport;
    fn connect(
        &self,
        alias: &str,
    ) -> impl Future<Output = Result<Arc<Self::Remote>, TransportError>> + Send;
}

pub struct OpenSsh;

impl Connector for OpenSsh {
    type Remote = SshTransport;
    async fn connect(&self, alias: &str) -> Result<Arc<Self::Remote>, TransportError> {
        SshTransport::connect(alias).await.map(Arc::new)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    Stale,
    NotFound,
    Busy,
    Cancelled,
    InvalidHandle,
    Changed,
    Untrusted,
    Confirmation,
    Connection,
    Blocked,
    Local,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_kind: Option<&'static str>,
}

impl CommandError {
    pub fn new(code: ErrorCode, message: impl AsRef<str>) -> Self {
        Self {
            code,
            message: escape_field(message.as_ref()),
            connection_kind: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    checkout: PathBuf,
    env: String,
}

#[derive(Clone)]
struct Stamp {
    key: Key,
    path: PathBuf,
    bytes: Vec<u8>,
    project: String,
    snapshot: TrustSnapshot,
}

struct Settings {
    config: LoadedConfig,
    stamp: Stamp,
}

async fn settings(checkout: PathBuf, env: String) -> Result<Settings, CommandError> {
    blocking(move || settings_sync(checkout, env)).await
}

fn settings_sync(checkout: PathBuf, env: String) -> Result<Settings, CommandError> {
    let checkout = std::fs::canonicalize(checkout).map_err(local)?;
    let config = LoadedConfig::load(&checkout, None).map_err(local)?;
    if config.repo_root() != checkout {
        return Err(CommandError::new(
            ErrorCode::Changed,
            "The selected checkout changed. Refresh the map.",
        ));
    }
    let snapshot = config.trust_snapshot(&env).ok_or_else(|| {
        CommandError::new(
            ErrorCode::NotFound,
            "The environment is no longer in this config.",
        )
    })?;
    let stamp = Stamp {
        key: Key { checkout, env },
        path: config.path().into(),
        bytes: std::fs::read(config.path()).map_err(local)?,
        project: config.project_name().into(),
        snapshot,
    };
    Ok(Settings { config, stamp })
}

async fn unchanged(stamp: &Stamp) -> Result<LoadedConfig, CommandError> {
    let stamp = stamp.clone();
    blocking(move || unchanged_sync(&stamp)).await
}

fn unchanged_sync(stamp: &Stamp) -> Result<LoadedConfig, CommandError> {
    let current = settings_sync(stamp.key.checkout.clone(), stamp.key.env.clone())?;
    if current.stamp.path != stamp.path
        || current.stamp.bytes != stamp.bytes
        || current.stamp.project != stamp.project
        || current.stamp.snapshot != stamp.snapshot
    {
        return Err(CommandError::new(
            ErrorCode::Changed,
            "The config changed. Review it again before continuing.",
        ));
    }
    Ok(current.config)
}

async fn blocking<R: Send + 'static>(
    work: impl FnOnce() -> Result<R, CommandError> + Send + 'static,
) -> Result<R, CommandError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| CommandError::new(ErrorCode::Local, "The request failed inside the app."))?
}

fn local(error: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorCode::Local, error.to_string())
}

struct TrustTicket {
    page: u64,
    stamp: Stamp,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustReview {
    pub handle: String,
    pub env: String,
    pub project: String,
    pub changes: Vec<String>,
}

// Per canonical (checkout, env): Idle -> Preparing -> Ready -> Starting -> Running.
// Reload abandons Preparing, cancels Ready, and preserves Starting/Running.
struct Slot<T: Transport> {
    id: String,
    page: u64,
    phase: Phase<T>,
}
enum Phase<T: Transport> {
    Preparing,
    Ready(Box<Ready<T>>),
    Recovery(Recovery),
    Starting(RunInfo),
    Running {
        info: RunInfo,
        control: ExecutionHandle,
    },
}
struct Ready<T: Transport> {
    stamp: Stamp,
    remote: Arc<T>,
    preview: Preview,
}

struct Recovery {
    stamp: Stamp,
    saved: Option<Saved>,
}
struct Saved {
    path: PathBuf,
    bytes: Vec<u8>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryReview {
    pub handle: String,
    pub env: String,
    pub production: bool,
    pub operation: &'static str,
    pub ssh_alias: String,
    pub path: String,
    pub run_id: Option<String>,
    pub target_sha: Option<String>,
    pub phase: String,
    pub remaining_steps: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployReview {
    pub handle: String,
    pub env: String,
    pub production: bool,
    pub checkout: String,
    pub ssh_alias: String,
    pub path: String,
    pub branch: String,
    pub maintenance: bool,
    pub recipe_hash: String,
    pub from_sha: String,
    pub target_sha: String,
    pub commits: Vec<String>,
    pub more_commits: usize,
    pub steps: Vec<String>,
    pub run_plan: RunPlan,
}
impl DeployReview {
    fn from_preview(handle: String, preview: &Preview, checkout: &std::path::Path) -> Self {
        Self {
            handle,
            env: escape_field(&preview.target().env),
            production: preview.target().production,
            checkout: escape_field(&checkout.to_string_lossy()),
            ssh_alias: escape_field(&preview.target().ssh_alias),
            path: escape_field(&preview.target().path),
            branch: escape_field(&preview.target().branch),
            maintenance: preview.target().maintenance,
            recipe_hash: escape_field(preview.recipe_hash()),
            from_sha: escape_field(preview.from_sha()),
            target_sha: escape_field(preview.target_sha()),
            commits: preview.commits().iter().map(|s| escape_field(s)).collect(),
            more_commits: preview.more_commits(),
            steps: preview
                .target()
                .steps
                .iter()
                .map(|s| escape_field(s))
                .collect(),
            run_plan: preview.run_plan(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    pub handle: String,
    pub project: String,
    pub checkout: String,
    pub env: String,
    pub run_id: Option<String>,
    pub target_sha: Option<String>,
    pub operation: &'static str,
    pub stage: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentRuns {
    pub runs: Vec<RunSnapshot>,
    pub recent: Vec<RunSnapshot>,
    pub warnings: Vec<CommandError>,
}

struct Registry<T: Transport> {
    page: u64,
    next: u64,
    tickets: HashMap<String, TrustTicket>,
    slots: HashMap<Key, Slot<T>>,
    warnings: Vec<CommandError>,
    feeds: HashMap<String, Arc<RunFeed>>,
    recent: VecDeque<String>,
    /// Whether any run is starting or running, as last told to the observer.
    live: bool,
    quitting: bool,
    idle: Vec<tauri::async_runtime::Sender<()>>,
}

pub struct Sessions<C: Connector> {
    connector: C,
    receipts_root: PathBuf,
    trust_path: PathBuf,
    nonce: String,
    inner: Mutex<Registry<C::Remote>>,
    live_observer: OnceLock<Box<dyn Fn(bool) + Send + Sync>>,
}

impl<C: Connector> Sessions<C> {
    pub fn new(receipts_root: PathBuf, trust_path: PathBuf, connector: C) -> Arc<Self> {
        let nonce = format!(
            "{:x}-{:x}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        Arc::new(Self {
            connector,
            receipts_root,
            trust_path,
            nonce,
            inner: Mutex::new(Registry {
                page: 0,
                next: 0,
                tickets: HashMap::new(),
                slots: HashMap::new(),
                warnings: Vec::new(),
                feeds: HashMap::new(),
                recent: VecDeque::new(),
                live: false,
                quitting: false,
                idle: Vec::new(),
            }),
            live_observer: OnceLock::new(),
        })
    }

    fn registry(&self) -> MutexGuard<'_, Registry<C::Remote>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn next_handle(&self, state: &mut Registry<C::Remote>) -> String {
        state.next += 1;
        format!("{}-{:x}", self.nonce, state.next)
    }

    pub async fn review_trust(
        &self,
        checkout: PathBuf,
        env: String,
    ) -> Result<TrustReview, CommandError> {
        let page = self.registry().page;
        let loaded = settings(checkout, env).await?;
        let trust_path = self.trust_path.clone();
        let stamp = loaded.stamp;
        let check = stamp.clone();
        let previous = blocking(move || {
            config::trust_status(
                &trust_path,
                &check.key.checkout,
                &check.key.env,
                &check.snapshot,
            )
            .map_err(local)
        })
        .await?;
        let changes = config::trust_changes(
            match &previous {
                TrustStatus::Trusted => Some(&stamp.snapshot),
                TrustStatus::Untrusted { previous } => previous.as_ref(),
            },
            &stamp.snapshot,
        )
        .into_iter()
        .map(|s| escape_field(&s))
        .collect();
        let mut state = self.registry();
        if state.page != page {
            return Err(cancelled());
        }
        state.tickets.retain(|_, old| old.stamp.key != stamp.key);
        let handle = self.next_handle(&mut state);
        let review = TrustReview {
            handle: handle.clone(),
            env: escape_field(&stamp.key.env),
            project: escape_field(&stamp.project),
            changes,
        };
        state.tickets.insert(handle, TrustTicket { page, stamp });
        Ok(review)
    }

    pub fn invalidate_page(self: &Arc<Self>) {
        let cleanup = {
            let mut state = self.registry();
            state.page += 1;
            state.tickets.clear();
            for feed in state.feeds.values() {
                feed.disconnect();
            }
            state
                .slots
                .retain(|_, slot| !matches!(slot.phase, Phase::Recovery(_)));
            state
                .slots
                .iter_mut()
                .filter_map(|(key, slot)| {
                    if !matches!(slot.phase, Phase::Ready(_)) {
                        return None;
                    }
                    let Phase::Ready(ready) = std::mem::replace(&mut slot.phase, Phase::Preparing)
                    else {
                        unreachable!()
                    };
                    Some((key.clone(), slot.id.clone(), ready))
                })
                .collect::<Vec<_>>()
        };
        for (key, id, ready) in cleanup {
            let owner = self.clone();
            tauri::async_runtime::spawn(async move {
                let _ = owner
                    .cancel_preview(&key, ready.preview, ready.remote)
                    .await;
                owner.remove(&key, &id);
            });
        }
    }

    async fn cancel_preview(
        &self,
        key: &Key,
        preview: Preview,
        remote: Arc<C::Remote>,
    ) -> Result<(), CommandError> {
        match shipslip::cancel(preview, &*remote).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let message = format!("Could not release the preview lock for {} ({}): {error}. Check the old run on the server before using slip break-lock {} or trying again.", key.env, key.checkout.display(), key.env);
                let mut state = self.registry();
                if state.warnings.len() == 16 {
                    state.warnings.remove(0);
                }
                state
                    .warnings
                    .push(CommandError::new(ErrorCode::Connection, message));
                Err(connection(error))
            }
        }
    }

    pub async fn cancel(self: &Arc<Self>, handle: &str) -> Result<(), CommandError> {
        let owner = self.clone();
        let handle = handle.to_owned();
        tauri::async_runtime::spawn(async move {
            let (key, ready) = {
                let mut state = owner.registry();
                let page = state.page;
                let (key, slot) = state
                    .slots
                    .iter_mut()
                    .find(|(_, slot)| {
                        slot.id == handle
                            && slot.page == page
                            && matches!(slot.phase, Phase::Ready(_) | Phase::Recovery(_))
                    })
                    .ok_or_else(invalid_handle)?;
                if matches!(slot.phase, Phase::Recovery(_)) {
                    let key = key.clone();
                    state.slots.remove(&key);
                    return Ok(());
                }
                let Phase::Ready(ready) = std::mem::replace(&mut slot.phase, Phase::Preparing)
                else {
                    unreachable!()
                };
                (key.clone(), ready)
            };
            let result = owner
                .cancel_preview(&key, ready.preview, ready.remote)
                .await;
            owner.remove(&key, &handle);
            result
        })
        .await
        .map_err(local)?
    }

    fn remove(&self, key: &Key, id: &str) {
        let mut state = self.registry();
        if state.slots.get(key).is_some_and(|slot| slot.id == id) {
            state.slots.remove(key);
        }
        self.settle(&mut state);
    }

    /// Called with `true` when the first run starts and `false` when the last
    /// one ends, in order, while the registry is locked. Keep it cheap.
    pub fn set_live_observer(&self, observer: impl Fn(bool) + Send + Sync + 'static) {
        let _ = self.live_observer.set(Box::new(observer));
    }

    fn settle(&self, state: &mut Registry<C::Remote>) {
        let live = state
            .slots
            .values()
            .any(|slot| matches!(slot.phase, Phase::Starting(_) | Phase::Running { .. }));
        if live != state.live {
            state.live = live;
            if let Some(observer) = self.live_observer.get() {
                observer(live);
            }
        }
        if !live {
            for waiter in state.idle.drain(..) {
                let _ = waiter.try_send(());
            }
        }
    }

    pub fn quit_runs(&self) -> Vec<QuitRun> {
        let state = self.registry();
        let mut runs: Vec<_> = state
            .slots
            .values()
            .filter_map(|slot| match &slot.phase {
                Phase::Starting(info) => Some(QuitRun::connecting(info)),
                Phase::Running { info, .. } => Some(match state.feeds.get(&slot.id) {
                    Some(feed) => feed.quit_run(),
                    None => QuitRun::connecting(info),
                }),
                _ => None,
            })
            .collect();
        runs.sort_by(|a, b| a.handle.cmp(&b.handle));
        runs
    }

    /// Bring every live run to a safe point the way an interrupt would, and
    /// refuse new runs. Irreversible: the app is about to exit.
    pub fn begin_quit(&self) {
        let mut state = self.registry();
        state.quitting = true;
        for slot in state.slots.values() {
            if let Phase::Running { control, .. } = &slot.phase {
                let stage = state
                    .feeds
                    .get(&slot.id)
                    .map_or(Stage::Running, |feed| feed.interrupt_stage());
                interrupt(control, stage);
            }
        }
    }

    /// Resolves once no run is starting or running.
    pub async fn wait_idle(&self) {
        let mut idle = {
            let mut state = self.registry();
            if !state.live {
                return;
            }
            let (sender, receiver) = tauri::async_runtime::channel(1);
            state.idle.push(sender);
            receiver
        };
        idle.recv().await;
    }

    pub fn subscribe_run(
        &self,
        handle: &str,
        channel: tauri::ipc::Channel<RunBatch>,
    ) -> Result<String, CommandError> {
        let mut state = self.registry();
        let feed = state.feeds.get(handle).cloned().ok_or_else(run_not_found)?;
        let id = self.next_handle(&mut state);
        feed.subscribe(id.clone(), channel);
        Ok(id)
    }

    pub fn ack_run(
        &self,
        handle: &str,
        subscription: &str,
        through: u64,
    ) -> Result<(), CommandError> {
        let state = self.registry();
        state
            .feeds
            .get(handle)
            .ok_or_else(run_not_found)?
            .ack(subscription, through)
    }

    pub fn unsubscribe_run(&self, handle: &str, subscription: &str) -> Result<(), CommandError> {
        let state = self.registry();
        state
            .feeds
            .get(handle)
            .ok_or_else(run_not_found)?
            .unsubscribe(subscription)
    }

    pub fn current_runs(&self) -> CurrentRuns {
        let (active, recent, warnings) = {
            let state = self.registry();
            let active: Vec<_> = state
                .slots
                .values()
                .filter_map(|slot| match &slot.phase {
                    Phase::Running { info, .. } | Phase::Starting(info) => {
                        Some((info.clone(), state.feeds.get(&slot.id).cloned()))
                    }
                    _ => None,
                })
                .collect();
            let recent: Vec<_> = state
                .recent
                .iter()
                .filter_map(|id| state.feeds.get(id).cloned())
                .collect();
            (active, recent, state.warnings.clone())
        };
        let mut runs: Vec<_> = active
            .into_iter()
            .map(|(info, feed)| match feed {
                Some(feed) => feed.snapshot(),
                None => RunSnapshot::starting(info),
            })
            .collect();
        runs.sort_by(|a, b| a.info.handle.cmp(&b.info.handle));
        CurrentRuns {
            runs,
            recent: recent.iter().map(|feed| feed.snapshot()).collect(),
            warnings,
        }
    }

    fn archive(&self, key: &Key, handle: &str) {
        let mut state = self.registry();
        if state.slots.get(key).is_some_and(|slot| slot.id == handle) {
            state.slots.remove(key);
        }
        state.recent.push_back(handle.into());
        while state.recent.len() > RECENT_RUNS {
            if let Some(old) = state.recent.pop_front() {
                state.feeds.remove(&old);
            }
        }
        self.settle(&mut state);
    }

    pub async fn review_up(
        self: &Arc<Self>,
        checkout: PathBuf,
        env: String,
    ) -> Result<RecoveryReview, CommandError> {
        let page = self.registry().page;
        let loaded = settings(checkout, env).await?;
        let trusted = self.trusted(&loaded).await?;
        let mut state = self.registry();
        if state.page != page {
            return Err(cancelled());
        }
        let key = loaded.stamp.key.clone();
        if state.slots.contains_key(&key) {
            return Err(busy());
        }
        let id = self.next_handle(&mut state);
        let target = trusted.target();
        let review = RecoveryReview {
            handle: id.clone(),
            env: escape_field(&key.env),
            production: target.production,
            operation: "up",
            ssh_alias: escape_field(&target.ssh_alias),
            path: escape_field(&target.path),
            run_id: None,
            target_sha: None,
            phase: "Bring this app out of maintenance mode".into(),
            remaining_steps: vec!["php artisan up".into()],
        };
        state.slots.insert(
            key,
            Slot {
                id,
                page,
                phase: Phase::Recovery(Recovery {
                    stamp: loaded.stamp,
                    saved: None,
                }),
            },
        );
        Ok(review)
    }

    pub async fn review_attach(
        self: &Arc<Self>,
        checkout: PathBuf,
        env: String,
    ) -> Result<RecoveryReview, CommandError> {
        let page = self.registry().page;
        let loaded = settings(checkout, env).await?;
        let trusted = self.trusted(&loaded).await?;
        let root = self.receipts_root.clone();
        let (saved, receipt) = blocking(move || {
            let path = receipt::find_open(
                &root,
                trusted.project(),
                &trusted.target().env,
                trusted.repo_root(),
            )
            .map_err(local)?
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::NotFound,
                    "There is no unfinished run for this checkout and environment.",
                )
            })?;
            let before = std::fs::read(&path).map_err(local)?;
            let receipt = receipt::read(&path).map_err(local)?;
            if std::fs::read(&path).map_err(local)? != before {
                return Err(changed_receipt());
            }
            shipslip::check_resumable(&trusted, &receipt)
                .map_err(|e| CommandError::new(ErrorCode::Changed, e.to_string()))?;
            Ok((
                Saved {
                    path,
                    bytes: before,
                },
                receipt,
            ))
        })
        .await?;
        let mut state = self.registry();
        if state.page != page {
            return Err(cancelled());
        }
        let key = loaded.stamp.key.clone();
        if state.slots.contains_key(&key) {
            return Err(busy());
        }
        let id = self.next_handle(&mut state);
        let review = RecoveryReview {
            handle: id.clone(),
            env: escape_field(&key.env),
            production: receipt.target.production,
            operation: "attach",
            ssh_alias: escape_field(&receipt.target.ssh_alias),
            path: escape_field(&receipt.target.path),
            run_id: Some(escape_field(&receipt.run_id)),
            target_sha: Some(escape_field(&receipt.target_sha)),
            phase: format!("{:?}", receipt.phase),
            remaining_steps: receipt
                .steps
                .iter()
                .filter(|step| {
                    !matches!(
                        step.status,
                        ReceiptStepStatus::Ok | ReceiptStepStatus::Skipped
                    )
                })
                .map(|step| escape_field(&step.command))
                .collect(),
        };
        state.slots.insert(
            key,
            Slot {
                id,
                page,
                phase: Phase::Recovery(Recovery {
                    stamp: loaded.stamp,
                    saved: Some(saved),
                }),
            },
        );
        Ok(review)
    }

    async fn confirm_recovery(
        self: &Arc<Self>,
        handle: &str,
        page: u64,
        recovery: Recovery,
    ) -> Result<RunInfo, CommandError> {
        let key = recovery.stamp.key.clone();
        let project = escape_field(&recovery.stamp.project);
        let result = async {
            let config = unchanged(&recovery.stamp).await?;
            let trusted = self
                .trusted(&Settings {
                    config,
                    stamp: recovery.stamp,
                })
                .await?;
            let attach_journal = if let Some(saved) = recovery.saved {
                let target = trusted.clone();
                Some(
                    blocking(move || {
                        if std::fs::read(&saved.path).map_err(local)? != saved.bytes {
                            return Err(changed_receipt());
                        }
                        let journal = ReceiptJournal::load(&saved.path).map_err(local)?;
                        journal.claim().map_err(local)?;
                        if std::fs::read(&saved.path).map_err(local)? != saved.bytes {
                            return Err(changed_receipt());
                        }
                        let receipt = journal.snapshot();
                        if receipt.status != ReceiptStatus::InProgress {
                            return Err(changed_receipt());
                        }
                        shipslip::check_resumable(&target, &receipt)
                            .map_err(|e| CommandError::new(ErrorCode::Changed, e.to_string()))?;
                        Ok(Arc::new(journal))
                    })
                    .await?,
                )
            } else {
                None
            };
            let saved = attach_journal.as_ref().map(|j| j.snapshot());
            let info = RunInfo {
                handle: handle.into(),
                project,
                checkout: escape_field(&key.checkout.to_string_lossy()),
                env: escape_field(&key.env),
                run_id: saved.as_ref().map(|r| escape_field(&r.run_id)),
                target_sha: saved.as_ref().map(|r| escape_field(&r.target_sha)),
                operation: if saved.is_some() { "attach" } else { "up" },
                stage: "starting".into(),
            };
            {
                let mut state = self.registry();
                if state.page != page {
                    return Err(cancelled());
                }
                if state.quitting {
                    return Err(quitting());
                }
                state.slots.get_mut(&key).ok_or_else(invalid_handle)?.phase =
                    Phase::Starting(info.clone());
                state
                    .feeds
                    .insert(handle.into(), RunFeed::new(info.clone()));
                self.settle(&mut state);
            }
            let remote = self
                .connector
                .connect(&trusted.target().ssh_alias)
                .await
                .map_err(connection)?;
            let (events, control) = if let Some(journal) = attach_journal {
                shipslip::attach(&trusted, journal, remote)
                    .map_err(|e| CommandError::new(ErrorCode::Blocked, e.to_string()))?
            } else {
                shipslip::bring_app_up(&trusted, remote, &self.receipts_root)
                    .await
                    .map_err(|e| CommandError::new(ErrorCode::Blocked, e.to_string()))?
            };
            {
                let mut state = self.registry();
                // Quit was requested while connecting: stop before the first step.
                if state.quitting {
                    interrupt(&control, Stage::Running);
                }
                state.slots.get_mut(&key).ok_or_else(invalid_handle)?.phase = Phase::Running {
                    info: info.clone(),
                    control,
                };
            }
            self.drain(key.clone(), handle.into(), events);
            Ok(info)
        }
        .await;
        if let Err(error) = &result {
            self.abandon(&key, handle, error);
        }
        result
    }

    // A run that failed after Starting stays replayable for a reloaded view.
    fn abandon(&self, key: &Key, handle: &str, error: &CommandError) {
        let feed = self.registry().feeds.get(handle).cloned();
        match feed {
            Some(feed) => {
                feed.fail(error.message.clone());
                self.archive(key, handle);
            }
            None => self.remove(key, handle),
        }
    }

    pub async fn confirm(
        self: &Arc<Self>,
        handle: &str,
        typed_env: Option<String>,
    ) -> Result<RunInfo, CommandError> {
        let owner = self.clone();
        let handle = handle.to_owned();
        tauri::async_runtime::spawn(async move { owner.confirm_inner(handle, typed_env).await })
            .await
            .map_err(local)?
    }

    async fn confirm_inner(
        self: &Arc<Self>,
        handle: String,
        typed_env: Option<String>,
    ) -> Result<RunInfo, CommandError> {
        let recovery = {
            let mut state = self.registry();
            let page = state.page;
            let slot = state
                .slots
                .values_mut()
                .find(|slot| slot.id == handle && slot.page == page)
                .ok_or_else(invalid_handle)?;
            if let Phase::Recovery(recovery) = &slot.phase {
                if recovery.stamp.snapshot.production
                    && typed_env.as_deref() != Some(&recovery.stamp.key.env)
                {
                    return Err(CommandError::new(
                        ErrorCode::Confirmation,
                        "Type the exact environment name to confirm this operation.",
                    ));
                }
                let Phase::Recovery(recovery) =
                    std::mem::replace(&mut slot.phase, Phase::Preparing)
                else {
                    unreachable!()
                };
                Some((page, recovery))
            } else {
                None
            }
        };
        if let Some((page, recovery)) = recovery {
            return self.confirm_recovery(&handle, page, recovery).await;
        }
        let (key, page, ready, confirmation) = {
            let mut state = self.registry();
            let page = state.page;
            let (key, slot) = state
                .slots
                .iter_mut()
                .find(|(_, slot)| {
                    slot.id == handle && slot.page == page && matches!(slot.phase, Phase::Ready(_))
                })
                .ok_or_else(invalid_handle)?;
            let Phase::Ready(ready) = &slot.phase else {
                unreachable!()
            };
            let confirmation = Confirmation::from(&ready.preview, typed_env.as_deref())
                .map_err(|e| CommandError::new(ErrorCode::Confirmation, e.to_string()))?;
            let Phase::Ready(ready) = std::mem::replace(&mut slot.phase, Phase::Preparing) else {
                unreachable!()
            };
            (key.clone(), page, ready, confirmation)
        };
        let check = ready.stamp.clone();
        let trust_path = self.trust_path.clone();
        let approved = blocking(move || {
            unchanged_sync(&check)?
                .trusted_target(&check.key.env, &trust_path)
                .map_err(|e| CommandError::new(ErrorCode::Untrusted, e.to_string()))
        })
        .await;
        if let Err(error) = approved {
            let cleanup = self.cancel_preview(&key, ready.preview, ready.remote).await;
            self.remove(&key, &handle);
            cleanup?;
            return Err(error);
        }
        let root = self.receipts_root.clone();
        let (ready, journal) = tauri::async_runtime::spawn_blocking(move || {
            let journal = ReceiptJournal::create(
                &root,
                &ready.stamp.project,
                &ready.stamp.key.checkout,
                &ready.preview,
            )
            .map(Arc::new)
            .map_err(local);
            (ready, journal)
        })
        .await
        .map_err(local)?;
        let journal = match journal {
            Ok(journal) => journal,
            Err(error) => {
                let cleanup = self.cancel_preview(&key, ready.preview, ready.remote).await;
                self.remove(&key, &handle);
                cleanup?;
                return Err(error);
            }
        };
        let info = RunInfo {
            handle: handle.clone(),
            project: escape_field(&ready.stamp.project),
            checkout: escape_field(&key.checkout.to_string_lossy()),
            env: escape_field(&key.env),
            run_id: Some(escape_field(ready.preview.run_id())),
            target_sha: Some(escape_field(ready.preview.target_sha())),
            operation: "deploy",
            stage: "starting".into(),
        };
        let result = {
            let mut state = self.registry();
            if state.page != page {
                Err((ready, cancelled()))
            } else if state.quitting {
                Err((ready, quitting()))
            } else {
                match shipslip::execute_recorded(
                    ready.preview,
                    confirmation,
                    ready.remote.clone(),
                    journal.clone(),
                ) {
                    Ok((events, control)) => {
                        state.slots.get_mut(&key).ok_or_else(invalid_handle)?.phase =
                            Phase::Running {
                                info: info.clone(),
                                control,
                            };
                        self.settle(&mut state);
                        Ok(events)
                    }
                    Err(rejected) => Err((
                        Box::new(Ready {
                            preview: *rejected.preview,
                            ..*ready
                        }),
                        local(rejected.error),
                    )),
                }
            }
        };
        match result {
            Ok(events) => {
                self.drain(key, handle, events);
                Ok(info)
            }
            Err((ready, error)) => {
                let cleanup = self.cancel_preview(&key, ready.preview, ready.remote).await;
                let finalized = blocking(move || {
                    journal
                        .finish(shipslip::DeployOutcome::CancelledBeforeChanges)
                        .map_err(local)
                })
                .await;
                self.remove(&key, &handle);
                cleanup?;
                finalized?;
                Err(error)
            }
        }
    }

    fn drain(self: &Arc<Self>, key: Key, handle: String, mut events: RunEvents) {
        let feed = {
            let mut state = self.registry();
            let Some(Slot {
                phase: Phase::Running { info, .. },
                ..
            }) = state.slots.get(&key)
            else {
                return;
            };
            let info = info.clone();
            state
                .feeds
                .entry(handle.clone())
                .or_insert_with(|| RunFeed::new(info))
                .clone()
        };
        let owner = self.clone();
        // Always drain without a webview so output backpressure cannot stall the runner.
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                feed.record(event);
            }
            feed.close();
            owner.archive(&key, &handle);
        });
    }

    pub async fn prepare(
        self: &Arc<Self>,
        checkout: PathBuf,
        env: String,
        plan: RunPlan,
    ) -> Result<DeployReview, CommandError> {
        let owner = self.clone();
        let page = self.registry().page;
        tauri::async_runtime::spawn(
            async move { owner.prepare_inner(checkout, env, plan, page).await },
        )
        .await
        .map_err(local)?
    }

    async fn trusted(&self, loaded: &Settings) -> Result<TrustedTarget, CommandError> {
        let stamp = loaded.stamp.clone();
        let path = self.trust_path.clone();
        blocking(move || {
            unchanged_sync(&stamp)?
                .trusted_target(&stamp.key.env, &path)
                .map_err(|e| {
                    CommandError::new(
                        ErrorCode::Untrusted,
                        format!("{e}. Review and approve these settings first."),
                    )
                })
        })
        .await
    }

    async fn prepare_inner(
        self: &Arc<Self>,
        checkout: PathBuf,
        env: String,
        plan: RunPlan,
        page: u64,
    ) -> Result<DeployReview, CommandError> {
        let loaded = settings(checkout, env).await?;
        let trusted = self.trusted(&loaded).await?;
        let key = loaded.stamp.key.clone();
        let id = {
            let mut state = self.registry();
            if state.page != page {
                return Err(cancelled());
            }
            if state.slots.contains_key(&key) {
                return Err(busy());
            }
            let id = self.next_handle(&mut state);
            state.slots.insert(
                key.clone(),
                Slot {
                    id: id.clone(),
                    page,
                    phase: Phase::Preparing,
                },
            );
            id
        };
        let result = async {
            let remote = self
                .connector
                .connect(&trusted.target().ssh_alias)
                .await
                .map_err(connection)?;
            let preview = shipslip::prepare_with_plan(&trusted, plan, &*remote)
                .await
                .map_err(|e| prepare_error(e, &key.env))?;
            let review = DeployReview::from_preview(id.clone(), &preview, &key.checkout);
            let ready = Ready {
                stamp: loaded.stamp,
                remote,
                preview,
            };
            let ready = {
                let mut state = self.registry();
                if state.page == page {
                    state.slots.get_mut(&key).ok_or_else(invalid_handle)?.phase =
                        Phase::Ready(Box::new(ready));
                    return Ok(review);
                }
                ready
            };
            self.cancel_preview(&key, ready.preview, ready.remote)
                .await?;
            Err(cancelled())
        }
        .await;
        if result.is_err() {
            self.remove(&key, &id);
        }
        result
    }

    pub async fn approve_trust(
        self: &Arc<Self>,
        handle: &str,
        typed_env: &str,
    ) -> Result<(), CommandError> {
        let stamp = {
            let state = self.registry();
            let ticket = state
                .tickets
                .get(handle)
                .filter(|t| t.page == state.page)
                .ok_or_else(invalid_handle)?;
            if typed_env != ticket.stamp.key.env {
                return Err(CommandError::new(
                    ErrorCode::Confirmation,
                    "Type the exact environment name to approve its settings.",
                ));
            }
            ticket.stamp.clone()
        };
        let owner = self.clone();
        let handle = handle.to_owned();
        blocking(move || {
            let mut state = owner.registry();
            let ticket = state
                .tickets
                .get(&handle)
                .filter(|t| t.page == state.page)
                .ok_or_else(invalid_handle)?;
            if ticket.stamp.key != stamp.key {
                return Err(invalid_handle());
            }
            if state.slots.contains_key(&stamp.key) {
                return Err(busy());
            }
            // Serialize the final snapshot check and approval with invalidation.
            unchanged_sync(&stamp)?;
            config::approve_trust(
                &owner.trust_path,
                &stamp.key.checkout,
                &stamp.key.env,
                stamp.snapshot,
            )
            .map_err(local)?;
            state.tickets.remove(&handle);
            Ok(())
        })
        .await
    }
}

fn interrupt(control: &ExecutionHandle, stage: Stage) {
    match on_interrupt(stage, false) {
        Interrupt::Detach => {
            control.detach();
            control.cancel_watch();
        }
        Interrupt::SkipWatch | Interrupt::StopWatch => control.cancel_watch(),
        Interrupt::Wait | Interrupt::Quit => {}
    }
}

fn quitting() -> CommandError {
    CommandError::new(
        ErrorCode::Cancelled,
        "Shipslip is quitting, so no new run was started.",
    )
}
fn run_not_found() -> CommandError {
    CommandError::new(
        ErrorCode::NotFound,
        "This run is no longer retained. Open its saved receipt.",
    )
}
fn changed_receipt() -> CommandError {
    CommandError::new(
        ErrorCode::Changed,
        "The saved run changed. Review it again before attaching.",
    )
}
fn busy() -> CommandError {
    CommandError::new(
        ErrorCode::Busy,
        "This checkout and environment already have a deployment session.",
    )
}
fn connection(error: TransportError) -> CommandError {
    let kind = match error.connect_failure() {
        Some(ConnectFailure::HostKeyUnknown) => "hostKeyUnknown",
        Some(ConnectFailure::HostKeyChanged) => "hostKeyChanged",
        Some(ConnectFailure::KeyNotLoaded) => "keyNotLoaded",
        _ => "other",
    };
    let mut result = CommandError::new(ErrorCode::Connection, error.to_string());
    result.connection_kind = Some(kind);
    result
}
fn prepare_error(error: PrepareError, env: &str) -> CommandError {
    match error {
        PrepareError::Transport(e) => connection(e),
        PrepareError::LockHeld(info) => CommandError::new(ErrorCode::Blocked, format!("Deploy lock {info}. Check the old run on the server; use slip break-lock {env} only after reviewing the holder.")),
        PrepareError::Blocked(shipslip::BlockReason::FetchFailed(output)) => CommandError::new(ErrorCode::Blocked, format!("The server could not fetch origin: {output}. Environments that require a local GitHub token must deploy through the CLI; token authentication is not available in the app.")),
        other => CommandError::new(ErrorCode::Blocked, other.to_string()),
    }
}
fn invalid_handle() -> CommandError {
    CommandError::new(
        ErrorCode::InvalidHandle,
        "This review is no longer available. Start again.",
    )
}
fn cancelled() -> CommandError {
    CommandError::new(
        ErrorCode::Cancelled,
        "The workspace was closed before preparation finished.",
    )
}

/// Quit dialog states for the UI fixtures: a step running with maintenance on,
/// and a run still connecting.
#[cfg(test)]
pub(crate) fn sample_quit_runs() -> Vec<QuitRun> {
    use shipslip::{DeployEvent, MaintenancePhase, StepStatus};
    let info = |handle: &str, operation| RunInfo {
        handle: handle.into(),
        project: "shop".into(),
        checkout: "/Users/you/code/shop".into(),
        env: "production".into(),
        run_id: None,
        target_sha: None,
        operation,
        stage: "starting".into(),
    };
    let feed = RunFeed::new(info("run-1", "deploy"));
    feed.record(DeployEvent::MaintenanceStarted {
        phase: MaintenancePhase::Down,
    });
    feed.record(DeployEvent::MaintenanceFinished {
        phase: MaintenancePhase::Down,
        status: StepStatus::Ok,
        exit_code: Some(0),
    });
    feed.record(DeployEvent::StepStarted {
        index: 2,
        name: "php artisan migrate --force".into(),
    });
    vec![feed.quit_run(), QuitRun::connecting(&info("run-2", "up"))]
}

#[cfg(test)]
mod tests;
