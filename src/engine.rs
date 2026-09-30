//! Two-phase deploy: [`prepare`] (read-only) then [`execute`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};

use crate::event::{DeployEvent, DeployOutcome, StepStatus};
use crate::runner::{self, StepResult};
use crate::script::{is_full_sha, is_safe_branch, shell_quote, wrap_step};
use crate::transport::{Transport, TransportError};

/// One environment of one project, as resolved from config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployTarget {
    pub env: String,
    pub production: bool,
    pub ssh_alias: String,
    pub path: String,
    pub branch: String,
    /// Recipe steps 1..=N (step 0, the git fast-forward, is built in).
    pub steps: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    #[error("invalid target: {0}")]
    InvalidTarget(String),
    /// Preflight refused to continue; nothing on the server was changed.
    #[error("blocked: {0}")]
    Blocked(String),
    #[error("unexpected output from `{command}`: {output}")]
    UnexpectedOutput { command: String, output: String },
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
}

/// Controls for a running deploy.
#[derive(Debug, Clone)]
pub struct ExecutionHandle {
    stop: Arc<AtomicBool>,
    detach: watch::Sender<bool>,
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
    /// No-op until log watching exists.
    pub fn cancel_watch(&self) {}
}

/// Read-only preflight: resolves the current and target commits and builds
/// the [`Preview`]. Changes nothing on the server except fetching refs.
pub async fn prepare<T: Transport>(
    target: DeployTarget,
    transport: &T,
) -> Result<Preview, PrepareError> {
    if !is_safe_branch(&target.branch) {
        return Err(PrepareError::InvalidTarget(format!(
            "branch name `{}` is not supported",
            target.branch
        )));
    }

    let from_sha = capture_sha(transport, &target.path, "git rev-parse HEAD").await?;

    let b = &target.branch;
    let fetch = format!(
        "git fetch origin {refspec} && git rev-parse {remote}",
        refspec = shell_quote(&format!("+refs/heads/{b}:refs/remotes/origin/{b}")),
        remote = shell_quote(&format!("origin/{b}")),
    );
    let target_sha = capture_sha(transport, &target.path, &fetch).await?;

    if target_sha == from_sha {
        return Err(PrepareError::Blocked("up_to_date".into()));
    }

    let commits = capture(
        transport,
        &target.path,
        &format!("git log --oneline {from_sha}..{target_sha}"),
    )
    .await?;

    let recipe_hash = recipe_hash(&target);
    Ok(Preview {
        target,
        from_sha,
        target_sha,
        commits,
        recipe_hash,
        run_id: new_run_id(),
    })
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
    if !confirmation.matches(&preview) {
        return Err(ExecuteError::ConfirmationMismatch);
    }

    let (events, rx) = mpsc::unbounded_channel();
    let (detach_tx, detach_rx) = watch::channel(false);
    let handle = ExecutionHandle {
        stop: Arc::new(AtomicBool::new(false)),
        detach: detach_tx,
    };
    let stop = handle.stop.clone();

    tokio::spawn(run_steps(preview, transport, events, stop, detach_rx));
    Ok((rx, handle))
}

async fn run_steps<T: Transport>(
    preview: Preview,
    transport: Arc<T>,
    events: mpsc::UnboundedSender<DeployEvent>,
    stop: Arc<AtomicBool>,
    mut detach: watch::Receiver<bool>,
) {
    let path = &preview.target.path;
    let mut steps = vec![(
        "git fast-forward".to_string(),
        format!("git merge --ff-only {}", preview.target_sha),
    )];
    steps.extend(preview.target.steps.iter().map(|s| (s.clone(), s.clone())));

    for (index, (name, body)) in steps.into_iter().enumerate() {
        // Detaching between steps leaves nothing running, so it is a stop.
        if stop.load(Ordering::SeqCst) || *detach.borrow() {
            let outcome = match index {
                0 => DeployOutcome::CancelledBeforeChanges,
                n => DeployOutcome::StoppedAfterStep(n - 1),
            };
            let _ = events.send(DeployEvent::Finished(outcome));
            return;
        }

        let _ = events.send(DeployEvent::StepStarted { index, name });

        let (line_tx, mut line_rx) = mpsc::unbounded_channel();
        let step = async {
            let tx = line_tx;
            let script = wrap_step(path, &body);
            runner::run_step(&*transport, &preview.run_id, index, &script, &tx).await
        };
        // All output for this step is delivered before its StepFinished.
        let forward = async {
            while let Some(line) = line_rx.recv().await {
                let _ = events.send(DeployEvent::Output { index, line });
            }
        };
        let result = tokio::select! {
            biased;
            _ = wait_for_detach(&mut detach) => {
                let _ = events.send(DeployEvent::Detached { index });
                return;
            }
            (r, ()) = async { tokio::join!(step, forward) } => r,
        };

        let (status, exit_code, outcome) = match result {
            StepResult::Exited(0) => (StepStatus::Ok, Some(0), None),
            StepResult::Exited(code) => (
                StepStatus::Failed,
                Some(code),
                Some(DeployOutcome::FailedAtStep(index)),
            ),
            StepResult::NotStarted(reason) => (
                StepStatus::NotStarted,
                None,
                Some(match index {
                    0 => DeployOutcome::AbortedBeforeChanges(reason),
                    n => DeployOutcome::StoppedAfterStep(n - 1),
                }),
            ),
            StepResult::Gone => (
                StepStatus::Unknown,
                None,
                Some(DeployOutcome::Unknown {
                    step: index,
                    reason: "the step's process ended without recording an exit code".into(),
                }),
            ),
            StepResult::Interrupted(reason) => {
                let _ = events.send(DeployEvent::Interrupted { index, reason });
                return;
            }
        };
        let _ = events.send(DeployEvent::StepFinished {
            index,
            status,
            exit_code,
        });
        if let Some(outcome) = outcome {
            let _ = events.send(DeployEvent::Finished(outcome));
            return;
        }
    }

    let _ = events.send(DeployEvent::Finished(DeployOutcome::Succeeded));
}

async fn wait_for_detach(detach: &mut watch::Receiver<bool>) {
    if detach.wait_for(|d| *d).await.is_err() {
        // Handle dropped without detaching: never resolve.
        std::future::pending::<()>().await;
    }
}

async fn capture<T: Transport>(
    transport: &T,
    path: &str,
    command: &str,
) -> Result<Vec<String>, PrepareError> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let code = transport.run(&wrap_step(path, command), tx).await?;
    let mut lines = Vec::new();
    while let Some(line) = rx.recv().await {
        lines.push(line);
    }
    if code != 0 {
        return Err(PrepareError::Blocked(format!(
            "`{command}` exited with {code}: {}",
            lines.join("\n")
        )));
    }
    Ok(lines)
}

async fn capture_sha<T: Transport>(
    transport: &T,
    path: &str,
    command: &str,
) -> Result<String, PrepareError> {
    let lines = capture(transport, path, command).await?;
    match lines.last().map(|l| l.trim()) {
        Some(sha) if is_full_sha(sha) => Ok(sha.to_string()),
        _ => Err(PrepareError::UnexpectedOutput {
            command: command.to_string(),
            output: lines.join("\n"),
        }),
    }
}

fn recipe_hash(target: &DeployTarget) -> String {
    let mut h = Sha256::new();
    let mut field = |bytes: &[u8]| {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    for part in [&target.env, &target.ssh_alias, &target.path, &target.branch] {
        field(part.as_bytes());
    }
    field(&[target.production as u8]);
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
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Mutex, OnceLock};

    use tokio::sync::Notify;

    use super::*;

    const FROM: &str = "1111111111111111111111111111111111111111";
    const TO: &str = "2222222222222222222222222222222222222222";

    /// Scripted transport. Preflight commands and steps are matched by the
    /// first rule whose needle appears in the script. Steps are simulated as
    /// the detached runner would behave on a server. Records every script.
    #[derive(Default)]
    struct Fake {
        rules: Vec<Rule>,
        ran: Mutex<Vec<String>>,
        launched: Mutex<HashMap<usize, usize>>,
        reconnect_fails: bool,
        reconnects: AtomicUsize,
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

        fn unreachable_after_drop(mut self) -> Self {
            self.reconnect_fails = true;
            self
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
            let index = number_after(script, "step-");
            let rule = self.matching(script);
            let mut launched = self.launched.lock().unwrap();
            if launched.contains_key(&index) {
                return Ok(1);
            }
            match self.rules[rule].launch {
                Launch::Unreachable => return Err(TransportError::Connect("refused".into())),
                Launch::LostBeforeStart => return Err(lost()),
                Launch::Ok | Launch::LostAfterStart => {}
            }
            launched.insert(index, rule);
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
            let index = number_after(script, "step-");
            let skip = number_after(script, "tail -n +") - 1;
            let Some(&rule) = self.launched.lock().unwrap().get(&index) else {
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
            let index = number_after(script, "step-");
            match self.launched.lock().unwrap().get(&index) {
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
            if script.contains("setsid") {
                self.launch_step(script)
            } else if script.contains(runner::LOG_START) {
                self.observe(script, &output).await
            } else if script.contains("not-started") {
                let _ = output.send(self.probe(script));
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
        }
    }

    fn preflight() -> Fake {
        Fake::default()
            .on("git rev-parse HEAD", &[FROM], 0)
            .on("git fetch origin", &[TO], 0)
            .on(
                "git log --oneline",
                &["2222222 Fix checkout", "1a2b3c4 Add invoices"],
                0,
            )
    }

    async fn prepared(fake: &Fake, production: bool, steps: &[&str]) -> Preview {
        prepare(target(production, steps), fake).await.unwrap()
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
        let fake =
            Fake::default()
                .on("git rev-parse HEAD", &[FROM], 0)
                .on("git fetch origin", &[FROM], 0);
        let err = prepare(target(false, &[]), &fake).await.unwrap_err();
        assert!(matches!(err, PrepareError::Blocked(r) if r == "up_to_date"));
    }

    #[tokio::test]
    async fn prepare_rejects_garbage_sha() {
        let fake = Fake::default().on("git rev-parse HEAD", &["not a sha"], 0);
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
        let fake = preflight();
        let a = prepared(&fake, false, &["echo a"]).await;
        let b = prepared(&fake, false, &["echo b"]).await;
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
        assert_eq!(fake.ran_matching(&format!("git merge --ff-only {TO}")), 1);
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
            Some(&DeployEvent::Finished(DeployOutcome::FailedAtStep(1)))
        );
        assert_eq!(fake.ran_matching("optimize"), 0);
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
            Some(&DeployEvent::Finished(DeployOutcome::FailedAtStep(1)))
        );
    }

    async fn deploy(fake: &Arc<Fake>, steps: &[&str]) -> Vec<DeployEvent> {
        let p = prepared(fake, false, steps).await;
        let c = Confirmation::from(&p, None).unwrap();
        let (rx, _h) = execute(p, c, fake.clone()).unwrap();
        collect(rx).await
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
        assert_eq!(fake.ran_matching("migrate"), 1);
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
        assert_eq!(fake.ran_matching("migrate"), 1);
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
        assert_eq!(fake.ran_matching("migrate"), 1);
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
            Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep(0)))
        );
        assert_eq!(fake.ran_matching("migrate"), 1);
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
            Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep(1)))
        );
        assert_eq!(fake.ran_matching("migrate"), 0);
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
        assert_eq!(fake.ran_matching("git merge --ff-only"), 0);
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
        assert_eq!(fake.ran_matching("git merge --ff-only"), 0);
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
            Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep(1)))
        );
        assert_eq!(fake.ran_matching("migrate"), 0);
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
                    "could not connect: refused".into()
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
            Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep(0)))
        );
    }

    #[tokio::test]
    async fn confirmation_for_an_identical_earlier_preview_is_rejected() {
        let fake = preflight();
        let a = prepared(&fake, false, &["echo a"]).await;
        let b = prepared(&fake, false, &["echo a"]).await;
        assert_eq!(a.recipe_hash(), b.recipe_hash());
        assert_ne!(a.run_id(), b.run_id());
        let confirm_a = Confirmation::from(&a, None).unwrap();
        let err = execute(b, confirm_a, Arc::new(preflight())).unwrap_err();
        assert_eq!(err, ExecuteError::ConfirmationMismatch);
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
            assert_ne!(recipe_hash(&t), recipe_hash(&base), "{t:?}");
        }
    }
}
