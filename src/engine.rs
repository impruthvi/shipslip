//! Two-phase deploy: [`prepare`] (read-only) then [`execute`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};

use crate::event::{DeployEvent, DeployOutcome, MaintenancePhase, StepStatus, StopReason};
use crate::lock::{self, Acquire, Break, LockInfo, LockOwner};
use crate::observation::{self, LogObserver, SmokeResult, WatchResult, WatchStatus};
use crate::preflight::{self, AbortReason, BlockReason, State};
use crate::receipt::{Receipt, ReceiptJournal, ReceiptPhase, ReceiptStatus, ReceiptStepStatus};
use crate::runner::{self, run_collect, StepResult};
use crate::script::{is_full_sha, is_safe_branch, wrap_step};
use crate::transport::{Transport, TransportError};

/// One environment of one project, as resolved from config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployTarget {
    pub env: String,
    pub production: bool,
    pub ssh_alias: String,
    pub path: String,
    pub branch: String,
    /// Recipe steps 1..=N (step 0, the git fast-forward, is built in).
    pub steps: Vec<String>,
    /// Run `php artisan down` before step 0 and `php artisan up` after the
    /// last step succeeds. On any failure the app stays down.
    pub maintenance: bool,
    /// Watch the configured Laravel log during and after the deploy.
    #[serde(default)]
    pub watch_log: bool,
    /// A fixed log path, or the directory and prefix when `log_daily` is set.
    #[serde(default)]
    pub log: Option<String>,
    #[serde(default)]
    pub log_daily: bool,
    #[serde(default)]
    pub smoke_url: Option<String>,
}

/// Which part of the configured recipe to run after its commit is deployed.
/// Recipe steps are numbered from 1; step 0 is the built-in Git fast-forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunPlan {
    Deploy,
    /// Run all recipe steps against the already-checked-out target commit.
    Rerun,
    /// Run recipe step `step` and the remaining recipe steps.
    FromStep(usize),
}

impl RunPlan {
    fn is_rerun(self) -> bool {
        !matches!(self, Self::Deploy)
    }

    fn first_recipe_step(self) -> usize {
        match self {
            Self::Deploy | Self::Rerun => 1,
            Self::FromStep(step) => step,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    #[error("invalid target: {0}")]
    InvalidTarget(String),
    /// Preflight refused to continue; nothing on the server was changed.
    #[error("blocked: {0}")]
    Blocked(BlockReason),
    #[error("unexpected output from `{command}`: {output}")]
    UnexpectedOutput { command: String, output: String },
    #[error("deploy lock is {0}")]
    LockHeld(LockInfo),
    #[error("could not take the deploy lock: {0}")]
    Lock(String),
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// What a deploy will do, produced by [`prepare`]. Only [`prepare`] can
/// build one, and a [`Confirmation`] can only be built from one.
#[derive(Debug, Clone)]
pub struct Preview {
    target: DeployTarget,
    from_sha: String,
    target_sha: String,
    commits: Vec<String>,
    recipe_hash: String,
    run_plan: RunPlan,
    run_id: String,
}

impl Preview {
    pub fn target(&self) -> &DeployTarget {
        &self.target
    }
    pub fn from_sha(&self) -> &str {
        &self.from_sha
    }
    pub fn target_sha(&self) -> &str {
        &self.target_sha
    }
    /// `git log --oneline FROM..TARGET`, newest first.
    pub fn commits(&self) -> &[String] {
        &self.commits
    }
    pub fn recipe_hash(&self) -> &str {
        &self.recipe_hash
    }
    pub fn run_plan(&self) -> RunPlan {
        self.run_plan
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfirmError {
    #[error("production deploys require typing the environment name `{0}`")]
    EnvNameRequired(String),
}

/// Proof that the user approved one specific [`Preview`]. Fields are private
/// and the only constructor takes the preview, so a deploy cannot run on a
/// confirmation for a different env, commit range, or recipe. Not `Clone`:
/// one confirmation runs one deploy.
#[derive(Debug, PartialEq, Eq)]
pub struct Confirmation {
    env: String,
    from_sha: String,
    target_sha: String,
    recipe_hash: String,
    run_id: String,
}

impl Confirmation {
    /// `typed_env` must equal the env name when the target is production.
    pub fn from(preview: &Preview, typed_env: Option<&str>) -> Result<Self, ConfirmError> {
        let env = &preview.target.env;
        if preview.target.production && typed_env != Some(env.as_str()) {
            return Err(ConfirmError::EnvNameRequired(env.clone()));
        }
        Ok(Self {
            env: env.clone(),
            from_sha: preview.from_sha.clone(),
            target_sha: preview.target_sha.clone(),
            recipe_hash: preview.recipe_hash.clone(),
            run_id: preview.run_id.clone(),
        })
    }

    fn matches(&self, preview: &Preview) -> bool {
        self.env == preview.target.env
            && self.from_sha == preview.from_sha
            && self.target_sha == preview.target_sha
            && self.recipe_hash == preview.recipe_hash
            && self.run_id == preview.run_id
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExecuteError {
    #[error("confirmation does not match this preview")]
    ConfirmationMismatch,
    #[error("receipt does not match this preview")]
    ReceiptMismatch,
    #[error("could not save the receipt: {0}")]
    Receipt(String),
}

/// Controls for a running deploy.
#[derive(Debug, Clone)]
pub struct ExecutionHandle {
    stop: Arc<AtomicBool>,
    detach: watch::Sender<bool>,
    watch_cancel: Arc<AtomicBool>,
}

impl ExecutionHandle {
    /// Let the running step finish, then stop before the next one.
    pub fn stop_after_step(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Stop observing now. The running step is left alone on the server.
    pub fn detach(&self) {
        let _ = self.detach.send(true);
    }

    /// Stop watching the Laravel log; the deploy outcome is unchanged.
    pub fn cancel_watch(&self) {
        self.watch_cancel.store(true, Ordering::SeqCst);
    }
}

/// Preflight: resolves the current and target commits, takes the deploy
/// lock, and builds the [`Preview`]. Changes nothing on the server except
/// fetching refs and taking the lock. Release it with [`cancel`] if the
/// deploy does not go ahead.
pub async fn prepare<T: Transport>(
    target: DeployTarget,
    transport: &T,
) -> Result<Preview, PrepareError> {
    prepare_with_plan(target, RunPlan::Deploy, transport).await
}

/// Prepares a deploy, recipe rerun, or recipe restart from a selected step.
/// Reruns require `origin/<branch>` to match the server's current clean HEAD.
pub async fn prepare_with_plan<T: Transport>(
    target: DeployTarget,
    run_plan: RunPlan,
    transport: &T,
) -> Result<Preview, PrepareError> {
    match run_plan {
        RunPlan::Deploy => {}
        RunPlan::Rerun if target.steps.is_empty() => {
            return Err(PrepareError::InvalidTarget(
                "rerun requires at least one recipe step".into(),
            ));
        }
        RunPlan::FromStep(step) if step == 0 || step > target.steps.len() => {
            return Err(PrepareError::InvalidTarget(format!(
                "from-step must be between 1 and {}",
                target.steps.len()
            )));
        }
        _ => {}
    }
    if !is_safe_branch(&target.branch) {
        return Err(PrepareError::InvalidTarget(format!(
            "branch name `{}` is not supported",
            target.branch
        )));
    }

    let script = preflight::preflight_script(&target.path, &target.branch, &target.steps);
    let (result, lines) = run_collect(transport, &script).await;
    let code = result?;
    if code != 0 {
        return Err(PrepareError::Blocked(BlockReason::CommandFailed {
            code,
            output: lines.join("\n"),
        }));
    }
    let checks = preflight::parse_preflight(&lines);
    let unexpected = || PrepareError::UnexpectedOutput {
        command: "preflight".into(),
        output: lines.join("\n"),
    };
    if !is_full_sha(&checks.state.head) {
        return Err(unexpected());
    }
    if let Some(reason) = preflight::block_reason(&checks, &target.branch, run_plan.is_rerun()) {
        return Err(PrepareError::Blocked(reason));
    }
    if !is_full_sha(&checks.target) {
        return Err(unexpected());
    }
    let (from_sha, target_sha, commits) = (checks.state.head, checks.target, checks.commits);

    let run_id = new_run_id();
    let owner = LockOwner::current(&run_id, &target_sha);
    match lock::acquire(transport, &target.path, &owner)
        .await
        .map_err(PrepareError::Lock)?
    {
        Acquire::Acquired => {}
        Acquire::Held(info) => return Err(PrepareError::LockHeld(info)),
    }

    let recipe_hash = recipe_hash(&target, run_plan);
    Ok(Preview {
        target,
        from_sha,
        target_sha,
        commits,
        recipe_hash,
        run_plan,
        run_id,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum BringUpError {
    #[error("deploy lock is {0}")]
    LockHeld(LockInfo),
    #[error("could not take the deploy lock: {0}")]
    Lock(String),
    #[error("`php artisan up` exited with {code}: {output}")]
    Failed { code: i32, output: String },
    #[error("could not confirm whether `php artisan up` completed; the deploy lock was kept")]
    Unknown,
    #[error("`php artisan up` was not started: {0}")]
    NotStarted(String),
    #[error("could not follow `php artisan up`: {0}; the deploy lock was kept")]
    Interrupted(String),
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// Runs `php artisan up`, e.g. after a failed deploy left the app in
/// maintenance mode. Holds the deploy lock meanwhile, so it cannot run
/// during a deploy. Returns the command's output.
pub async fn bring_app_up<T: Transport>(
    target: &DeployTarget,
    transport: &T,
) -> Result<Vec<String>, BringUpError> {
    let run_id = new_run_id();
    let owner = LockOwner::current(&run_id, "");
    match lock::acquire(transport, &target.path, &owner)
        .await
        .map_err(BringUpError::Lock)?
    {
        Acquire::Acquired => {}
        Acquire::Held(info) => return Err(BringUpError::LockHeld(info)),
    }
    let (line_tx, mut line_rx) = mpsc::unbounded_channel();
    let script = wrap_step(&target.path, "php artisan up");
    let run = {
        let tx = line_tx;
        let step_run_id = run_id.clone();
        async move { runner::run_step(transport, &step_run_id, "maintenance-up", &script, &tx).await }
    };
    let collect = async {
        let mut lines = Vec::new();
        while let Some(line) = line_rx.recv().await {
            lines.push(line);
        }
        lines
    };
    let heartbeat = async {
        loop {
            tokio::time::sleep(lock::HEARTBEAT_EVERY).await;
            lock::heartbeat(transport, &target.path, &run_id).await;
        }
    };
    let (result, lines) = tokio::select! {
        result = async { tokio::join!(run, collect) } => result,
        () = heartbeat => unreachable!("heartbeat never ends"),
    };
    match result {
        StepResult::Exited(0) => {
            lock::release(transport, &target.path, &run_id).await?;
            Ok(lines)
        }
        StepResult::Exited(code) => {
            lock::release(transport, &target.path, &run_id).await?;
            Err(BringUpError::Failed {
                code,
                output: lines.join("\n"),
            })
        }
        StepResult::NotStarted(reason) => {
            lock::release(transport, &target.path, &run_id).await?;
            Err(BringUpError::NotStarted(reason))
        }
        StepResult::Gone => Err(BringUpError::Unknown),
        StepResult::Interrupted(reason) => Err(BringUpError::Interrupted(reason)),
    }
}

/// Gives up a prepared deploy without running it, releasing its lock.
pub async fn cancel<T: Transport>(preview: Preview, transport: &T) -> Result<(), TransportError> {
    lock::release(transport, &preview.target.path, &preview.run_id).await
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum BreakLockError {
    #[error("breaking the lock requires typing the environment name `{0}`")]
    EnvNameRequired(String),
    #[error("no deploy lock is held")]
    NotHeld,
    #[error("the lock is live (last heartbeat {0}s ago); only a stale lock can be broken")]
    Live(u64),
    #[error("could not break the lock: {0}")]
    Failed(String),
}

/// Breaks a stale deploy lock (see [`LockInfo::is_stale`]). Check first,
/// e.g. with `slip shell`, that no step of the old run is still running.
pub async fn break_lock<T: Transport>(
    target: &DeployTarget,
    transport: &T,
    typed_env: &str,
) -> Result<(), BreakLockError> {
    if typed_env != target.env {
        return Err(BreakLockError::EnvNameRequired(target.env.clone()));
    }
    match lock::break_stale(transport, &target.path)
        .await
        .map_err(BreakLockError::Failed)?
    {
        Break::Broken => Ok(()),
        Break::NotHeld => Err(BreakLockError::NotHeld),
        Break::Live(age) => Err(BreakLockError::Live(age)),
    }
}

/// Runs the confirmed deploy in the background. Events arrive on the
/// returned receiver; the last one is `Finished`, `Detached` or `Interrupted`.
///
/// Must be called within a Tokio runtime.
pub fn execute<T: Transport>(
    preview: Preview,
    confirmation: Confirmation,
    transport: Arc<T>,
) -> Result<(mpsc::UnboundedReceiver<DeployEvent>, ExecutionHandle), ExecuteError> {
    execute_inner(preview, confirmation, transport, None)
}

/// Executes a confirmed run while journaling each mutation before it starts.
pub fn execute_recorded<T: Transport>(
    preview: Preview,
    confirmation: Confirmation,
    transport: Arc<T>,
    journal: Arc<ReceiptJournal>,
) -> Result<(mpsc::UnboundedReceiver<DeployEvent>, ExecutionHandle), ExecuteError> {
    execute_inner(preview, confirmation, transport, Some(journal))
}

fn execute_inner<T: Transport>(
    preview: Preview,
    confirmation: Confirmation,
    transport: Arc<T>,
    journal: Option<Arc<ReceiptJournal>>,
) -> Result<(mpsc::UnboundedReceiver<DeployEvent>, ExecutionHandle), ExecuteError> {
    if !confirmation.matches(&preview) {
        return Err(ExecuteError::ConfirmationMismatch);
    }
    if let Some(journal) = &journal {
        let receipt = journal.snapshot();
        if receipt.run_id != preview.run_id
            || receipt.recipe_hash != preview.recipe_hash
            || receipt.target != preview.target
            || receipt.run_plan != preview.run_plan
            || receipt.from_sha != preview.from_sha
            || receipt.target_sha != preview.target_sha
            || receipt.status != ReceiptStatus::InProgress
            || receipt.phase != ReceiptPhase::Preview
            || receipt.mutation_started
            || receipt.outcome.is_some()
        {
            return Err(ExecuteError::ReceiptMismatch);
        }
        journal
            .confirm()
            .map_err(|error| ExecuteError::Receipt(error.to_string()))?;
    }

    let (events, rx) = mpsc::unbounded_channel();
    let (detach_tx, detach_rx) = watch::channel(false);
    let handle = ExecutionHandle {
        stop: Arc::new(AtomicBool::new(false)),
        detach: detach_tx,
        watch_cancel: Arc::new(AtomicBool::new(false)),
    };
    let stop = handle.stop.clone();
    let watch_cancel = handle.watch_cancel.clone();

    tokio::spawn(run_steps(
        preview,
        transport,
        events,
        stop,
        detach_rx,
        journal,
        None,
        watch_cancel,
    ));
    Ok((rx, handle))
}

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("the receipt is already final")]
    Final,
    #[error("the receipt is invalid: {0}")]
    Invalid(String),
    #[error("could not update the receipt: {0}")]
    Receipt(String),
}

/// Resumes observation of an unfinished run; an active step is never launched again.
pub fn attach<T: Transport>(
    journal: Arc<ReceiptJournal>,
    transport: Arc<T>,
) -> Result<(mpsc::UnboundedReceiver<DeployEvent>, ExecutionHandle), AttachError> {
    journal
        .claim()
        .map_err(|error| AttachError::Receipt(error.to_string()))?;
    let receipt = journal.snapshot();
    if receipt.status != ReceiptStatus::InProgress {
        return Err(AttachError::Final);
    }
    if receipt.mutation_started && !receipt.confirmed {
        return Err(AttachError::Invalid(
            "mutation began without confirmation".into(),
        ));
    }
    if !is_full_sha(&receipt.from_sha)
        || !is_full_sha(&receipt.target_sha)
        || recipe_hash(&receipt.target, receipt.run_plan) != receipt.recipe_hash
    {
        return Err(AttachError::Invalid(
            "the saved deploy plan is inconsistent".into(),
        ));
    }
    validate_receipt_plan(&receipt).map_err(AttachError::Invalid)?;
    let preview = Preview {
        target: receipt.target.clone(),
        from_sha: receipt.from_sha.clone(),
        target_sha: receipt.target_sha.clone(),
        commits: receipt.commits.clone(),
        recipe_hash: receipt.recipe_hash.clone(),
        run_plan: receipt.run_plan,
        run_id: receipt.run_id.clone(),
    };
    journal
        .set_owner_pid(std::process::id())
        .map_err(|error| AttachError::Receipt(error.to_string()))?;
    let (events, rx) = mpsc::unbounded_channel();
    let (detach_tx, detach_rx) = watch::channel(false);
    let handle = ExecutionHandle {
        stop: Arc::new(AtomicBool::new(false)),
        detach: detach_tx,
        watch_cancel: Arc::new(AtomicBool::new(false)),
    };
    let stop = handle.stop.clone();
    let watch_cancel = handle.watch_cancel.clone();
    tokio::spawn(run_steps(
        preview,
        transport,
        events,
        stop,
        detach_rx,
        Some(journal),
        Some(receipt),
        watch_cancel,
    ));
    Ok((rx, handle))
}

fn validate_receipt_plan(receipt: &Receipt) -> Result<(), String> {
    let first = match receipt.run_plan {
        RunPlan::Deploy | RunPlan::Rerun => 1,
        RunPlan::FromStep(step) if step > 0 && step <= receipt.target.steps.len() => step,
        RunPlan::FromStep(_) => return Err("invalid saved recipe step".into()),
    };
    if receipt.run_plan == RunPlan::Rerun && receipt.target.steps.is_empty() {
        return Err("saved rerun has no recipe steps".into());
    }
    let expected_len =
        receipt.target.steps.len() + usize::from(receipt.run_plan == RunPlan::Deploy);
    if receipt.steps.len() != expected_len {
        return Err("saved step list does not match the plan".into());
    }
    for (position, step) in receipt.steps.iter().enumerate() {
        let expected_index = if receipt.run_plan == RunPlan::Deploy {
            position
        } else {
            position + 1
        };
        let expected_command = if expected_index == 0 {
            format!("git merge --ff-only {}", receipt.target_sha)
        } else {
            receipt.target.steps[expected_index - 1].clone()
        };
        if step.index != expected_index || step.command != expected_command {
            return Err("saved step list does not match the plan".into());
        }
        if expected_index > 0 && expected_index < first && step.status != ReceiptStepStatus::Skipped
        {
            return Err("saved skipped steps are inconsistent".into());
        }
        if expected_index >= first && step.status == ReceiptStepStatus::Skipped {
            return Err("saved step status is inconsistent".into());
        }
    }
    let mut before_next = true;
    for step in receipt
        .steps
        .iter()
        .filter(|step| step.status != ReceiptStepStatus::Skipped)
    {
        match step.status {
            ReceiptStepStatus::Ok if before_next => {}
            ReceiptStepStatus::Ok => return Err("saved completed steps are out of order".into()),
            ReceiptStepStatus::Pending => before_next = false,
            ReceiptStepStatus::Running
            | ReceiptStepStatus::Failed
            | ReceiptStepStatus::Unknown
            | ReceiptStepStatus::NotStarted => {
                if !before_next {
                    return Err("saved step statuses are out of order".into());
                }
                before_next = false;
            }
            ReceiptStepStatus::Skipped => unreachable!(),
        }
    }
    let running = receipt
        .steps
        .iter()
        .filter(|step| step.status == ReceiptStepStatus::Running)
        .count();
    match receipt.phase {
        ReceiptPhase::Preview
            if !receipt.mutation_started
                && running == 0
                && receipt.steps.iter().all(|step| {
                    matches!(
                        step.status,
                        ReceiptStepStatus::Pending | ReceiptStepStatus::Skipped
                    )
                }) => {}
        ReceiptPhase::MaintenanceDown
            if receipt.target.maintenance
                && receipt.mutation_started
                && running == 0
                && receipt.maintenance_down_status.is_none()
                && receipt.steps.iter().all(|step| {
                    matches!(
                        step.status,
                        ReceiptStepStatus::Pending | ReceiptStepStatus::Skipped
                    )
                }) => {}
        ReceiptPhase::Step(index)
            if receipt.mutation_started
                && running == 1
                && (!receipt.target.maintenance
                    || receipt.maintenance_down_status == Some(StepStatus::Ok))
                && receipt.steps.iter().any(|step| {
                    step.index == index && step.status == ReceiptStepStatus::Running
                }) => {}
        ReceiptPhase::MaintenanceUp
            if receipt.target.maintenance
                && receipt.mutation_started
                && receipt.app_left_down
                && running == 0
                && receipt.maintenance_up_status.is_none()
                && receipt.steps.iter().all(|step| {
                    matches!(
                        step.status,
                        ReceiptStepStatus::Ok | ReceiptStepStatus::Skipped
                    )
                }) => {}
        ReceiptPhase::BetweenSteps if receipt.mutation_started && running == 0 => {}
        _ => return Err("saved phase does not match step status".into()),
    }
    Ok(())
}

/// How a run ends: with an outcome, or leaving a command on the server.
enum End {
    Finished(DeployOutcome),
    Detached(usize),
    Interrupted { index: usize, reason: String },
}

#[allow(clippy::too_many_arguments)]
async fn run_steps<T: Transport>(
    preview: Preview,
    transport: Arc<T>,
    events: mpsc::UnboundedSender<DeployEvent>,
    stop: Arc<AtomicBool>,
    detach: watch::Receiver<bool>,
    journal: Option<Arc<ReceiptJournal>>,
    resume: Option<Receipt>,
    watch_cancel: Arc<AtomicBool>,
) {
    let (path, run_id) = (&preview.target.path, &preview.run_id);
    let saved_checks = resume.as_ref().and_then(|receipt| {
        receipt.outcome.as_ref()?;
        Some((receipt.watch.clone()?, receipt.smoke.clone()?))
    });
    let using_saved_checks = saved_checks.is_some();
    let observer = if preview.target.watch_log && saved_checks.is_none() {
        Some(
            LogObserver::start(
                transport.clone(),
                &preview.target,
                events.clone(),
                journal.as_ref().map(|receipt| receipt.history_path()),
                watch_cancel,
            )
            .await,
        )
    } else {
        None
    };
    let heartbeat = async {
        loop {
            tokio::time::sleep(lock::HEARTBEAT_EVERY).await;
            lock::heartbeat(&*transport, path, run_id).await;
        }
    };
    let mut app_down = resume.as_ref().is_some_and(|receipt| receipt.app_left_down);
    let end = tokio::select! {
        end = run_steps_locked(&preview, &*transport, &events, &stop, detach, &mut app_down, journal.as_deref(), resume.as_ref(), observer.as_ref()) => end,
        () = heartbeat => unreachable!("heartbeat never ends"),
    };
    if app_down {
        let _ = events.send(DeployEvent::AppLeftDown);
    }
    let post_checks = matches!(&end, End::Finished(outcome) if should_run_post_checks(outcome));
    let watch_status = if post_checks {
        WatchStatus::Complete
    } else if matches!(
        &end,
        End::Finished(
            DeployOutcome::CancelledBeforeChanges | DeployOutcome::AbortedBeforeChanges(_)
        )
    ) {
        WatchStatus::NotRun
    } else {
        WatchStatus::Partial
    };
    let checks = async {
        let watch = async {
            match observer {
                Some(observer) => observer.finish(watch_status, post_window()).await,
                None => WatchResult::not_run(),
            }
        };
        let smoke = async {
            let result = if post_checks {
                observation::smoke_check(preview.target.smoke_url.as_deref()).await
            } else if preview.target.smoke_url.is_some() {
                SmokeResult::Skipped
            } else {
                SmokeResult::NotConfigured
            };
            if preview.target.smoke_url.is_some() {
                let _ = events.send(DeployEvent::SmokeFinished(result.clone()));
            }
            result
        };
        tokio::join!(watch, smoke)
    };
    let (mut watch_result, smoke_result) = if let Some(saved) = saved_checks {
        saved
    } else {
        tokio::select! {
            results = checks => results,
            () = async {
                loop {
                    tokio::time::sleep(lock::HEARTBEAT_EVERY).await;
                    lock::heartbeat(&*transport, path, run_id).await;
                }
            } => unreachable!("heartbeat never ends"),
        }
    };
    if resume.is_some() && !using_saved_checks && watch_result.status == WatchStatus::Complete {
        watch_result.status = WatchStatus::Partial;
        watch_result.warnings.push(
            "Log watch restarted after attach; entries during the interruption may be missing"
                .into(),
        );
    }
    if let Some(previous) = resume.as_ref().and_then(|receipt| receipt.watch.as_ref()) {
        if resume
            .as_ref()
            .is_some_and(|receipt| receipt.outcome.is_none())
        {
            merge_previous_watch(&mut watch_result, previous);
            watch_result.status = WatchStatus::Partial;
            watch_result
                .warnings
                .push("Previous observation was interrupted".into());
        }
    }
    if let Some(journal) = &journal {
        if let Err(error) = journal.observation(watch_result.clone(), smoke_result.clone()) {
            let _ = events.send(DeployEvent::Interrupted {
                index: preview.target.steps.len() + 1,
                reason: format!("could not save post-deploy checks: {error}"),
            });
            return;
        }
    }
    if preview.target.watch_log {
        let _ = events.send(DeployEvent::WatchFinished(watch_result));
    }
    if using_saved_checks && preview.target.smoke_url.is_some() {
        let _ = events.send(DeployEvent::SmokeFinished(smoke_result));
    }
    let event = match end {
        End::Finished(outcome) => {
            if let Some(journal) = &journal {
                if let Err(error) = journal.record_outcome(outcome.clone()) {
                    let _ = events.send(DeployEvent::Interrupted {
                        index: preview.target.steps.len() + 1,
                        reason: format!("could not save the outcome: {error}"),
                    });
                    return;
                }
            }
            // After an unknown outcome the server's state needs checking
            // first, so the lock is kept and goes stale.
            if !matches!(outcome, DeployOutcome::Unknown { .. }) {
                if let Err(error) = lock::release(&*transport, path, run_id).await {
                    if journal.is_some() {
                        let _ = events.send(DeployEvent::Interrupted {
                            index: preview.target.steps.len() + 1,
                            reason: format!("could not release deploy lock: {error}"),
                        });
                        return;
                    }
                }
            }
            if let Some(journal) = &journal {
                if let Err(error) = journal.finish(outcome.clone()) {
                    let _ = events.send(DeployEvent::Interrupted {
                        index: preview.target.steps.len() + 1,
                        reason: format!("could not finalize the receipt: {error}"),
                    });
                    return;
                }
            }
            DeployEvent::Finished(outcome)
        }
        End::Detached(index) => {
            if let Some(journal) = &journal {
                let _ = journal.message(format!("detached while observing step {index}"));
            }
            DeployEvent::Detached { index }
        }
        End::Interrupted { index, reason } => {
            if let Some(journal) = &journal {
                let _ = journal.message(reason.clone());
            }
            DeployEvent::Interrupted { index, reason }
        }
    };
    // A caller may attach as soon as it receives Detached or Interrupted.
    // Release the local claim before reporting that the observer has stopped.
    drop(journal);
    let _ = events.send(event);
}

fn should_run_post_checks(outcome: &DeployOutcome) -> bool {
    match outcome {
        DeployOutcome::Succeeded | DeployOutcome::StoppedAfterStep { .. } => true,
        DeployOutcome::FailedAtStep {
            step,
            partial_update,
        } => *step > 0 || *partial_update,
        _ => false,
    }
}

fn merge_previous_watch(current: &mut WatchResult, previous: &WatchResult) {
    current.duration_ms = current.duration_ms.saturating_add(previous.duration_ms);
    current.observed_lines = current
        .observed_lines
        .saturating_add(previous.observed_lines);
    current.parsed_lines = current.parsed_lines.saturating_add(previous.parsed_lines);
    current.dropped_view_lines = current
        .dropped_view_lines
        .saturating_add(previous.dropped_view_lines);
    current.truncated_entries = current
        .truncated_entries
        .saturating_add(previous.truncated_entries);
    current.overflow_groups = current
        .overflow_groups
        .saturating_add(previous.overflow_groups);
    current.overflow_signatures = current
        .overflow_signatures
        .saturating_add(previous.overflow_signatures);
    for group in &previous.new_errors {
        if let Some(existing) = current
            .new_errors
            .iter_mut()
            .find(|item| item.exception == group.exception && item.file == group.file)
        {
            existing.count = existing.count.saturating_add(group.count);
            existing.overflow_variants = existing
                .overflow_variants
                .saturating_add(group.overflow_variants);
            for variant in &group.variants {
                if let Some(saved) = existing
                    .variants
                    .iter_mut()
                    .find(|item| item.signature == variant.signature && item.phase == variant.phase)
                {
                    saved.count = saved.count.saturating_add(variant.count);
                } else if existing.variants.len() < 50 {
                    existing.variants.push(variant.clone());
                } else {
                    existing.overflow_variants += 1;
                }
            }
        } else if current.new_errors.len() < 500 {
            current.new_errors.push(group.clone());
        } else {
            current.overflow_groups += 1;
        }
    }
    for warning in &previous.warnings {
        if current.warnings.len() >= 20 {
            break;
        }
        if !current.warnings.contains(warning) {
            current.warnings.push(warning.clone());
        }
    }
}

fn post_window() -> std::time::Duration {
    #[cfg(test)]
    {
        std::time::Duration::from_millis(100)
    }
    #[cfg(not(test))]
    {
        std::time::Duration::from_secs(120)
    }
}

/// Runs maintenance down, the steps, and maintenance up. `app_down` is left
/// set if maintenance mode may still be on.
#[allow(clippy::too_many_arguments)]
async fn run_steps_locked<T: Transport>(
    preview: &Preview,
    transport: &T,
    events: &mpsc::UnboundedSender<DeployEvent>,
    stop: &AtomicBool,
    mut detach: watch::Receiver<bool>,
    app_down: &mut bool,
    journal: Option<&ReceiptJournal>,
    resume: Option<&Receipt>,
    observer: Option<&LogObserver>,
) -> End {
    let path = &preview.target.path;
    let first_recipe_step = preview.run_plan.first_recipe_step();
    let mut steps = Vec::new();
    if preview.run_plan == RunPlan::Deploy {
        steps.push((
            0,
            "git fast-forward".to_string(),
            format!("git merge --ff-only {}", preview.target_sha),
        ));
    }
    steps.extend(
        preview
            .target
            .steps
            .iter()
            .enumerate()
            .filter_map(|(step, body)| {
                let index = step + 1;
                (index >= first_recipe_step).then(|| (index, body.clone(), body.clone()))
            }),
    );
    let first_index = steps.first().map(|(index, _, _)| *index).unwrap_or(0);
    let mut watching = false;

    if let Some(receipt) = resume {
        if let Some(outcome) = &receipt.outcome {
            return End::Finished(outcome.clone());
        }
        if !receipt.mutation_started {
            return End::Finished(DeployOutcome::CancelledBeforeChanges);
        }
        if let Some(status) = &receipt.maintenance_down_status {
            match status {
                StepStatus::Failed => {
                    return End::Finished(DeployOutcome::AbortedBeforeChanges(
                        AbortReason::MaintenanceDownFailed(
                            receipt.maintenance_down_exit_code.unwrap_or(-1),
                        ),
                    ));
                }
                StepStatus::Unknown => {
                    return End::Finished(DeployOutcome::Unknown {
                        step: 0,
                        reason: "maintenance down ended without a known result".into(),
                    });
                }
                StepStatus::NotStarted => {
                    return End::Finished(DeployOutcome::AbortedBeforeChanges(
                        AbortReason::ConnectFailed("maintenance down was not started".into()),
                    ));
                }
                StepStatus::Ok => {}
            }
        }
        if let Some(step) = receipt.steps.iter().find(|step| {
            matches!(
                step.status,
                ReceiptStepStatus::Failed
                    | ReceiptStepStatus::Unknown
                    | ReceiptStepStatus::NotStarted
            )
        }) {
            return End::Finished(match step.status {
                ReceiptStepStatus::Failed => DeployOutcome::FailedAtStep {
                    step: step.index,
                    partial_update: step.index == 0
                        && (receipt.server_head_at_end.as_deref() != Some(&receipt.from_sha)
                            || receipt.tree_dirty != Some(false)),
                },
                ReceiptStepStatus::Unknown => DeployOutcome::Unknown {
                    step: step.index,
                    reason: "the step ended without a known result".into(),
                },
                ReceiptStepStatus::NotStarted => stopped_before_plan(
                    step.index,
                    first_index,
                    preview.run_plan,
                    StopReason::ConnectFailed("the step was not started".into()),
                ),
                _ => unreachable!(),
            });
        }
        if let Some(status) = &receipt.maintenance_up_status {
            return End::Finished(match status {
                StepStatus::Unknown => DeployOutcome::Unknown {
                    step: preview.target.steps.len() + 1,
                    reason: "maintenance up ended without a known result".into(),
                },
                StepStatus::Ok | StepStatus::Failed | StepStatus::NotStarted => {
                    DeployOutcome::Succeeded
                }
            });
        }
    }

    for (index, name, body) in steps {
        if resume.is_some_and(|receipt| {
            receipt
                .steps
                .iter()
                .any(|step| step.index == index && step.status == ReceiptStepStatus::Ok)
        }) {
            continue;
        }
        let active_step = resume.is_some_and(|receipt| receipt.phase == ReceiptPhase::Step(index));
        let active_down = index == first_index
            && resume.is_some_and(|receipt| receipt.phase == ReceiptPhase::MaintenanceDown);
        // Detaching between steps leaves nothing running, so it is a stop.
        if stop.load(Ordering::SeqCst) || *detach.borrow() {
            return End::Finished(stopped_before_plan(
                index,
                first_index,
                preview.run_plan,
                StopReason::Requested,
            ));
        }
        match if active_step || active_down {
            Ok(true)
        } else {
            lock_owned(transport, path, &preview.run_id).await
        } {
            Ok(true) => {}
            Ok(false) => {
                return End::Finished(stopped_before_plan(
                    index,
                    first_index,
                    preview.run_plan,
                    StopReason::LockLost,
                ))
            }
            Err(reason) => {
                return End::Finished(stopped_before_plan(
                    index,
                    first_index,
                    preview.run_plan,
                    StopReason::ConnectFailed(reason),
                ))
            }
        }
        if !active_step
            && resume.is_some_and(|receipt| {
                preview.run_plan == RunPlan::Deploy
                    && index == 1
                    && receipt
                        .steps
                        .iter()
                        .any(|step| step.index == 0 && step.status == ReceiptStepStatus::Ok)
            })
        {
            match read_state(transport, path).await {
                Ok(state) if state.head == preview.target_sha && state.dirty.is_empty() => {}
                Ok(_) => {
                    return End::Finished(DeployOutcome::Unknown {
                        step: 0,
                        reason: "checkout changed after the recorded fast-forward".into(),
                    });
                }
                Err(reason) => {
                    return End::Interrupted {
                        index,
                        reason: format!("could not verify checkout after fast-forward: {reason:?}"),
                    }
                }
            }
        }
        if index == first_index && !active_step {
            if !active_down {
                let changed = match read_state(transport, path).await {
                    Ok(state) => preflight::recheck(&state, &preview.from_sha),
                    Err(reason) => Some(reason),
                };
                if let Some(reason) = changed {
                    return End::Finished(DeployOutcome::AbortedBeforeChanges(reason));
                }
            }
            if let Some(observer) = observer {
                observer.begin();
                watching = true;
            }
            if preview.target.maintenance
                && !resume.is_some_and(|receipt| receipt.maintenance_down_done)
            {
                if !active_down {
                    if let Some(journal) = journal {
                        if let Err(error) = journal.begin_maintenance(MaintenancePhase::Down) {
                            return End::Finished(DeployOutcome::AbortedBeforeChanges(
                                AbortReason::JournalFailed(error.to_string()),
                            ));
                        }
                    }
                }
                let down = maintenance(
                    transport,
                    preview,
                    MaintenancePhase::Down,
                    events,
                    &mut detach,
                    true,
                    if active_down {
                        FollowMode::Attach
                    } else {
                        FollowMode::Launch
                    },
                    journal,
                )
                .await;
                if let (Some(journal), Some(result)) = (journal, down.as_ref()) {
                    if !matches!(result, StepResult::Interrupted(_)) {
                        let (status, code) = result_status(result);
                        if let Err(error) =
                            journal.finish_maintenance(MaintenancePhase::Down, status, code)
                        {
                            return End::Interrupted {
                                index: 0,
                                reason: error.to_string(),
                            };
                        }
                    }
                }
                match down {
                    None => return End::Detached(0),
                    Some(StepResult::Exited(0)) => *app_down = true,
                    Some(StepResult::Exited(code)) => {
                        *app_down = false;
                        let reason = AbortReason::MaintenanceDownFailed(code);
                        return End::Finished(DeployOutcome::AbortedBeforeChanges(reason));
                    }
                    Some(StepResult::NotStarted(reason)) => {
                        *app_down = false;
                        let reason = AbortReason::ConnectFailed(reason);
                        return End::Finished(DeployOutcome::AbortedBeforeChanges(reason));
                    }
                    Some(StepResult::Gone) => {
                        return End::Finished(DeployOutcome::Unknown {
                            step: 0,
                            reason: "`php artisan down` ended without recording an exit code"
                                .into(),
                        })
                    }
                    Some(StepResult::Interrupted(reason)) => {
                        return End::Interrupted { index: 0, reason }
                    }
                }
                match lock_owned(transport, path, &preview.run_id).await {
                    Ok(true) => {}
                    Ok(false) => {
                        return End::Finished(DeployOutcome::AbortedBeforeChanges(
                            AbortReason::LockLost,
                        ))
                    }
                    Err(reason) => return End::Interrupted { index: 0, reason },
                }
                if stop.load(Ordering::SeqCst) || *detach.borrow() {
                    return End::Finished(stopped_before_plan(
                        index,
                        first_index,
                        preview.run_plan,
                        StopReason::Requested,
                    ));
                }
            }
        }

        if !watching {
            if let Some(observer) = observer {
                observer.begin();
                watching = true;
            }
        }
        if !active_step {
            if let Some(journal) = journal {
                if let Err(error) = journal.begin_step(index) {
                    return End::Finished(stopped_before_plan(
                        index,
                        first_index,
                        preview.run_plan,
                        StopReason::JournalFailed(error.to_string()),
                    ));
                }
            }
        }
        let _ = events.send(DeployEvent::StepStarted { index, name });
        let key = format!("step-{index}");
        let output = |line| DeployEvent::Output { index, line };
        let Some(result) = follow(
            transport,
            preview,
            &key,
            &body,
            events,
            Some(&mut detach),
            output,
            if active_step {
                FollowMode::Attach
            } else {
                FollowMode::Launch
            },
            journal,
        )
        .await
        else {
            return End::Detached(index);
        };

        let mut server_state = None;
        let (status, exit_code, outcome) = match result {
            StepResult::Exited(0) => (StepStatus::Ok, Some(0), None),
            StepResult::Exited(code) => {
                // A failed fast-forward can still leave some files updated.
                let partial_update = index == 0 && {
                    let state = read_state(transport, path).await.ok();
                    let head = state.as_ref().map(|s| s.head.clone());
                    let tree_dirty = state.as_ref().map(|s| !s.dirty.is_empty());
                    let unchanged = head.as_deref() == Some(preview.from_sha.as_str())
                        && tree_dirty == Some(false);
                    server_state = Some(DeployEvent::ServerState { head, tree_dirty });
                    !unchanged
                };
                (
                    StepStatus::Failed,
                    Some(code),
                    Some(DeployOutcome::FailedAtStep {
                        step: index,
                        partial_update,
                    }),
                )
            }
            StepResult::NotStarted(reason) => (
                StepStatus::NotStarted,
                None,
                Some(stopped_before_plan(
                    index,
                    first_index,
                    preview.run_plan,
                    StopReason::ConnectFailed(reason),
                )),
            ),
            StepResult::Gone => (
                StepStatus::Unknown,
                None,
                Some(DeployOutcome::Unknown {
                    step: index,
                    reason: "the step's process ended without recording an exit code".into(),
                }),
            ),
            StepResult::Interrupted(reason) => return End::Interrupted { index, reason },
        };
        if let Some(journal) = journal {
            if let Some(DeployEvent::ServerState { head, tree_dirty }) = &server_state {
                if let Err(error) = journal.server_state(head.clone(), *tree_dirty) {
                    return End::Interrupted {
                        index,
                        reason: error.to_string(),
                    };
                }
            }
            if let Err(error) = journal.finish_step(index, status.clone(), exit_code) {
                return End::Interrupted {
                    index,
                    reason: error.to_string(),
                };
            }
        }
        let _ = events.send(DeployEvent::StepFinished {
            index,
            status,
            exit_code,
        });
        if let Some(event) = server_state {
            let _ = events.send(event);
        }
        if let Some(outcome) = outcome {
            return End::Finished(outcome);
        }
    }

    if *app_down {
        if !watching {
            if let Some(observer) = observer {
                observer.begin();
            }
        }
        let active_up = resume.is_some_and(|receipt| receipt.phase == ReceiptPhase::MaintenanceUp);
        if !active_up {
            match lock_owned(transport, path, &preview.run_id).await {
                Ok(true) => {}
                Ok(false) => {
                    return End::Finished(DeployOutcome::Unknown {
                        step: preview.target.steps.len() + 1,
                        reason: "deploy lock was lost before maintenance up".into(),
                    });
                }
                Err(reason) => {
                    return End::Interrupted {
                        index: preview.target.steps.len() + 1,
                        reason,
                    };
                }
            }
            if let Some(journal) = journal {
                if let Err(error) = journal.begin_maintenance(MaintenancePhase::Up) {
                    return End::Interrupted {
                        index: preview.target.steps.len() + 1,
                        reason: error.to_string(),
                    };
                }
            }
        }
        let up = maintenance(
            transport,
            preview,
            MaintenancePhase::Up,
            events,
            &mut detach,
            false,
            if active_up {
                FollowMode::Attach
            } else {
                FollowMode::Launch
            },
            journal,
        )
        .await;
        if let (Some(journal), Some(result)) = (journal, up.as_ref()) {
            if !matches!(result, StepResult::Interrupted(_)) {
                let (status, code) = result_status(result);
                if let Err(error) = journal.finish_maintenance(MaintenancePhase::Up, status, code) {
                    return End::Interrupted {
                        index: preview.target.steps.len() + 1,
                        reason: error.to_string(),
                    };
                }
            }
        }
        match up {
            Some(StepResult::Exited(0)) => *app_down = false,
            Some(StepResult::Gone) => {
                return End::Finished(DeployOutcome::Unknown {
                    step: preview.target.steps.len() + 1,
                    reason: "maintenance up ended without recording an exit code".into(),
                });
            }
            Some(StepResult::Interrupted(reason)) => {
                return End::Interrupted {
                    index: preview.target.steps.len() + 1,
                    reason,
                };
            }
            None => return End::Detached(preview.target.steps.len() + 1),
            Some(StepResult::Exited(_) | StepResult::NotStarted(_)) => {}
        }
    }
    End::Finished(DeployOutcome::Succeeded)
}

/// Runs `body` detached on the server as `key`, turning its output lines
/// into events. `None` if the front-end detached meanwhile.
#[derive(Clone, Copy)]
enum FollowMode {
    Launch,
    Attach,
}

#[allow(clippy::too_many_arguments)]
async fn follow<T: Transport>(
    transport: &T,
    preview: &Preview,
    key: &str,
    body: &str,
    events: &mpsc::UnboundedSender<DeployEvent>,
    detach: Option<&mut watch::Receiver<bool>>,
    to_event: impl Fn(String) -> DeployEvent,
    mode: FollowMode,
    journal: Option<&ReceiptJournal>,
) -> Option<StepResult> {
    let (line_tx, mut line_rx) = mpsc::unbounded_channel();
    let step = async {
        let tx = line_tx;
        match mode {
            FollowMode::Launch => {
                let script = wrap_step(&preview.target.path, body);
                runner::run_step(transport, &preview.run_id, key, &script, &tx).await
            }
            FollowMode::Attach => runner::attach_step(transport, &preview.run_id, key, &tx).await,
        }
    };
    // All output is delivered before the caller reports the result.
    let forward = async {
        while let Some(line) = line_rx.recv().await {
            if let Some(journal) = journal {
                journal.output_line(key, &line);
            }
            let _ = events.send(to_event(line));
        }
    };
    if let Some(detach) = detach {
        tokio::select! {
            biased;
            _ = wait_for_detach(detach) => None,
            (result, ()) = async { tokio::join!(step, forward) } => Some(result),
        }
    } else {
        let (result, ()) = tokio::join!(step, forward);
        Some(result)
    }
}

#[allow(clippy::too_many_arguments)]
async fn maintenance<T: Transport>(
    transport: &T,
    preview: &Preview,
    phase: MaintenancePhase,
    events: &mpsc::UnboundedSender<DeployEvent>,
    detach: &mut watch::Receiver<bool>,
    detachable: bool,
    mode: FollowMode,
    journal: Option<&ReceiptJournal>,
) -> Option<StepResult> {
    let (key, command) = match phase {
        MaintenancePhase::Down => ("maintenance-down", "php artisan down"),
        MaintenancePhase::Up => ("maintenance-up", "php artisan up"),
    };
    let _ = events.send(DeployEvent::MaintenanceStarted { phase });
    let output = |line| DeployEvent::MaintenanceOutput { phase, line };
    let Some(result) = follow(
        transport,
        preview,
        key,
        command,
        events,
        if detachable { Some(detach) } else { None },
        output,
        mode,
        journal,
    )
    .await
    else {
        let _ = events.send(DeployEvent::MaintenanceFinished {
            phase,
            status: StepStatus::Unknown,
            exit_code: None,
        });
        return None;
    };
    let (status, exit_code) = result_status(&result);
    let _ = events.send(DeployEvent::MaintenanceFinished {
        phase,
        status,
        exit_code,
    });
    Some(result)
}

fn result_status(result: &StepResult) -> (StepStatus, Option<i32>) {
    match result {
        StepResult::Exited(0) => (StepStatus::Ok, Some(0)),
        StepResult::Exited(code) => (StepStatus::Failed, Some(*code)),
        StepResult::NotStarted(_) => (StepStatus::NotStarted, None),
        StepResult::Gone | StepResult::Interrupted(_) => (StepStatus::Unknown, None),
    }
}

/// The outcome of stopping before step `index` starts.
fn stopped_before(index: usize, reason: StopReason) -> DeployOutcome {
    match (index, reason) {
        (0, StopReason::Requested) => DeployOutcome::CancelledBeforeChanges,
        (0, StopReason::LockLost) => DeployOutcome::AbortedBeforeChanges(AbortReason::LockLost),
        (0, StopReason::ConnectFailed(reason)) => {
            DeployOutcome::AbortedBeforeChanges(AbortReason::ConnectFailed(reason))
        }
        (0, StopReason::JournalFailed(reason)) => {
            DeployOutcome::AbortedBeforeChanges(AbortReason::JournalFailed(reason))
        }
        (n, reason) => DeployOutcome::StoppedAfterStep {
            step: n - 1,
            reason,
        },
    }
}

fn stopped_before_plan(
    index: usize,
    first_index: usize,
    run_plan: RunPlan,
    reason: StopReason,
) -> DeployOutcome {
    if run_plan.is_rerun() && index == first_index {
        stopped_before(0, reason)
    } else {
        stopped_before(index, reason)
    }
}

/// Reads the checkout's state, reconnecting once if the server did not answer.
async fn read_state<T: Transport>(transport: &T, path: &str) -> Result<State, AbortReason> {
    let script = preflight::state_script(path);
    let mut run = run_collect(transport, &script).await;
    if run.0.is_err() {
        let _ = transport.reconnect().await;
        run = run_collect(transport, &script).await;
    }
    match run {
        (Ok(0), lines) => {
            let state = preflight::parse_state(&lines);
            if is_full_sha(&state.head) {
                Ok(state)
            } else {
                Err(AbortReason::CheckFailed(lines.join("\n")))
            }
        }
        (Ok(code), lines) => Err(AbortReason::CheckFailed(format!(
            "exited with {code}: {}",
            lines.join("\n")
        ))),
        (Err(e), _) => Err(AbortReason::ConnectFailed(e.to_string())),
    }
}

/// Checks lock ownership, reconnecting once if the server did not answer.
async fn lock_owned<T: Transport>(transport: &T, path: &str, run_id: &str) -> Result<bool, String> {
    match lock::is_owned(transport, path, run_id).await {
        Err(_) => {
            let _ = transport.reconnect().await;
            lock::is_owned(transport, path, run_id).await
        }
        owned => owned,
    }
}

async fn wait_for_detach(detach: &mut watch::Receiver<bool>) {
    if detach.wait_for(|d| *d).await.is_err() {
        // Handle dropped without detaching: never resolve.
        std::future::pending::<()>().await;
    }
}

fn recipe_hash(target: &DeployTarget, run_plan: RunPlan) -> String {
    let mut h = Sha256::new();
    let mut field = |bytes: &[u8]| {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    for part in [&target.env, &target.ssh_alias, &target.path, &target.branch] {
        field(part.as_bytes());
    }
    field(&[target.production as u8, target.maintenance as u8]);
    // Receipts written before observation settings were added keep their
    // original hash, so an interrupted deploy remains attachable on upgrade.
    if target.watch_log || target.log.is_some() || target.log_daily || target.smoke_url.is_some() {
        field(&[target.watch_log as u8, target.log_daily as u8]);
        field(target.log.as_deref().unwrap_or("").as_bytes());
        field(target.smoke_url.as_deref().unwrap_or("").as_bytes());
    }
    match run_plan {
        RunPlan::Deploy => field(b"deploy"),
        RunPlan::Rerun => field(b"rerun"),
        RunPlan::FromStep(step) => {
            field(b"from-step");
            field(&step.to_be_bytes());
        }
    }
    for step in &target.steps {
        field(step.as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn new_run_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{:x}-{seq:x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use tokio::sync::Notify;

    use super::*;

    const FROM: &str = "1111111111111111111111111111111111111111";
    const TO: &str = "2222222222222222222222222222222222222222";

    struct TempReceipts(PathBuf);

    impl TempReceipts {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "shipslip-receipt-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempReceipts {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Scripted transport. Preflight commands and steps are matched by the
    /// first rule whose needle appears in the script. Steps are simulated as
    /// the detached runner would behave on a server. Records every script.
    #[derive(Default)]
    struct Fake {
        rules: Vec<Rule>,
        ran: Mutex<Vec<String>>,
        launched: Mutex<HashMap<String, usize>>,
        reconnect_fails: bool,
        reconnects: AtomicUsize,
        lock: Mutex<LockSim>,
        server: Mutex<ServerSim>,
    }

    /// The app's checkout as preflight and the recheck see it.
    struct ServerSim {
        head: String,
        branch: String,
        operation: Option<String>,
        dirty: Vec<String>,
        target: String,
        ancestor: bool,
        fetch_failed: Option<String>,
        invalid_step: Option<(usize, String)>,
        /// A failing step 0 leaves the tree dirty.
        ff_fails_partially: bool,
        /// Reading the state after step 0 fails.
        inspect_fails: bool,
    }

    impl Default for ServerSim {
        fn default() -> Self {
            Self {
                head: FROM.into(),
                branch: "main".into(),
                operation: None,
                dirty: vec![],
                target: TO.into(),
                ancestor: true,
                fetch_failed: None,
                invalid_step: None,
                ff_fails_partially: false,
                inspect_fails: false,
            }
        }
    }

    impl ServerSim {
        fn state_lines(&self) -> Vec<String> {
            let mut out = vec![
                format!("@head {}", self.head),
                format!("@branch {}", self.branch),
            ];
            out.extend(self.operation.iter().map(|op| format!("@operation {op}")));
            out.extend(self.dirty.iter().map(|f| format!("@dirty {f}")));
            out
        }

        fn preflight_lines(&self) -> Vec<String> {
            let mut out = self.state_lines();
            if let Some(message) = &self.fetch_failed {
                out.push("@fetch_failed".into());
                out.push(format!("@message {message}"));
                return out;
            }
            out.push(format!("@target {}", self.target));
            out.push(format!("@fetch_head {}", self.target));
            if self.ancestor {
                out.push("@ancestor".into());
            }
            if self.target != self.head {
                out.push("@commit 2222222 Fix checkout".into());
                out.push("@commit 1a2b3c4 Add invoices".into());
            }
            if let Some((step, message)) = &self.invalid_step {
                out.push(format!("@invalid {step}"));
                out.push(format!("@message {message}"));
            }
            out
        }
    }

    #[derive(Default)]
    struct LockSim {
        owner: Option<String>,
        stale: bool,
        /// Another run takes the lock just before this step's check.
        lost_before_step: Option<usize>,
        checks: usize,
        heartbeats: usize,
    }

    #[derive(Default)]
    struct Rule {
        needle: &'static str,
        lines: Vec<&'static str>,
        code: i32,
        launch: Launch,
        /// The next observe drops the connection after this many lines.
        drop_after: Mutex<Option<usize>>,
        vanishes: bool,
        gate: Option<Arc<Notify>>,
        on_run: Option<Box<dyn Fn() + Send + Sync>>,
    }

    #[derive(Default, PartialEq)]
    enum Launch {
        #[default]
        Ok,
        Unreachable,
        LostAfterStart,
        LostBeforeStart,
    }

    fn lost() -> TransportError {
        TransportError::ConnectionLost("reset".into())
    }

    fn number_after(script: &str, prefix: &str) -> usize {
        let rest = &script[script.find(prefix).unwrap() + prefix.len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..end].parse().unwrap()
    }

    fn runner_key(script: &str) -> String {
        let start = script
            .find("/step-")
            .or_else(|| script.find("/maintenance-"))
            .unwrap_or_else(|| panic!("no runner key in script:\n{script}"))
            + 1;
        let rest = &script[start..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(rest.len());
        rest[..end].to_string()
    }

    impl Fake {
        fn rule(mut self, needle: &'static str, lines: &[&'static str], code: i32) -> Self {
            self.rules.push(Rule {
                needle,
                lines: lines.to_vec(),
                code,
                ..Rule::default()
            });
            self
        }

        fn last(&mut self) -> &mut Rule {
            self.rules.last_mut().unwrap()
        }

        fn on(self, needle: &'static str, lines: &[&'static str], code: i32) -> Self {
            self.rule(needle, lines, code)
        }

        fn launch(mut self, needle: &'static str, launch: Launch) -> Self {
            self = self.rule(needle, &["step output"], 0);
            self.last().launch = launch;
            self
        }

        fn drops(
            mut self,
            needle: &'static str,
            lines: &[&'static str],
            code: i32,
            after: usize,
        ) -> Self {
            self = self.rule(needle, lines, code);
            *self.last().drop_after.get_mut().unwrap() = Some(after);
            self
        }

        fn vanishes(mut self, needle: &'static str) -> Self {
            self = self.rule(needle, &[], 0);
            self.last().vanishes = true;
            self
        }

        fn gated(mut self, needle: &'static str, gate: Arc<Notify>) -> Self {
            self = self.rule(needle, &[], 0);
            self.last().gate = Some(gate);
            self
        }

        /// Succeeds, calling `f` while the step is still running.
        fn calls(mut self, needle: &'static str, f: impl Fn() + Send + Sync + 'static) -> Self {
            self = self.rule(needle, &[], 0);
            self.last().on_run = Some(Box::new(f));
            self
        }

        fn locked_by_other(self, stale: bool) -> Self {
            *self.lock.lock().unwrap() = LockSim {
                owner: Some("other".into()),
                stale,
                ..LockSim::default()
            };
            self
        }

        fn loses_lock_before_step(self, step: usize) -> Self {
            self.lock.lock().unwrap().lost_before_step = Some(step);
            self
        }

        fn lock_owner(&self) -> Option<String> {
            self.lock.lock().unwrap().owner.clone()
        }

        fn lock_script(&self, script: &str) -> String {
            let start = script.find("r='").unwrap() + 3;
            let run_id = &script[start..start + script[start..].find('\'').unwrap()];
            let mut lock = self.lock.lock().unwrap();
            let owned = |lock: &LockSim| lock.owner.as_deref() == Some(run_id);
            let answer = |owned| if owned { "owned" } else { "lost" };
            if script.contains("echo acquired") {
                match &lock.owner {
                    Some(other) => {
                        let owner = LockOwner {
                            run_id: other.clone(),
                            user: "ana".into(),
                            machine: "mac".into(),
                            pid: 1,
                            started_at: 0,
                            target_sha: TO.into(),
                        };
                        format!("held 30 {}", serde_json::to_string(&owner).unwrap())
                    }
                    None => {
                        lock.owner = Some(run_id.into());
                        "acquired".into()
                    }
                }
            } else if script.contains("released-") {
                if owned(&lock) {
                    lock.owner = None;
                    "released".into()
                } else {
                    "not-owner".into()
                }
            } else if script.contains(".broken-") {
                match (&lock.owner, lock.stale) {
                    (None, _) => "not-held".into(),
                    (Some(_), false) => "live 30".into(),
                    (Some(_), true) => {
                        lock.owner = None;
                        "broken".into()
                    }
                }
            } else if script.contains("heartbeat.tmp") {
                lock.heartbeats += 1;
                answer(owned(&lock)).into()
            } else {
                if lock.lost_before_step == Some(lock.checks) {
                    lock.owner = Some("other".into());
                }
                lock.checks += 1;
                answer(owned(&lock)).into()
            }
        }

        fn unreachable_after_drop(mut self) -> Self {
            self.reconnect_fails = true;
            self
        }

        fn server(self, change: impl FnOnce(&mut ServerSim)) -> Self {
            change(&mut self.server.lock().unwrap());
            self
        }

        /// Step launches whose script contains `needle`.
        fn launched(&self, needle: &str) -> usize {
            self.ran
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.contains("setsid") && s.contains(needle))
                .count()
        }

        fn ran_matching(&self, needle: &str) -> usize {
            self.ran
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.contains(needle))
                .count()
        }

        fn matching(&self, script: &str) -> usize {
            self.rules
                .iter()
                .position(|r| script.contains(r.needle))
                .unwrap_or_else(|| panic!("no fake rule for script:\n{script}"))
        }

        fn launch_step(&self, script: &str) -> Result<i32, TransportError> {
            let key = runner_key(script);
            let rule = self.matching(script);
            let mut launched = self.launched.lock().unwrap();
            if launched.contains_key(&key) {
                return Ok(1);
            }
            match self.rules[rule].launch {
                Launch::Unreachable => return Err(TransportError::Connect("refused".into())),
                Launch::LostBeforeStart => return Err(lost()),
                Launch::Ok | Launch::LostAfterStart => {}
            }
            launched.insert(key.clone(), rule);
            let mut server = self.server.lock().unwrap();
            match (key.as_str(), self.rules[rule].code) {
                ("step-0", 0) => server.head = server.target.clone(),
                ("step-0", _) if server.ff_fails_partially => server.dirty = vec![" M file".into()],
                _ => {}
            }
            match self.rules[rule].launch {
                Launch::LostAfterStart => Err(lost()),
                _ => Ok(0),
            }
        }

        async fn observe(
            &self,
            script: &str,
            output: &mpsc::UnboundedSender<String>,
        ) -> Result<i32, TransportError> {
            let key = runner_key(script);
            let skip = number_after(script, "tail -n +") - 1;
            let Some(&rule) = self.launched.lock().unwrap().get(&key) else {
                return Ok(1);
            };
            let rule = &self.rules[rule];
            if skip == 0 {
                if let Some(gate) = &rule.gate {
                    gate.notified().await;
                }
                if let Some(f) = &rule.on_run {
                    f();
                }
            }
            let _ = output.send(runner::LOG_START.to_string());
            let drop_after = rule.drop_after.lock().unwrap().take();
            let end = drop_after.map_or(rule.lines.len(), |n| skip + n);
            for line in &rule.lines[skip..end] {
                let _ = output.send(line.to_string());
            }
            match drop_after {
                Some(_) => Err(lost()),
                None => Ok(0),
            }
        }

        fn probe(&self, script: &str) -> String {
            let key = runner_key(script);
            match self.launched.lock().unwrap().get(&key) {
                None => "not-started".into(),
                Some(&rule) if self.rules[rule].vanishes => "gone".into(),
                Some(&rule) => format!("exited {}", self.rules[rule].code),
            }
        }
    }

    impl Transport for Fake {
        async fn run(
            &self,
            script: &str,
            output: mpsc::UnboundedSender<String>,
        ) -> Result<i32, TransportError> {
            self.ran.lock().unwrap().push(script.to_string());
            if script.contains("shipslip.lock") {
                let _ = output.send(self.lock_script(script));
                Ok(0)
            } else if script.contains("setsid") {
                self.launch_step(script)
            } else if script.contains(runner::LOG_START) {
                self.observe(script, &output).await
            } else if script.contains("not-started") {
                let _ = output.send(self.probe(script));
                Ok(0)
            } else if script.contains("@fetch_head") {
                for line in self.server.lock().unwrap().preflight_lines() {
                    let _ = output.send(line);
                }
                Ok(0)
            } else if script.contains("@head") {
                let server = self.server.lock().unwrap();
                if server.inspect_fails && self.launched.lock().unwrap().contains_key("step-0") {
                    return Err(lost());
                }
                for line in server.state_lines() {
                    let _ = output.send(line);
                }
                Ok(0)
            } else {
                let rule = &self.rules[self.matching(script)];
                for line in &rule.lines {
                    let _ = output.send(line.to_string());
                }
                Ok(rule.code)
            }
        }

        async fn reconnect(&self) -> Result<(), TransportError> {
            self.reconnects.fetch_add(1, Ordering::SeqCst);
            match self.reconnect_fails {
                true => Err(TransportError::Connect("refused".into())),
                false => Ok(()),
            }
        }
    }

    fn target(production: bool, steps: &[&str]) -> DeployTarget {
        DeployTarget {
            env: if production { "production" } else { "staging" }.into(),
            production,
            ssh_alias: "app-prod".into(),
            path: "/var/www/app".into(),
            branch: "main".into(),
            steps: steps.iter().map(|s| s.to_string()).collect(),
            maintenance: false,
            watch_log: false,
            log: None,
            log_daily: false,
            smoke_url: None,
        }
    }

    fn maintenance_target(production: bool, steps: &[&str]) -> DeployTarget {
        DeployTarget {
            maintenance: true,
            ..target(production, steps)
        }
    }

    fn preflight() -> Fake {
        Fake::default()
    }

    async fn prepared(fake: &Fake, production: bool, steps: &[&str]) -> Preview {
        prepare(target(production, steps), fake).await.unwrap()
    }

    async fn prepared_with_plan(
        fake: &Fake,
        production: bool,
        steps: &[&str],
        run_plan: RunPlan,
    ) -> Preview {
        prepare_with_plan(target(production, steps), run_plan, fake)
            .await
            .unwrap()
    }

    async fn collect(mut rx: mpsc::UnboundedReceiver<DeployEvent>) -> Vec<DeployEvent> {
        let mut out = Vec::new();
        while let Some(e) = rx.recv().await {
            out.push(e);
        }
        out
    }

    #[tokio::test]
    async fn prepare_builds_preview() {
        let fake = preflight();
        let p = prepared(&fake, false, &["php artisan migrate --force"]).await;
        assert_eq!(p.from_sha(), FROM);
        assert_eq!(p.target_sha(), TO);
        assert_eq!(p.commits().len(), 2);
        assert_eq!(p.recipe_hash().len(), 64);
        assert_eq!(
            fake.ran_matching("+refs/heads/main:refs/remotes/origin/main"),
            1
        );
    }

    #[tokio::test]
    async fn prepare_blocks_when_up_to_date() {
        let fake = preflight().server(|s| s.target = FROM.into());
        let err = prepare(target(false, &[]), &fake).await.unwrap_err();
        assert!(matches!(err, PrepareError::Blocked(BlockReason::UpToDate)));
    }

    #[tokio::test]
    async fn rerun_preview_allows_only_the_clean_checked_out_target() {
        let fake = preflight().server(|s| {
            s.head = TO.into();
            s.target = TO.into();
        });
        let p = prepared_with_plan(&fake, false, &["php artisan migrate"], RunPlan::Rerun).await;
        assert_eq!(p.from_sha(), TO);
        assert_eq!(p.target_sha(), TO);
        assert!(p.commits().is_empty());
        assert_eq!(p.run_plan(), RunPlan::Rerun);
    }

    #[tokio::test]
    async fn rerun_preview_blocks_when_target_is_not_already_checked_out() {
        let fake = preflight();
        let err = prepare_with_plan(
            target(false, &["php artisan migrate"]),
            RunPlan::Rerun,
            &fake,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            PrepareError::Blocked(BlockReason::RerunTargetMismatch { .. })
        ));
        assert_eq!(fake.lock_owner(), None);
    }

    #[tokio::test]
    async fn from_step_rejects_zero_and_out_of_range_steps_before_remote_work() {
        for step in [0, 2] {
            let fake = preflight();
            assert!(matches!(
                prepare_with_plan(
                    target(false, &["only step"]),
                    RunPlan::FromStep(step),
                    &fake
                )
                .await,
                Err(PrepareError::InvalidTarget(_))
            ));
            assert!(fake.ran.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn prepare_rejects_garbage_sha() {
        let fake = preflight().server(|s| s.head = "not a sha".into());
        let err = prepare(target(false, &[]), &fake).await.unwrap_err();
        assert!(matches!(err, PrepareError::UnexpectedOutput { .. }));
    }

    #[tokio::test]
    async fn prepare_rejects_unsafe_branch() {
        let mut t = target(false, &[]);
        t.branch = "main;rm -rf /".into();
        let err = prepare(t, &Fake::default()).await.unwrap_err();
        assert!(matches!(err, PrepareError::InvalidTarget(_)));
    }

    #[tokio::test]
    async fn production_needs_typed_env_name() {
        let p = prepared(&preflight(), true, &[]).await;
        assert!(Confirmation::from(&p, None).is_err());
        assert!(Confirmation::from(&p, Some("staging")).is_err());
        assert!(Confirmation::from(&p, Some("production")).is_ok());
    }

    #[tokio::test]
    async fn staging_confirms_without_typing() {
        let p = prepared(&preflight(), false, &[]).await;
        assert!(Confirmation::from(&p, None).is_ok());
    }

    #[tokio::test]
    async fn confirmation_for_another_preview_is_rejected() {
        let a = prepared(&preflight(), false, &["echo a"]).await;
        let b = prepared(&preflight(), false, &["echo b"]).await;
        let confirm_a = Confirmation::from(&a, None).unwrap();
        let err = execute(b, confirm_a, Arc::new(preflight())).unwrap_err();
        assert_eq!(err, ExecuteError::ConfirmationMismatch);
    }

    #[tokio::test]
    async fn successful_deploy_emits_ordered_events() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &["Fast-forward"], 0)
                .on("composer install", &["Installing", "Done"], 0),
        );
        let p = prepared(&fake, false, &["composer install --no-dev"]).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, _h) = execute(p, c, fake.clone()).unwrap();

        let events = collect(rx).await;
        assert_eq!(
            events,
            vec![
                DeployEvent::StepStarted {
                    index: 0,
                    name: "git fast-forward".into()
                },
                DeployEvent::Output {
                    index: 0,
                    line: "Fast-forward".into()
                },
                DeployEvent::StepFinished {
                    index: 0,
                    status: StepStatus::Ok,
                    exit_code: Some(0)
                },
                DeployEvent::StepStarted {
                    index: 1,
                    name: "composer install --no-dev".into()
                },
                DeployEvent::Output {
                    index: 1,
                    line: "Installing".into()
                },
                DeployEvent::Output {
                    index: 1,
                    line: "Done".into()
                },
                DeployEvent::StepFinished {
                    index: 1,
                    status: StepStatus::Ok,
                    exit_code: Some(0)
                },
                DeployEvent::Finished(DeployOutcome::Succeeded),
            ]
        );
        assert_eq!(fake.launched(&format!("git merge --ff-only {TO}")), 1);
    }

    #[tokio::test]
    async fn failing_step_stops_the_deploy() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .on("migrate", &["SQLSTATE[42S22]"], 1)
                .on("optimize", &[], 0),
        );
        let p = prepared(
            &fake,
            false,
            &["php artisan migrate --force", "php artisan optimize"],
        )
        .await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, _h) = execute(p, c, fake.clone()).unwrap();

        let events = collect(rx).await;
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::FailedAtStep {
                step: 1,
                partial_update: false
            }))
        );
        assert_eq!(fake.launched("optimize"), 0);
    }

    #[tokio::test]
    async fn exit_255_is_a_failure_not_unknown() {
        let fake = Arc::new(preflight().on("git merge --ff-only", &[], 0).on(
            "migrate",
            &["PHP Fatal error"],
            255,
        ));
        let p = prepared(&fake, false, &["php artisan migrate --force"]).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, _h) = execute(p, c, fake).unwrap();

        let events = collect(rx).await;
        assert!(events.contains(&DeployEvent::StepFinished {
            index: 1,
            status: StepStatus::Failed,
            exit_code: Some(255),
        }));
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::FailedAtStep {
                step: 1,
                partial_update: false
            }))
        );
    }

    async fn deploy(fake: &Arc<Fake>, steps: &[&str]) -> Vec<DeployEvent> {
        let p = prepared(fake, false, steps).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, _h) = execute(p, c, fake.clone()).unwrap();
        collect(rx).await
    }

    async fn deploy_target(fake: &Arc<Fake>, target: DeployTarget) -> Vec<DeployEvent> {
        let p = prepare(target, &**fake).await.unwrap();
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, _h) = execute(p, c, fake.clone()).unwrap();
        collect(rx).await
    }

    #[tokio::test]
    async fn maintenance_runs_down_before_deploy_and_up_after_success() {
        let fake = Arc::new(
            preflight()
                .on("php artisan down", &["down output"], 0)
                .on("git merge --ff-only", &[], 0)
                .on("deploy command", &[], 0)
                .on("php artisan up", &["up output"], 0),
        );
        let events = deploy_target(&fake, maintenance_target(false, &["deploy command"])).await;

        let down = events
            .iter()
            .position(|e| {
                *e == DeployEvent::MaintenanceStarted {
                    phase: MaintenancePhase::Down,
                }
            })
            .unwrap();
        let merge = events
            .iter()
            .position(|e| matches!(e, DeployEvent::StepStarted { index: 0, .. }))
            .unwrap();
        let up = events
            .iter()
            .position(|e| {
                *e == DeployEvent::MaintenanceStarted {
                    phase: MaintenancePhase::Up,
                }
            })
            .unwrap();
        assert!(down < merge && merge < up);
        assert!(events.contains(&DeployEvent::MaintenanceOutput {
            phase: MaintenancePhase::Down,
            line: "down output".into(),
        }));
        assert!(events.contains(&DeployEvent::MaintenanceOutput {
            phase: MaintenancePhase::Up,
            line: "up output".into(),
        }));
        assert!(!events.contains(&DeployEvent::AppLeftDown));
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
        );
    }

    #[tokio::test]
    async fn maintenance_down_failure_aborts_before_fast_forward() {
        let fake = Arc::new(preflight().on("php artisan down", &["down failed"], 9).on(
            "git merge --ff-only",
            &[],
            0,
        ));
        let events = deploy_target(&fake, maintenance_target(false, &[])).await;

        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::AbortedBeforeChanges(
                AbortReason::MaintenanceDownFailed(9)
            )))
        );
        assert_eq!(fake.launched("git merge --ff-only"), 0);
        assert!(!events.contains(&DeployEvent::AppLeftDown));
    }

    #[tokio::test]
    async fn stopping_during_maintenance_down_skips_fast_forward() {
        let gate = Arc::new(Notify::new());
        let fake = Arc::new(preflight().gated("php artisan down", gate.clone()).on(
            "git merge --ff-only",
            &[],
            0,
        ));
        let p = prepare(maintenance_target(false, &[]), &*fake)
            .await
            .unwrap();
        let c = Confirmation::from(&p, None).unwrap();
        let (mut rx, handle) = execute(p, c, fake.clone()).unwrap();

        while rx.recv().await
            != Some(DeployEvent::MaintenanceStarted {
                phase: MaintenancePhase::Down,
            })
        {}
        handle.stop_after_step();
        gate.notify_one();
        let events = collect(rx).await;

        assert_eq!(fake.launched("git merge --ff-only"), 0);
        assert!(events.contains(&DeployEvent::AppLeftDown));
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(
                DeployOutcome::CancelledBeforeChanges
            ))
        );
    }

    #[tokio::test]
    async fn failed_deploy_leaves_maintenance_mode_on() {
        let fake = Arc::new(
            preflight()
                .on("php artisan down", &[], 0)
                .on("git merge --ff-only", &[], 0)
                .on("deploy command", &[], 1),
        );
        let events = deploy_target(&fake, maintenance_target(false, &["deploy command"])).await;

        assert!(events.contains(&DeployEvent::AppLeftDown));
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::FailedAtStep {
                step: 1,
                partial_update: false,
            }))
        );
        assert_eq!(fake.launched("php artisan up"), 0);
    }

    #[tokio::test]
    async fn failed_maintenance_up_keeps_successful_deploy_outcome() {
        let fake = Arc::new(
            preflight()
                .on("php artisan down", &[], 0)
                .on("git merge --ff-only", &[], 0)
                .on("deploy command", &[], 0)
                .on("php artisan up", &[], 7),
        );
        let events = deploy_target(&fake, maintenance_target(false, &["deploy command"])).await;

        assert!(events.contains(&DeployEvent::MaintenanceFinished {
            phase: MaintenancePhase::Up,
            status: StepStatus::Failed,
            exit_code: Some(7),
        }));
        assert!(events.contains(&DeployEvent::AppLeftDown));
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
        );
    }

    #[tokio::test]
    async fn maintenance_disabled_skips_artisan_commands() {
        let fake = Arc::new(preflight().on("git merge --ff-only", &[], 0).on(
            "deploy command",
            &[],
            0,
        ));
        let events = deploy_target(&fake, target(false, &["deploy command"])).await;

        assert_eq!(fake.ran_matching("php artisan"), 0);
        assert!(!events.iter().any(|event| matches!(
            event,
            DeployEvent::MaintenanceStarted { .. }
                | DeployEvent::MaintenanceOutput { .. }
                | DeployEvent::MaintenanceFinished { .. }
                | DeployEvent::AppLeftDown
        )));
    }

    #[tokio::test]
    async fn from_step_skips_the_fast_forward_and_earlier_recipe_steps() {
        let fake = Arc::new(preflight().on("second command", &[], 0).server(|s| {
            s.head = TO.into();
            s.target = TO.into();
        }));
        let p = prepare_with_plan(
            target(false, &["first command", "second command"]),
            RunPlan::FromStep(2),
            &*fake,
        )
        .await
        .unwrap();
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, _handle) = execute(p, c, fake.clone()).unwrap();
        let events = collect(rx).await;

        assert!(events.contains(&DeployEvent::StepStarted {
            index: 2,
            name: "second command".into(),
        }));
        assert!(!events
            .iter()
            .any(|event| matches!(event, DeployEvent::StepStarted { index: 0 | 1, .. })));
        assert_eq!(fake.launched("first command"), 0);
        assert_eq!(fake.launched("git merge --ff-only"), 0);
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
        );
    }

    #[tokio::test]
    async fn bring_app_up_runs_under_the_lock_and_releases_it() {
        let fake = preflight().on("php artisan up", &["back up"], 0);
        let target = target(false, &[]);
        let output = bring_app_up(&target, &fake).await.unwrap();
        assert_eq!(output, ["back up"]);
        assert_eq!(fake.lock_owner(), None);
        assert_eq!(fake.launched("php artisan up"), 1);
    }

    #[tokio::test]
    async fn bring_app_up_refuses_to_run_while_a_deploy_holds_the_lock() {
        let fake = preflight().locked_by_other(false);
        let err = bring_app_up(&target(false, &[]), &fake).await.unwrap_err();
        assert!(matches!(err, BringUpError::LockHeld(_)));
        assert_eq!(fake.ran_matching("php artisan up"), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn bring_app_up_keeps_the_lock_when_its_result_is_unknown() {
        let fake = preflight()
            .drops("php artisan up", &[], 0, 0)
            .unreachable_after_drop();
        let error = bring_app_up(&target(false, &[]), &fake).await.unwrap_err();

        assert!(matches!(error, BringUpError::Interrupted(_)));
        assert!(fake.lock_owner().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn interrupted_maintenance_up_warns_the_app_may_be_down() {
        let fake = Arc::new(
            preflight()
                .on("php artisan down", &[], 0)
                .on("git merge --ff-only", &[], 0)
                .on("deploy command", &[], 0)
                .drops("php artisan up", &[], 0, 0)
                .unreachable_after_drop(),
        );
        let events = deploy_target(&fake, maintenance_target(false, &["deploy command"])).await;

        assert!(events.contains(&DeployEvent::MaintenanceFinished {
            phase: MaintenancePhase::Up,
            status: StepStatus::Unknown,
            exit_code: None,
        }));
        assert!(events.contains(&DeployEvent::AppLeftDown));
        assert!(matches!(
            events.last(),
            Some(DeployEvent::Interrupted { index: 2, .. })
        ));
        assert!(fake.lock_owner().is_some());
    }

    fn output(events: &[DeployEvent], step: usize) -> Vec<&str> {
        events
            .iter()
            .filter_map(|e| match e {
                DeployEvent::Output { index, line } if *index == step => Some(line.as_str()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn dropped_connection_reattaches_without_losing_or_repeating_output() {
        let fake = Arc::new(preflight().on("git merge --ff-only", &[], 0).drops(
            "migrate",
            &["a", "b", "c"],
            3,
            1,
        ));
        let events = deploy(&fake, &["php artisan migrate --force"]).await;

        assert_eq!(output(&events, 1), ["a", "b", "c"]);
        assert!(events.contains(&DeployEvent::StepFinished {
            index: 1,
            status: StepStatus::Failed,
            exit_code: Some(3),
        }));
        assert_eq!(fake.reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(fake.launched("migrate"), 1);
    }

    #[tokio::test]
    async fn step_that_vanished_without_exit_code_is_unknown() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .vanishes("migrate"),
        );
        let events = deploy(&fake, &["php artisan migrate --force"]).await;
        assert!(matches!(
            events.last(),
            Some(DeployEvent::Finished(DeployOutcome::Unknown {
                step: 1,
                ..
            }))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn server_unreachable_after_drop_is_interrupted() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .drops("migrate", &["a"], 0, 0)
                .unreachable_after_drop(),
        );
        let events = deploy(&fake, &["php artisan migrate --force"]).await;

        assert!(
            matches!(
                events.last(),
                Some(DeployEvent::Interrupted { index: 1, .. })
            ),
            "{events:?}"
        );
        assert!(!events.iter().any(|e| matches!(
            e,
            DeployEvent::Finished(_) | DeployEvent::StepFinished { index: 1, .. }
        )));
        assert_eq!(fake.reconnects.load(Ordering::SeqCst), 4);
        assert_eq!(fake.launched("migrate"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn lost_launch_that_started_is_followed_not_relaunched() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .launch("migrate", Launch::LostAfterStart),
        );
        let events = deploy(&fake, &["php artisan migrate --force"]).await;

        assert_eq!(output(&events, 1), ["step output"]);
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
        );
        assert_eq!(fake.launched("migrate"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn lost_launch_that_never_started_is_not_started() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .launch("migrate", Launch::LostBeforeStart),
        );
        let events = deploy(&fake, &["php artisan migrate --force"]).await;

        assert!(events.contains(&DeployEvent::StepFinished {
            index: 1,
            status: StepStatus::NotStarted,
            exit_code: None,
        }));
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep {
                step: 0,
                reason: StopReason::ConnectFailed("connection lost: reset".into())
            }))
        );
        assert_eq!(fake.launched("migrate"), 1);
    }

    #[tokio::test]
    async fn stop_after_step_finishes_current_step_then_stops() {
        let gate = Arc::new(Notify::new());
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .gated("composer install", gate.clone())
                .on("migrate", &[], 0),
        );
        let p = prepared(
            &fake,
            false,
            &["composer install", "php artisan migrate --force"],
        )
        .await;
        let c = Confirmation::from(&p, None).unwrap();
        let (mut rx, h) = execute(p, c, fake.clone()).unwrap();

        while rx.recv().await
            != Some(DeployEvent::StepStarted {
                index: 1,
                name: "composer install".into(),
            })
        {}
        h.stop_after_step();
        gate.notify_one();

        let events = collect(rx).await;
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep {
                step: 1,
                reason: StopReason::Requested
            }))
        );
        assert_eq!(fake.launched("migrate"), 0);
    }

    #[tokio::test]
    async fn detach_leaves_running_step_and_emits_no_finished() {
        let gate = Arc::new(Notify::new());
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .gated("migrate", gate),
        );
        let p = prepared(&fake, false, &["php artisan migrate --force"]).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (mut rx, h) = execute(p, c, fake).unwrap();

        while rx.recv().await
            != Some(DeployEvent::StepStarted {
                index: 1,
                name: "php artisan migrate --force".into(),
            })
        {}
        h.detach();

        let events = collect(rx).await;
        assert_eq!(events, vec![DeployEvent::Detached { index: 1 }]);
    }

    // `#[tokio::test]` is single-threaded: the deploy task does not start
    // until the test awaits, so the handle call lands before step 0.
    #[tokio::test]
    async fn stop_before_first_step_changes_nothing() {
        let fake = Arc::new(preflight().on("git merge --ff-only", &[], 0));
        let p = prepared(&fake, false, &[]).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, h) = execute(p, c, fake.clone()).unwrap();
        h.stop_after_step();

        let events = collect(rx).await;
        assert_eq!(
            events,
            vec![DeployEvent::Finished(DeployOutcome::CancelledBeforeChanges)]
        );
        assert_eq!(fake.launched("git merge --ff-only"), 0);
    }

    #[tokio::test]
    async fn detach_before_first_step_changes_nothing() {
        let fake = Arc::new(preflight().on("git merge --ff-only", &[], 0));
        let p = prepared(&fake, false, &[]).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, h) = execute(p, c, fake.clone()).unwrap();
        h.detach();

        let events = collect(rx).await;
        assert_eq!(
            events,
            vec![DeployEvent::Finished(DeployOutcome::CancelledBeforeChanges)]
        );
        assert_eq!(fake.launched("git merge --ff-only"), 0);
    }

    #[tokio::test]
    async fn detach_between_steps_does_not_start_the_next_step() {
        let handle = Arc::new(OnceLock::<ExecutionHandle>::new());
        let h2 = handle.clone();
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .calls("composer install", move || h2.get().unwrap().detach())
                .on("migrate", &[], 0),
        );
        let p = prepared(
            &fake,
            false,
            &["composer install", "php artisan migrate --force"],
        )
        .await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, h) = execute(p, c, fake.clone()).unwrap();
        handle.set(h).unwrap();

        let events = collect(rx).await;
        assert!(events.contains(&DeployEvent::StepFinished {
            index: 1,
            status: StepStatus::Ok,
            exit_code: Some(0),
        }));
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep {
                step: 1,
                reason: StopReason::Requested
            }))
        );
        assert_eq!(fake.launched("migrate"), 0);
    }

    #[tokio::test]
    async fn connect_failure_at_step_0_aborts_before_changes() {
        let fake = Arc::new(preflight().launch("git merge --ff-only", Launch::Unreachable));
        let p = prepared(&fake, false, &["php artisan migrate --force"]).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, _h) = execute(p, c, fake).unwrap();

        let events = collect(rx).await;
        assert_eq!(
            events[1..],
            [
                DeployEvent::StepFinished {
                    index: 0,
                    status: StepStatus::NotStarted,
                    exit_code: None,
                },
                DeployEvent::Finished(DeployOutcome::AbortedBeforeChanges(
                    AbortReason::ConnectFailed("could not connect: refused".into())
                )),
            ]
        );
    }

    #[tokio::test]
    async fn connect_failure_later_stops_after_previous_step() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .launch("migrate", Launch::Unreachable),
        );
        let p = prepared(&fake, false, &["php artisan migrate --force"]).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, _h) = execute(p, c, fake).unwrap();

        let events = collect(rx).await;
        assert!(events.contains(&DeployEvent::StepFinished {
            index: 1,
            status: StepStatus::NotStarted,
            exit_code: None,
        }));
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep {
                step: 0,
                reason: StopReason::ConnectFailed("could not connect: refused".into())
            }))
        );
    }

    #[tokio::test]
    async fn confirmation_for_an_identical_earlier_preview_is_rejected() {
        let a = prepared(&preflight(), false, &["echo a"]).await;
        let b = prepared(&preflight(), false, &["echo a"]).await;
        assert_eq!(a.recipe_hash(), b.recipe_hash());
        assert_ne!(a.run_id(), b.run_id());
        let confirm_a = Confirmation::from(&a, None).unwrap();
        let err = execute(b, confirm_a, Arc::new(preflight())).unwrap_err();
        assert_eq!(err, ExecuteError::ConfirmationMismatch);
    }

    #[tokio::test]
    async fn prepare_blocks_when_the_lock_is_held() {
        let fake = preflight().locked_by_other(false);
        let err = prepare(target(false, &[]), &fake).await.unwrap_err();
        let PrepareError::LockHeld(info) = err else {
            panic!("{err:?}");
        };
        assert!(!info.is_stale());
        assert_eq!(info.owner.unwrap().run_id, "other");
    }

    #[tokio::test]
    async fn lock_lost_before_step_0_aborts_before_changes() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .loses_lock_before_step(0),
        );
        let events = deploy(&fake, &[]).await;
        assert_eq!(
            events,
            [DeployEvent::Finished(DeployOutcome::AbortedBeforeChanges(
                AbortReason::LockLost
            ))]
        );
        assert_eq!(fake.launched("git merge --ff-only"), 0);
    }

    #[tokio::test]
    async fn lock_lost_mid_deploy_stops_after_the_current_step() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .on("composer", &[], 0)
                .on("migrate", &[], 0)
                .loses_lock_before_step(2),
        );
        let events = deploy(&fake, &["composer install", "php artisan migrate"]).await;
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep {
                step: 1,
                reason: StopReason::LockLost
            }))
        );
        assert_eq!(fake.launched("migrate"), 0);
        assert_eq!(fake.lock_owner().as_deref(), Some("other"));
    }

    #[tokio::test]
    async fn lock_is_released_after_success_and_failure() {
        for code in [0, 1] {
            let fake = Arc::new(preflight().on("git merge --ff-only", &[], 0).on(
                "migrate",
                &[],
                code,
            ));
            let events = deploy(&fake, &["php artisan migrate"]).await;
            assert!(matches!(events.last(), Some(DeployEvent::Finished(_))));
            assert_eq!(fake.lock_owner(), None, "exit {code}");
        }
    }

    #[tokio::test]
    async fn lock_is_kept_after_an_unknown_outcome() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .vanishes("migrate"),
        );
        deploy(&fake, &["php artisan migrate"]).await;
        assert!(fake.lock_owner().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn lock_is_kept_after_interrupted_and_detached() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .drops("migrate", &[], 0, 0)
                .unreachable_after_drop(),
        );
        deploy(&fake, &["php artisan migrate"]).await;
        assert!(fake.lock_owner().is_some());

        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .gated("migrate", Arc::new(Notify::new())),
        );
        let p = prepared(&fake, false, &["php artisan migrate"]).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (mut rx, h) = execute(p, c, fake.clone()).unwrap();
        while !matches!(
            rx.recv().await,
            Some(DeployEvent::StepStarted { index: 1, .. })
        ) {}
        h.detach();
        collect(rx).await;
        assert!(fake.lock_owner().is_some());
    }

    #[tokio::test]
    async fn cancel_releases_the_lock() {
        let fake = preflight();
        let p = prepared(&fake, false, &[]).await;
        assert!(fake.lock_owner().is_some());
        cancel(p, &fake).await.unwrap();
        assert_eq!(fake.lock_owner(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_runs_while_a_step_runs() {
        let gate = Arc::new(Notify::new());
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 0)
                .gated("migrate", gate.clone()),
        );
        let p = prepared(&fake, false, &["php artisan migrate"]).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (mut rx, _h) = execute(p, c, fake.clone()).unwrap();
        while !matches!(
            rx.recv().await,
            Some(DeployEvent::StepStarted { index: 1, .. })
        ) {}
        tokio::time::sleep(lock::HEARTBEAT_EVERY * 2 + Duration::from_secs(1)).await;
        assert_eq!(fake.lock.lock().unwrap().heartbeats, 2);
        gate.notify_one();
        let events = collect(rx).await;
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
        );
    }

    #[tokio::test]
    async fn break_lock_needs_the_env_name_and_a_stale_lock() {
        let t = target(false, &[]);
        let live = preflight().locked_by_other(false);
        assert_eq!(
            break_lock(&t, &live, "production").await,
            Err(BreakLockError::EnvNameRequired("staging".into()))
        );
        assert_eq!(
            break_lock(&t, &live, "staging").await,
            Err(BreakLockError::Live(30))
        );
        assert_eq!(
            break_lock(&t, &preflight(), "staging").await,
            Err(BreakLockError::NotHeld)
        );
        let stale = preflight().locked_by_other(true);
        assert_eq!(break_lock(&t, &stale, "staging").await, Ok(()));
        assert_eq!(stale.lock_owner(), None);
    }

    #[tokio::test]
    async fn blocked_preflight_takes_no_lock() {
        type Case = (fn(&mut ServerSim), BlockReason);
        let cases: [Case; 6] = [
            (
                |s| s.dirty = vec![" M .env".into()],
                BlockReason::DirtyTree(vec![" M .env".into()]),
            ),
            (
                |s| s.operation = Some("MERGE_HEAD".into()),
                BlockReason::OperationInProgress("MERGE_HEAD".into()),
            ),
            (
                |s| s.branch = String::new(),
                BlockReason::WrongBranch {
                    expected: "main".into(),
                    actual: String::new(),
                },
            ),
            (
                |s| s.fetch_failed = Some("Permission denied (publickey).".into()),
                BlockReason::FetchFailed("Permission denied (publickey).".into()),
            ),
            (|s| s.ancestor = false, BlockReason::NotFastForward),
            (
                |s| s.invalid_step = Some((1, "unexpected EOF".into())),
                BlockReason::InvalidStep {
                    step: 1,
                    message: "unexpected EOF".into(),
                },
            ),
        ];
        for (change, expected) in cases {
            let fake = preflight().server(change);
            let err = prepare(target(false, &["echo 'x"]), &fake)
                .await
                .unwrap_err();
            assert!(
                matches!(&err, PrepareError::Blocked(r) if *r == expected),
                "{err:?}"
            );
            assert_eq!(fake.lock_owner(), None);
        }
    }

    #[tokio::test]
    async fn changes_after_the_preview_abort_before_step_0() {
        type Case = (fn(&mut ServerSim), fn(&AbortReason) -> bool);
        let cases: [Case; 3] = [
            (
                |s| s.head = "3".repeat(40),
                |r| matches!(r, AbortReason::HeadMoved { .. }),
            ),
            (
                |s| s.dirty = vec![" M file".into()],
                |r| matches!(r, AbortReason::DirtyTree(_)),
            ),
            (
                |s| s.operation = Some("rebase-merge".into()),
                |r| matches!(r, AbortReason::OperationInProgress(_)),
            ),
        ];
        for (change, expected) in cases {
            let fake = Arc::new(preflight().on("git merge --ff-only", &[], 0));
            let p = prepared(&fake, false, &[]).await;
            change(&mut fake.server.lock().unwrap());
            let c = Confirmation::from(&p, None).unwrap();
            let (rx, _h) = execute(p, c, fake.clone()).unwrap();
            let events = collect(rx).await;

            let [DeployEvent::Finished(DeployOutcome::AbortedBeforeChanges(reason))] = &events[..]
            else {
                panic!("{events:?}");
            };
            assert!(expected(reason), "{reason:?}");
            assert_eq!(fake.launched("git merge --ff-only"), 0);
            assert_eq!(fake.lock_owner(), None);
        }
    }

    fn step_0_failure(events: &[DeployEvent]) -> (&DeployEvent, &DeployEvent) {
        let n = events.len();
        (&events[n - 2], &events[n - 1])
    }

    #[tokio::test]
    async fn failed_fast_forward_that_changed_nothing_is_not_partial() {
        let fake = Arc::new(preflight().on("git merge --ff-only", &[], 128));
        let events = deploy(&fake, &[]).await;
        assert_eq!(
            step_0_failure(&events),
            (
                &DeployEvent::ServerState {
                    head: Some(FROM.into()),
                    tree_dirty: Some(false)
                },
                &DeployEvent::Finished(DeployOutcome::FailedAtStep {
                    step: 0,
                    partial_update: false
                })
            )
        );
    }

    #[tokio::test]
    async fn failed_fast_forward_that_left_changes_is_partial() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 128)
                .server(|s| s.ff_fails_partially = true),
        );
        let events = deploy(&fake, &[]).await;
        assert_eq!(
            step_0_failure(&events),
            (
                &DeployEvent::ServerState {
                    head: Some(FROM.into()),
                    tree_dirty: Some(true)
                },
                &DeployEvent::Finished(DeployOutcome::FailedAtStep {
                    step: 0,
                    partial_update: true
                })
            )
        );
    }

    #[tokio::test]
    async fn failed_fast_forward_that_cannot_be_inspected_counts_as_partial() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &[], 128)
                .server(|s| s.inspect_fails = true),
        );
        let events = deploy(&fake, &[]).await;
        assert_eq!(
            step_0_failure(&events),
            (
                &DeployEvent::ServerState {
                    head: None,
                    tree_dirty: None
                },
                &DeployEvent::Finished(DeployOutcome::FailedAtStep {
                    step: 0,
                    partial_update: true
                })
            )
        );
    }

    #[test]
    fn recipe_hash_covers_every_field() {
        let base = target(false, &["a", "b"]);
        let changes: [fn(&mut DeployTarget); 7] = [
            |t| t.env = "other".into(),
            |t| t.production = true,
            |t| t.ssh_alias = "other".into(),
            |t| t.path = "/other".into(),
            |t| t.branch = "other".into(),
            |t| t.steps = vec!["ab".into()],
            |t| t.steps = vec!["a".into(), "b".into(), "".into()],
        ];
        for change in changes {
            let mut t = base.clone();
            change(&mut t);
            assert_ne!(
                recipe_hash(&t, RunPlan::Deploy),
                recipe_hash(&base, RunPlan::Deploy),
                "{t:?}"
            );
        }
        assert_ne!(
            recipe_hash(&base, RunPlan::Deploy),
            recipe_hash(&base, RunPlan::Rerun)
        );
        assert_ne!(
            recipe_hash(&base, RunPlan::FromStep(1)),
            recipe_hash(&base, RunPlan::FromStep(2))
        );
    }

    #[tokio::test]
    async fn recorded_run_persists_result_and_releases_lock() {
        let fake = Arc::new(
            preflight()
                .on("git merge --ff-only", &["Fast-forward"], 0)
                .on("deploy command", &["done"], 0),
        );
        let preview = prepared(&fake, false, &["deploy command"]).await;
        let dir = TempReceipts::new();
        let journal =
            Arc::new(ReceiptJournal::create(&dir.0, "app", Path::new("/repo"), &preview).unwrap());
        let path = journal.path().to_path_buf();
        let confirmation = Confirmation::from(&preview, None).unwrap();
        let (events, _handle) =
            execute_recorded(preview, confirmation, fake.clone(), journal).unwrap();
        assert_eq!(
            collect(events).await.last(),
            Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
        );
        let saved = ReceiptJournal::load(&path).unwrap().snapshot();
        assert_eq!(saved.status, ReceiptStatus::Final);
        assert_eq!(saved.outcome, Some(DeployOutcome::Succeeded));
        assert_eq!(saved.steps[0].status, ReceiptStepStatus::Ok);
        assert_eq!(saved.steps[1].output, ["done"]);
        assert_eq!(fake.lock_owner(), None);
    }

    #[tokio::test]
    async fn attach_recovers_active_step_without_relaunching_it() {
        let gate = Arc::new(Notify::new());
        let fake = Arc::new(preflight().gated("git merge --ff-only", gate.clone()).on(
            "deploy command",
            &["next step"],
            0,
        ));
        let preview = prepared(&fake, false, &["deploy command"]).await;
        let dir = TempReceipts::new();
        let journal =
            Arc::new(ReceiptJournal::create(&dir.0, "app", Path::new("/repo"), &preview).unwrap());
        let path = journal.path().to_path_buf();
        let confirmation = Confirmation::from(&preview, None).unwrap();
        let (mut events, handle) =
            execute_recorded(preview, confirmation, fake.clone(), journal.clone()).unwrap();
        assert!(matches!(
            events.recv().await,
            Some(DeployEvent::StepStarted { index: 0, .. })
        ));
        tokio::time::timeout(Duration::from_secs(2), async {
            while fake.launched("git merge --ff-only") == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        handle.detach();
        assert_eq!(
            collect(events).await.last(),
            Some(&DeployEvent::Detached { index: 0 })
        );
        assert_eq!(journal.snapshot().phase, ReceiptPhase::Step(0));
        tokio::time::timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&journal) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached observer kept the receipt claim");
        drop(journal);

        let resumed = Arc::new(ReceiptJournal::load(&path).unwrap());
        let (events, _handle) = attach(resumed.clone(), fake.clone()).unwrap();
        gate.notify_one();
        assert_eq!(
            collect(events).await.last(),
            Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
        );
        assert_eq!(fake.launched("git merge --ff-only"), 1);
        assert_eq!(fake.launched("deploy command"), 1);
        assert_eq!(resumed.snapshot().status, ReceiptStatus::Final);
        assert_eq!(fake.lock_owner(), None);
    }

    #[tokio::test]
    async fn attach_observes_journaled_but_unlaunched_step() {
        let fake = Arc::new(preflight().on("git merge --ff-only", &[], 0));
        let preview = prepared(&fake, false, &[]).await;
        let dir = TempReceipts::new();
        let journal = ReceiptJournal::create(&dir.0, "app", Path::new("/repo"), &preview).unwrap();
        journal.confirm().unwrap();
        journal.begin_step(0).unwrap();
        let path = journal.path().to_path_buf();
        drop(journal);

        let resumed = Arc::new(ReceiptJournal::load(&path).unwrap());
        let (events, _handle) = attach(resumed.clone(), fake.clone()).unwrap();
        assert_eq!(
            collect(events).await.last(),
            Some(&DeployEvent::Finished(DeployOutcome::AbortedBeforeChanges(
                AbortReason::ConnectFailed(
                    "the step was not launched before Shipslip exited".into()
                )
            )))
        );
        assert_eq!(fake.launched("git merge --ff-only"), 0);
        assert_eq!(
            resumed.snapshot().steps[0].status,
            ReceiptStepStatus::NotStarted
        );
    }

    #[tokio::test]
    async fn receipt_write_failure_prevents_step_launch() {
        let fake = Arc::new(preflight().on("git merge --ff-only", &[], 0));
        let preview = prepared(&fake, false, &[]).await;
        let dir = TempReceipts::new();
        let journal =
            Arc::new(ReceiptJournal::create(&dir.0, "app", Path::new("/repo"), &preview).unwrap());
        std::fs::remove_dir_all(&dir.0).unwrap();
        let confirmation = Confirmation::from(&preview, None).unwrap();
        assert!(matches!(
            execute_recorded(preview, confirmation, fake.clone(), journal),
            Err(ExecuteError::Receipt(_))
        ));
        assert_eq!(fake.launched("git merge --ff-only"), 0);
    }

    #[tokio::test]
    async fn receipt_claim_allows_only_one_local_owner() {
        let fake = preflight();
        let preview = prepared(&fake, false, &[]).await;
        let dir = TempReceipts::new();
        let owner = ReceiptJournal::create(&dir.0, "app", Path::new("/repo"), &preview).unwrap();
        let next = ReceiptJournal::load(owner.path()).unwrap();
        assert!(matches!(
            next.claim(),
            Err(crate::receipt::ReceiptError::Claimed(_))
        ));
        drop(owner);
        next.claim().unwrap();
    }
}
