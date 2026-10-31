//! Bounded, display-only run history. The engine receiver belongs to Rust.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use crate::receipt_view::{watch_view, WatchView};
use serde::Serialize;
use shipslip::interrupt::{next_stage, Stage};
use shipslip::logs::escape_field;
use shipslip::receipt::ReceiptStepStatus;
use shipslip::{DeployEvent, DeployOutcome, LogPhase, MaintenancePhase, SmokeResult};

use super::{CommandError, ErrorCode, RunInfo};
use tauri::async_runtime::{channel, Sender};
use tauri::ipc::Channel;

const OUTPUT_LINES: usize = 2_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputLine {
    pub sequence: u64,
    pub text: String,
    pub source: OutputSource,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OutputSource {
    Step {
        index: usize,
    },
    Maintenance {
        phase: MaintenancePhase,
    },
    Log {
        phase: LogPhase,
        file_line: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub name: Option<String>,
    pub status: ReceiptStepStatus,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Terminal {
    Finished {
        label: String,
        summary: String,
        succeeded: bool,
        unknown: bool,
        /// Another run took the server lock before anything ran.
        lock_taken: bool,
    },
    Detached {
        step: usize,
    },
    Interrupted {
        step: usize,
        reason: String,
    },
    Error {
        reason: String,
    },
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub terminal: Option<Terminal>,
    pub steps: BTreeMap<usize, Progress>,
    pub maintenance_up: Option<Progress>,
    pub maintenance_down: Option<Progress>,
    pub warnings: Vec<String>,
    pub dropped_warnings: u64,
    pub app_left_down: bool,
    pub server_head: Option<String>,
    pub tree_dirty: Option<bool>,
    pub watch_window_ms: Option<u64>,
    pub watch: Option<WatchView>,
    pub smoke: Option<SmokeView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmokeView {
    pub status: &'static str,
    pub http_status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub reason: Option<String>,
}
impl From<SmokeResult> for SmokeView {
    fn from(result: SmokeResult) -> Self {
        let (status, http_status, latency_ms, reason) = match result {
            SmokeResult::NotConfigured => ("notConfigured", None, None, None),
            SmokeResult::Skipped => ("skipped", None, None, None),
            SmokeResult::Passed { status, latency_ms } => {
                ("passed", Some(status), Some(latency_ms), None)
            }
            SmokeResult::Failed { status, reason } => {
                ("failed", status, None, Some(escape_field(&reason)))
            }
        };
        Self {
            status,
            http_status,
            latency_ms,
            reason,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub info: RunInfo,
    pub sequence: u64,
    pub dropped_output: u64,
    pub output: Vec<Arc<OutputLine>>,
    pub state: RunState,
}
impl RunSnapshot {
    pub fn starting(info: RunInfo) -> Self {
        Self {
            info,
            sequence: 0,
            dropped_output: 0,
            output: Vec::new(),
            state: RunState::default(),
        }
    }
}

/// Apply the state and output, then acknowledge `through`. Only one batch is
/// outstanding for a subscription; sequence numbers order replay and live data.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunBatch {
    pub handle: String,
    pub subscription: String,
    pub replay: bool,
    pub after: u64,
    pub through: u64,
    pub dropped_output: u64,
    pub output: Vec<Arc<OutputLine>>,
    pub state: RunState,
}

/// What quitting now would mean for one live run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuitRun {
    pub handle: String,
    pub project: String,
    pub env: String,
    pub checkout: String,
    pub operation: &'static str,
    pub stage: String,
    /// "Detach and quit": Shipslip reaches a safe point first.
    pub detach: String,
    /// "Quit now": exit at once.
    pub quit_now: String,
}

const ATTACH: &str = " Open Shipslip again and attach to check it.";

impl QuitRun {
    fn new(info: &RunInfo, stage: String, detach: String, quit_now: String) -> Self {
        Self {
            handle: info.handle.clone(),
            project: info.project.clone(),
            env: info.env.clone(),
            checkout: info.checkout.clone(),
            operation: info.operation,
            stage,
            detach,
            quit_now,
        }
    }

    pub fn connecting(info: &RunInfo) -> Self {
        Self::new(
            info,
            "connecting".into(),
            "Shipslip is still connecting. It stops the run before its first step, then quits."
                .into(),
            format!("The run may start without Shipslip watching it.{ATTACH}"),
        )
    }
}

struct Subscriber {
    id: String,
    wake: Sender<()>,
    cursor: u64,
    pending: Option<u64>,
    first: bool,
}

struct History {
    sequence: u64,
    stage: String,
    total_output: u64,
    output: VecDeque<Arc<OutputLine>>,
    state: RunState,
    interrupt: Stage,
    subscriber: Option<Subscriber>,
    closed: bool,
    outcome: Option<DeployOutcome>,
}

pub struct RunFeed {
    info: RunInfo,
    inner: Mutex<History>,
}
impl RunFeed {
    pub fn new(info: RunInfo) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(History {
                stage: info.stage.clone(),
                sequence: 0,
                total_output: 0,
                output: VecDeque::new(),
                state: RunState::default(),
                interrupt: Stage::Running,
                subscriber: None,
                closed: false,
                outcome: None,
            }),
            info,
        })
    }

    pub fn record(&self, event: DeployEvent) {
        let mut history = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        history.sequence += 1;
        history.interrupt = next_stage(history.interrupt, &event);
        let text = match event {
            DeployEvent::Output { index, line } => Some((line, OutputSource::Step { index })),
            DeployEvent::MaintenanceOutput { phase, line } => {
                Some((line, OutputSource::Maintenance { phase }))
            }
            DeployEvent::StepStarted { index, name } => {
                history.stage = format!("step {index}");
                history.state.steps.insert(
                    index,
                    Progress {
                        name: Some(escape_field(&name)),
                        status: ReceiptStepStatus::Running,
                        exit_code: None,
                    },
                );
                None
            }
            DeployEvent::StepFinished {
                index,
                status,
                exit_code,
                ..
            } => {
                let progress = history.state.steps.entry(index).or_insert(Progress {
                    name: None,
                    status: ReceiptStepStatus::Running,
                    exit_code: None,
                });
                progress.status = status.into();
                progress.exit_code = exit_code;
                None
            }
            DeployEvent::MaintenanceStarted { phase } => {
                history.stage = match phase {
                    MaintenancePhase::Up => "bringing up",
                    MaintenancePhase::Down => "maintenance",
                }
                .into();
                let progress = Some(Progress {
                    name: None,
                    status: ReceiptStepStatus::Running,
                    exit_code: None,
                });
                match phase {
                    MaintenancePhase::Up => history.state.maintenance_up = progress,
                    MaintenancePhase::Down => history.state.maintenance_down = progress,
                }
                None
            }
            DeployEvent::MaintenanceFinished {
                phase,
                status,
                exit_code,
                ..
            } => {
                history.stage = match phase {
                    MaintenancePhase::Up => "finishing",
                    MaintenancePhase::Down => "running",
                }
                .into();
                let progress = Some(Progress {
                    name: None,
                    status: status.into(),
                    exit_code,
                });
                match phase {
                    MaintenancePhase::Up => history.state.maintenance_up = progress,
                    MaintenancePhase::Down => history.state.maintenance_down = progress,
                }
                None
            }
            DeployEvent::Warning { reason } => {
                if history.state.warnings.len() == 16 {
                    history.state.warnings.remove(0);
                    history.state.dropped_warnings += 1;
                }
                history.state.warnings.push(escape_field(&reason));
                None
            }
            DeployEvent::AppLeftDown => {
                history.state.app_left_down = true;
                None
            }
            DeployEvent::ServerState { head, tree_dirty } => {
                history.state.server_head = head.as_deref().map(escape_field);
                history.state.tree_dirty = tree_dirty;
                None
            }
            DeployEvent::NewLogError {
                phase,
                message,
                file_line,
            } => Some((
                message,
                OutputSource::Log {
                    phase,
                    file_line: file_line.as_deref().map(escape_field),
                },
            )),
            DeployEvent::WatchStarted { window } => {
                history.stage = "watching".into();
                history.state.watch_window_ms =
                    Some(u64::try_from(window.as_millis()).unwrap_or(u64::MAX));
                None
            }
            DeployEvent::WatchFinished(result) => {
                history.stage = "finishing".into();
                history.state.watch = Some(watch_view(&result, "not observed"));
                None
            }
            DeployEvent::SmokeFinished(result) => {
                history.state.smoke = Some(result.into());
                None
            }
            DeployEvent::Finished(outcome) => {
                history.stage = "finished".into();
                history.state.terminal = Some(Terminal::Finished {
                    label: escape_field(&outcome.label()),
                    summary: escape_field(&outcome.summary()),
                    succeeded: outcome == DeployOutcome::Succeeded,
                    unknown: matches!(outcome, DeployOutcome::Unknown { .. }),
                    lock_taken: outcome
                        == DeployOutcome::AbortedBeforeChanges(shipslip::AbortReason::LockLost),
                });
                history.outcome = Some(outcome);
                None
            }
            DeployEvent::Detached { index } => {
                history.stage = "detached".into();
                history.state.terminal = Some(Terminal::Detached { step: index });
                None
            }
            DeployEvent::Interrupted { index, reason } => {
                history.stage = "needs attention".into();
                history.state.terminal = Some(Terminal::Interrupted {
                    step: index,
                    reason: escape_field(&reason),
                });
                None
            }
            DeployEvent::RunError { reason } => {
                history.stage = "needs attention".into();
                history.state.terminal = Some(Terminal::Error {
                    reason: escape_field(&reason),
                });
                None
            }
            _ => None,
        };
        if let Some((text, source)) = text {
            history.total_output += 1;
            let line = Arc::new(OutputLine {
                sequence: history.sequence,
                text: escape_field(&text),
                source,
            });
            if history.output.len() == OUTPUT_LINES {
                history.output.pop_front();
            }
            history.output.push_back(line);
        }
        notify(&history);
    }

    pub fn close(&self) {
        self.fail("The event stream closed without a final result. Inspect the saved receipt before recovery.".into());
    }

    /// Close the feed; `reason` is already escaped and only used when no result was recorded.
    pub fn fail(&self, reason: String) {
        let mut history = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if history.state.terminal.is_none() {
            history.stage = "needs attention".into();
            history.sequence += 1;
            history.state.terminal = Some(Terminal::Error { reason });
        }
        history.closed = true;
        notify(&history);
    }

    pub fn subscribe(self: &Arc<Self>, id: String, sink: Channel<RunBatch>) {
        let (wake, mut receiver) = channel(1);
        {
            let mut history = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            history.subscriber = Some(Subscriber {
                id: id.clone(),
                wake,
                cursor: 0,
                pending: None,
                first: true,
            });
            notify(&history);
        }
        let feed = Arc::downgrade(self);
        // IPC serialization and delivery never run in the engine's drain task
        // or while either registry is locked. The wake queue carries no output.
        tauri::async_runtime::spawn(async move {
            while receiver.recv().await.is_some() {
                let Some(owner) = feed.upgrade() else {
                    break;
                };
                let batch = owner.next_batch(&id);
                drop(owner);
                let Some(batch) = batch else {
                    continue;
                };
                let channel = sink.clone();
                let sent = tauri::async_runtime::spawn_blocking(move || channel.send(batch)).await;
                if !matches!(sent, Ok(Ok(()))) {
                    if let Some(owner) = feed.upgrade() {
                        owner.disconnect_if(&id);
                    }
                    break;
                }
            }
        });
    }

    fn next_batch(&self, id: &str) -> Option<RunBatch> {
        let mut history = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let subscriber = history.subscriber.as_ref().filter(|sub| sub.id == id)?;
        if subscriber.pending.is_some() {
            return None;
        }
        let (cursor, first) = (subscriber.cursor, subscriber.first);
        if !first && cursor == history.sequence {
            if history.closed {
                history.subscriber = None;
            }
            return None;
        }
        let batch = RunBatch {
            handle: self.info.handle.clone(),
            subscription: id.into(),
            replay: first,
            after: cursor,
            through: history.sequence,
            dropped_output: history.total_output - history.output.len() as u64,
            output: history
                .output
                .iter()
                .filter(|line| line.sequence > cursor)
                .cloned()
                .collect(),
            state: history.state.clone(),
        };
        let sequence = history.sequence;
        let subscriber = history.subscriber.as_mut()?;
        subscriber.pending = Some(sequence);
        subscriber.first = false;
        Some(batch)
    }

    pub fn ack(&self, id: &str, through: u64) -> Result<(), CommandError> {
        let mut history = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let subscriber = history
            .subscriber
            .as_mut()
            .filter(|sub| sub.id == id)
            .ok_or_else(subscription_error)?;
        if subscriber.pending == Some(through) {
            subscriber.cursor = through;
            subscriber.pending = None;
            notify(&history);
            Ok(())
        } else if !subscriber.first && subscriber.cursor == through {
            Ok(())
        } else {
            Err(CommandError::new(
                ErrorCode::InvalidHandle,
                "The acknowledgement does not match the pending run batch.",
            ))
        }
    }

    pub fn unsubscribe(&self, id: &str) -> Result<(), CommandError> {
        let mut history = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if !history.subscriber.as_ref().is_some_and(|sub| sub.id == id) {
            return Err(subscription_error());
        }
        history.subscriber = None;
        Ok(())
    }

    pub fn disconnect(&self) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .subscriber = None;
    }

    fn disconnect_if(&self, id: &str) {
        let _ = self.unsubscribe(id);
    }

    pub fn snapshot(&self) -> RunSnapshot {
        let history = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let mut info = self.info.clone();
        info.stage = history.stage.clone();
        RunSnapshot {
            info,
            sequence: history.sequence,
            dropped_output: history.total_output - history.output.len() as u64,
            output: history.output.iter().cloned().collect(),
            state: history.state.clone(),
        }
    }

    pub fn ending(&self) -> Ending {
        let history = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &history.state;
        let (words, mut critical) = match &history.outcome {
            Some(outcome) => (
                lower_first(&outcome.label()),
                !matches!(
                    outcome,
                    DeployOutcome::Succeeded
                        | DeployOutcome::CancelledBeforeChanges
                        | DeployOutcome::AbortedBeforeChanges(_)
                ),
            ),
            // Detached, interrupted or an error: no result was recorded.
            None => ("needs attention".into(), true),
        };
        critical |= state.app_left_down;
        let new_errors = state
            .watch
            .as_ref()
            .map_or(0, |watch| watch.groups.len() as u64 + watch.more_groups);
        let mut title = format!("{} → {}: {words}", self.info.project, self.info.env);
        if new_errors > 0 {
            let noun = if new_errors == 1 { "group" } else { "groups" };
            title.push_str(&format!(", {new_errors} new error {noun}"));
        }
        let new_errors = new_errors > 0;
        Ending {
            run_id: self.info.run_id.clone(),
            title,
            critical,
            flagged: critical || new_errors,
        }
    }

    pub fn interrupt_stage(&self) -> Stage {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .interrupt
    }

    pub fn quit_run(&self) -> QuitRun {
        let history = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let maintenance =
            history.state.maintenance_down.is_some() && history.state.maintenance_up.is_none();
        let (detach, quit_now) = match history.interrupt {
            Stage::Running => (
                format!(
                    "The current step keeps running on the server. No new step will start.{}",
                    if maintenance {
                        " Maintenance mode stays on."
                    } else {
                        ""
                    }
                ),
                format!(
                    "A step may be left running or may not have started.{}{ATTACH}",
                    if maintenance {
                        " The app may stay in maintenance mode."
                    } else {
                        ""
                    }
                ),
            ),
            Stage::BringingUp => (
                "Shipslip brings the app back up first and skips the log watch.".into(),
                format!("The app may stay in maintenance mode.{ATTACH}"),
            ),
            Stage::Watching => (
                "The log watch stops early. The deploy result does not change.".into(),
                format!("The receipt may be left unfinished.{ATTACH}"),
            ),
            Stage::Finishing => (
                "Only the lock release and receipt remain. Shipslip finishes them first.".into(),
                format!("The deploy lock and receipt may be left unfinished.{ATTACH}"),
            ),
        };
        QuitRun::new(&self.info, history.stage.clone(), detach, quit_now)
    }
}

/// How a run ended, for the signal outside the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ending {
    pub run_id: Option<String>,
    /// e.g. "shop → production: failed at step 2".
    pub title: String,
    /// Failed, unknown, stopped part-way or the app left down.
    pub critical: bool,
    /// Critical or new log errors: badged until its receipt is seen.
    pub flagged: bool,
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn notify(history: &History) {
    if let Some(subscriber) = &history.subscriber {
        let _ = subscriber.wake.try_send(());
    }
}
fn subscription_error() -> CommandError {
    CommandError::new(
        ErrorCode::InvalidHandle,
        "This run view is no longer subscribed. Reopen it.",
    )
}
