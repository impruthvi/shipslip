use super::*;
use crate::fixture::Fixture;
use shipslip::DeployEvent;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Refuse(AtomicUsize);
struct Never;
impl Transport for Never {
    async fn run(
        &self,
        _: &str,
        _: tauri::async_runtime::Sender<String>,
        _: usize,
    ) -> Result<i32, TransportError> {
        panic!("no remote call expected")
    }
    async fn reconnect(&self) -> Result<(), TransportError> {
        panic!("no reconnect expected")
    }
}
impl Connector for Refuse {
    type Remote = Never;
    async fn connect(&self, _: &str) -> Result<Arc<Never>, TransportError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(TransportError::Connect("refused".into()))
    }
}

fn checkout(fx: &Fixture, name: &str) -> PathBuf {
    let path = fx.dir().join(name);
    std::fs::create_dir_all(&path).unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(
        path.join(".shipslip.toml"),
        r#"[project]
name = "example"
stack = "laravel"
[env.production]
ssh = "web1"
path = "/srv/app"
branch = "main"
production = true
[recipe.deploy]
steps = ["true"]
"#,
    )
    .unwrap();
    std::fs::canonicalize(path).unwrap()
}

#[test]
fn trust_approval_is_exact_single_use_and_does_not_contact_ssh() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let sessions = Sessions::new(fx.receipts(), fx.trust(), Refuse::default());
        let review = sessions
            .review_trust(root.clone(), "production".into())
            .await
            .unwrap();
        assert!(!review.changes.is_empty());
        assert_eq!(
            sessions
                .approve_trust(&review.handle, "staging")
                .await
                .unwrap_err()
                .code,
            ErrorCode::Confirmation
        );
        sessions
            .approve_trust(&review.handle, "production")
            .await
            .unwrap();
        let loaded = LoadedConfig::load(&root, None).unwrap();
        assert!(loaded.trusted_target("production", &fx.trust()).is_ok());
        assert_eq!(
            sessions
                .approve_trust(&review.handle, "production")
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidHandle
        );
        assert_eq!(sessions.connector.0.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn changed_settings_and_reload_require_a_new_trust_review() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let sessions = Sessions::new(fx.receipts(), fx.trust(), Refuse::default());
        let review = sessions
            .review_trust(root.clone(), "production".into())
            .await
            .unwrap();
        let path = root.join(".shipslip.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, text.replace("/srv/app", "/srv/other")).unwrap();
        assert_eq!(
            sessions
                .approve_trust(&review.handle, "production")
                .await
                .unwrap_err()
                .code,
            ErrorCode::Changed
        );
        let review = sessions
            .review_trust(root, "production".into())
            .await
            .unwrap();
        sessions.invalidate_page();
        assert_eq!(
            sessions
                .approve_trust(&review.handle, "production")
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidHandle
        );
        assert!(!fx.trust().exists());
    });
}

#[test]
fn prepare_requires_trust_before_ssh_and_preserves_connection_diagnostics() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let sessions = Sessions::new(fx.receipts(), fx.trust(), Refuse::default());
        assert_eq!(
            sessions
                .prepare(root.clone(), "production".into(), shipslip::RunPlan::Deploy)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Untrusted
        );
        assert_eq!(sessions.connector.0.load(Ordering::SeqCst), 0);
        let ticket = sessions
            .review_trust(root.clone(), "production".into())
            .await
            .unwrap();
        sessions
            .approve_trust(&ticket.handle, "production")
            .await
            .unwrap();
        let error = sessions
            .prepare(root, "production".into(), shipslip::RunPlan::Deploy)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Connection);
        assert_eq!(error.connection_kind, Some("other"));
        assert!(error.message.contains("refused"));
    });
}

#[derive(Default)]
struct Remote {
    held: std::sync::atomic::AtomicBool,
    released: AtomicUsize,
    fail_release: std::sync::atomic::AtomicBool,
    launches: AtomicUsize,
    output_lines: AtomicUsize,
    observe_entered: AtomicUsize,
    observe_gate: tauri::async_runtime::Mutex<Option<tauri::async_runtime::Receiver<()>>>,
    preflight_entered: AtomicUsize,
    preflight_gate: tauri::async_runtime::Mutex<Option<tauri::async_runtime::Receiver<()>>>,
    state_entered: AtomicUsize,
    state_gate: tauri::async_runtime::Mutex<Option<tauri::async_runtime::Receiver<()>>>,
}
impl Connector for Arc<Remote> {
    type Remote = Remote;
    async fn connect(&self, _: &str) -> Result<Arc<Remote>, TransportError> {
        Ok(self.clone())
    }
}
impl Transport for Remote {
    async fn run(
        &self,
        script: &str,
        output: tauri::async_runtime::Sender<String>,
        _: usize,
    ) -> Result<i32, TransportError> {
        let lines = if script.contains("echo acquired") {
            assert!(
                !self.held.swap(true, Ordering::SeqCst),
                "duplicate remote lock acquisition"
            );
            vec!["acquired".into()]
        } else if script.contains("released-") {
            if self.fail_release.load(Ordering::SeqCst) {
                return Err(TransportError::Connect("cleanup refused".into()));
            }
            self.held.store(false, Ordering::SeqCst);
            self.released.fetch_add(1, Ordering::SeqCst);
            vec!["released".into()]
        } else if script.contains("setsid nohup") {
            assert!(
                script.contains("maintenance-up"),
                "unexpected recipe launch"
            );
            self.launches.fetch_add(1, Ordering::SeqCst);
            vec![]
        } else if script.contains("echo shipslip-log-start") {
            self.observe_entered.fetch_add(1, Ordering::SeqCst);
            if let Some(mut gate) = self.observe_gate.lock().await.take() {
                gate.recv().await;
            }
            let count = self.output_lines.load(Ordering::SeqCst);
            let mut lines = vec!["shipslip-log-start".into()];
            if count == 0 {
                lines.push("maintenance finished".into());
            } else {
                lines.extend((0..count).map(|n| format!("line {n}")));
            }
            lines
        } else if script.contains("echo \"exited ") {
            vec!["exited 0".into()]
        } else if script.contains("@target") {
            self.preflight_entered.fetch_add(1, Ordering::SeqCst);
            if let Some(mut gate) = self.preflight_gate.lock().await.take() {
                gate.recv().await;
            }
            vec![
                format!("@head {}", crate::fixture::OLD),
                "@branch main".into(),
                format!("@target {}", crate::fixture::NEW),
                format!("@fetch_head {}", crate::fixture::NEW),
                "@ancestor".into(),
                "@commit 2222222 release".into(),
            ]
        } else if script.contains("then echo owned") || script.contains("heartbeat.tmp") {
            vec![if self.held.load(Ordering::SeqCst) {
                "owned"
            } else {
                "lost"
            }
            .into()]
        } else if script.contains("@head") {
            self.state_entered.fetch_add(1, Ordering::SeqCst);
            if let Some(mut gate) = self.state_gate.lock().await.take() {
                gate.recv().await;
            }
            // A dirty checkout stops the run at its recheck, before any step.
            vec![
                format!("@head {}", crate::fixture::OLD),
                "@branch main".into(),
                "@dirty ?? changed".into(),
            ]
        } else if script.contains("@missing") {
            vec!["@missing".into()]
        } else {
            panic!("unexpected remote operation: {script}")
        };
        for line in lines {
            output.send(line).await.unwrap();
        }
        Ok(0)
    }
    async fn reconnect(&self) -> Result<(), TransportError> {
        Ok(())
    }
}
async fn approve<C: Connector>(sessions: &Arc<Sessions<C>>, root: &std::path::Path) {
    let review = sessions
        .review_trust(root.into(), "production".into())
        .await
        .unwrap();
    sessions
        .approve_trust(&review.handle, "production")
        .await
        .unwrap();
}

#[test]
fn preview_cancel_is_single_use_and_reload_releases_its_lock() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let preview = sessions
            .prepare(root.clone(), "production".into(), shipslip::RunPlan::Deploy)
            .await
            .unwrap();
        assert_eq!(preview.target_sha, crate::fixture::NEW);
        assert_eq!(
            sessions
                .prepare(root.clone(), "production".into(), shipslip::RunPlan::Deploy)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
        sessions.cancel(&preview.handle).await.unwrap();
        assert!(!remote.held.load(Ordering::SeqCst));
        assert_eq!(
            sessions.cancel(&preview.handle).await.unwrap_err().code,
            ErrorCode::InvalidHandle
        );
        let preview = sessions
            .prepare(root, "production".into(), shipslip::RunPlan::Deploy)
            .await
            .unwrap();
        sessions.invalidate_page();
        assert_eq!(
            sessions.cancel(&preview.handle).await.unwrap_err().code,
            ErrorCode::InvalidHandle
        );
        // Wait for the observable SSH cleanup, with a bounded deadline.
        let started = std::time::Instant::now();
        while remote.released.load(Ordering::SeqCst) < 2 {
            assert!(started.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!remote.held.load(Ordering::SeqCst));
    });
}

#[test]
fn confirmation_rejects_wrong_environment_and_changed_config_without_launch() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let review = sessions
            .prepare(root.clone(), "production".into(), shipslip::RunPlan::Deploy)
            .await
            .unwrap();
        assert_eq!(
            sessions
                .confirm(&review.handle, Some("staging".into()))
                .await
                .unwrap_err()
                .code,
            ErrorCode::Confirmation
        );
        let path = root.join(".shipslip.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, text.replace("[\"true\"]", "[\"echo changed\"]")).unwrap();
        assert_eq!(
            sessions
                .confirm(&review.handle, Some("production".into()))
                .await
                .unwrap_err()
                .code,
            ErrorCode::Changed
        );
        assert!(!remote.held.load(Ordering::SeqCst));
        assert_eq!(
            sessions
                .confirm(&review.handle, Some("production".into()))
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidHandle
        );
        assert!(sessions.current_runs().runs.is_empty());
    });
}

#[test]
fn up_review_requires_confirmation_and_expires_on_reload_without_ssh() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let sessions = Sessions::new(fx.receipts(), fx.trust(), Refuse::default());
        approve(&sessions, &root).await;
        let review = sessions.review_up(root, "production".into()).await.unwrap();
        assert_eq!(review.operation, "up");
        assert_eq!(review.remaining_steps, ["php artisan up"]);
        assert_eq!(
            sessions
                .confirm(&review.handle, None)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Confirmation
        );
        assert_eq!(sessions.connector.0.load(Ordering::SeqCst), 0);
        sessions.invalidate_page();
        assert_eq!(
            sessions
                .confirm(&review.handle, Some("production".into()))
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidHandle
        );
        assert_eq!(sessions.connector.0.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn attach_reviews_saved_steps_and_refuses_a_changed_receipt_before_ssh() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Remote::default();
        let sessions = Sessions::new(fx.receipts(), fx.trust(), Refuse::default());
        approve(&sessions, &root).await;
        let trusted = LoadedConfig::load(&root, None)
            .unwrap()
            .trusted_target("production", &fx.trust())
            .unwrap();
        let preview = shipslip::prepare(&trusted, &remote).await.unwrap();
        let journal =
            ReceiptJournal::create(&fx.receipts(), trusted.project(), &root, &preview).unwrap();
        let path = journal.path().to_owned();
        drop(journal);
        shipslip::cancel(preview, &remote).await.unwrap();
        let review = sessions
            .review_attach(root, "production".into())
            .await
            .unwrap();
        assert_eq!(review.operation, "attach");
        assert_eq!(review.target_sha.as_deref(), Some(crate::fixture::NEW));
        assert_eq!(review.remaining_steps.len(), 2);
        assert_eq!(
            sessions
                .confirm(&review.handle, None)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Confirmation
        );
        std::fs::write(path, "{}").unwrap();
        assert_eq!(
            sessions
                .confirm(&review.handle, Some("production".into()))
                .await
                .unwrap_err()
                .code,
            ErrorCode::Changed
        );
        assert_eq!(sessions.connector.0.load(Ordering::SeqCst), 0);
    });
}

fn until(mut ready: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !ready() {
        assert!(
            started.elapsed().as_secs() < 5,
            "background operation did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn reload_abandons_preparing_without_aborting_and_then_cancels_the_preview() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let (release, gate) = tauri::async_runtime::channel(1);
        *remote.preflight_gate.lock().await = Some(gate);
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let owner = sessions.clone();
        let path = root.clone();
        let request = tauri::async_runtime::spawn(async move {
            owner
                .prepare(path, "production".into(), RunPlan::Deploy)
                .await
        });
        until(|| remote.preflight_entered.load(Ordering::SeqCst) == 1);
        assert_eq!(
            sessions
                .prepare(root, "production".into(), RunPlan::Deploy)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
        sessions.invalidate_page();
        release.send(()).await.unwrap();
        assert_eq!(
            request.await.unwrap().unwrap_err().code,
            ErrorCode::Cancelled
        );
        assert_eq!(remote.released.load(Ordering::SeqCst), 1);
        assert!(!remote.held.load(Ordering::SeqCst));
    });
}

#[test]
fn confirmed_run_is_single_use_survives_reload_and_finishes_its_receipt() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let (release, gate) = tauri::async_runtime::channel(1);
        *remote.state_gate.lock().await = Some(gate);
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let review = sessions
            .prepare(root.clone(), "production".into(), RunPlan::Deploy)
            .await
            .unwrap();
        let run = sessions
            .confirm(&review.handle, Some("production".into()))
            .await
            .unwrap();
        until(|| remote.state_entered.load(Ordering::SeqCst) > 0);
        assert_eq!(
            sessions
                .confirm(&review.handle, Some("production".into()))
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidHandle
        );
        sessions.invalidate_page();
        let active = sessions.current_runs().runs;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].info.handle, run.handle);
        assert!(remote.held.load(Ordering::SeqCst));
        let saved = receipt::find_open(&fx.receipts(), "example", "production", &root)
            .unwrap()
            .unwrap();
        assert!(receipt::read(&saved).unwrap().confirmed);
        release.send(()).await.unwrap();
        until(|| sessions.current_runs().runs.is_empty());
        let finished = receipt::read(&saved).unwrap();
        assert_eq!(finished.status, ReceiptStatus::Final);
        assert!(matches!(
            finished.outcome,
            Some(shipslip::DeployOutcome::AbortedBeforeChanges(_))
        ));
        assert!(!remote.held.load(Ordering::SeqCst));
    });
}

#[test]
fn journal_creation_failure_cancels_preview_before_launch() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let bad_root = fx.dir().join("not-a-directory");
        std::fs::write(&bad_root, "x").unwrap();
        let sessions = Sessions::new(bad_root, fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let review = sessions
            .prepare(root, "production".into(), RunPlan::Deploy)
            .await
            .unwrap();
        assert_eq!(
            sessions
                .confirm(&review.handle, Some("production".into()))
                .await
                .unwrap_err()
                .code,
            ErrorCode::Local
        );
        assert!(!remote.held.load(Ordering::SeqCst));
        assert_eq!(remote.state_entered.load(Ordering::SeqCst), 0);
        assert!(sessions.current_runs().runs.is_empty());
    });
}

struct Fleet {
    one: Arc<Remote>,
    two: Arc<Remote>,
}
impl Connector for Fleet {
    type Remote = Remote;
    async fn connect(&self, alias: &str) -> Result<Arc<Remote>, TransportError> {
        Ok(if alias == "web1" {
            self.one.clone()
        } else {
            self.two.clone()
        })
    }
}

#[test]
fn two_checkouts_can_hold_production_previews_independently() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let one = checkout(&fx, "one");
        let two = checkout(&fx, "two");
        let path = two.join(".shipslip.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, text.replace("web1", "web2")).unwrap();
        let a = Arc::new(Remote::default());
        let b = Arc::new(Remote::default());
        let sessions = Sessions::new(
            fx.receipts(),
            fx.trust(),
            Fleet {
                one: a.clone(),
                two: b.clone(),
            },
        );
        approve(&sessions, &one).await;
        approve(&sessions, &two).await;
        let first = sessions
            .prepare(one, "production".into(), RunPlan::Deploy)
            .await
            .unwrap();
        let second = sessions
            .prepare(two, "production".into(), RunPlan::Deploy)
            .await
            .unwrap();
        assert_ne!(first.handle, second.handle);
        sessions.cancel(&first.handle).await.unwrap();
        assert!(!a.held.load(Ordering::SeqCst));
        assert!(b.held.load(Ordering::SeqCst));
        sessions.cancel(&second.handle).await.unwrap();
        assert!(!b.held.load(Ordering::SeqCst));
    });
}

#[test]
fn confirmed_up_is_a_recorded_maintenance_run_with_no_deploy_steps() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let review = sessions
            .review_up(root.clone(), "production".into())
            .await
            .unwrap();
        let run = sessions
            .confirm(&review.handle, Some("production".into()))
            .await
            .unwrap();
        assert_eq!(run.operation, "up");
        until(|| sessions.current_runs().runs.is_empty());
        assert!(!remote.held.load(Ordering::SeqCst));
        let dir = fx.receipts().join("example/production");
        let path = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|ext| ext == "json"))
            .unwrap();
        let saved = receipt::read(&path).unwrap();
        assert_eq!(saved.kind, receipt::ReceiptKind::Up);
        assert_eq!(saved.status, ReceiptStatus::Final);
        assert_eq!(saved.outcome, Some(shipslip::DeployOutcome::Succeeded));
        assert_eq!(saved.steps.len(), 1);
        assert_eq!(saved.steps[0].command, "php artisan up");
        assert_eq!(saved.steps[0].output, ["maintenance finished"]);
        assert_eq!(remote.preflight_entered.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn confirmed_attach_observes_recorded_up_without_launching_it_again() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let trusted = LoadedConfig::load(&root, None)
            .unwrap()
            .trusted_target("production", &fx.trust())
            .unwrap();
        let (_release, gate) = tauri::async_runtime::channel(1);
        *remote.observe_gate.lock().await = Some(gate);
        let (mut events, control) =
            shipslip::bring_app_up(&trusted, remote.clone(), &fx.receipts())
                .await
                .unwrap();
        until(|| remote.observe_entered.load(Ordering::SeqCst) == 1);
        control.detach();
        let mut detached = false;
        while let Some(event) = events.recv().await {
            detached |= matches!(event, DeployEvent::Detached { .. });
        }
        assert!(detached);
        assert_eq!(remote.launches.load(Ordering::SeqCst), 1);
        let review = sessions
            .review_attach(root.clone(), "production".into())
            .await
            .unwrap();
        let path = receipt::find_open(&fx.receipts(), "example", "production", &root)
            .unwrap()
            .unwrap();
        sessions
            .confirm(&review.handle, Some("production".into()))
            .await
            .unwrap();
        until(|| sessions.current_runs().runs.is_empty());
        let saved = receipt::read(&path).unwrap();
        assert_eq!(saved.outcome, Some(shipslip::DeployOutcome::Succeeded));
        assert_eq!(saved.status, ReceiptStatus::Final);
        assert_eq!(remote.launches.load(Ordering::SeqCst), 1);
        assert!(!remote.held.load(Ordering::SeqCst));
    });
}

#[test]
fn reload_cleanup_failure_remains_visible_alongside_active_runs() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        sessions
            .prepare(root, "production".into(), RunPlan::Deploy)
            .await
            .unwrap();
        remote.fail_release.store(true, Ordering::SeqCst);
        sessions.invalidate_page();
        until(|| !sessions.current_runs().warnings.is_empty());
        let status = sessions.current_runs();
        assert!(status.runs.is_empty());
        assert!(status.warnings[0].message.contains("cleanup refused"));
        assert!(status.warnings[0].message.contains("production"));
        assert!(remote.held.load(Ordering::SeqCst));
    });
}

#[test]
fn abandoned_prepare_cleanup_failure_survives_a_dropped_request() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let (release, gate) = tauri::async_runtime::channel(1);
        *remote.preflight_gate.lock().await = Some(gate);
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let owner = sessions.clone();
        let request = tauri::async_runtime::spawn(async move {
            owner
                .prepare(root, "production".into(), RunPlan::Deploy)
                .await
        });
        until(|| remote.preflight_entered.load(Ordering::SeqCst) == 1);
        request.abort();
        sessions.invalidate_page();
        remote.fail_release.store(true, Ordering::SeqCst);
        release.send(()).await.unwrap();
        until(|| !sessions.current_runs().warnings.is_empty());
        assert!(sessions.current_runs().warnings[0]
            .message
            .contains("cleanup refused"));
    });
}

struct Diagnostic(&'static str);
impl Connector for Diagnostic {
    type Remote = Never;
    async fn connect(&self, _: &str) -> Result<Arc<Never>, TransportError> {
        Err(TransportError::Connect(self.0.into()))
    }
}

#[test]
fn ssh_refusals_are_classified_and_failed_starts_release_the_reservation() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        for (message, kind) in [
            ("Host key verification failed.", "hostKeyUnknown"),
            ("REMOTE HOST IDENTIFICATION HAS CHANGED!", "hostKeyChanged"),
            ("Permission denied (publickey).", "keyNotLoaded"),
        ] {
            let sessions = Sessions::new(fx.receipts(), fx.trust(), Diagnostic(message));
            approve(&sessions, &root).await;
            let error = sessions
                .prepare(root.clone(), "production".into(), RunPlan::Deploy)
                .await
                .unwrap_err();
            assert_eq!(error.connection_kind, Some(kind));
            let review = sessions
                .review_up(root.clone(), "production".into())
                .await
                .unwrap();
            let error = sessions
                .confirm(&review.handle, Some("production".into()))
                .await
                .unwrap_err();
            assert_eq!(error.connection_kind, Some(kind));
            assert!(sessions.current_runs().runs.is_empty());
            let retry = sessions
                .review_up(root.clone(), "production".into())
                .await
                .unwrap();
            sessions.cancel(&retry.handle).await.unwrap();
        }
    });
}

#[test]
fn trust_write_failure_keeps_the_exact_review_available_for_retry() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let parent = fx.dir().join("blocked");
        std::fs::write(&parent, "x").unwrap();
        let trust_path = parent.join("trust.json");
        let sessions = Sessions::new(fx.receipts(), trust_path.clone(), Refuse::default());
        // Read an absent store first; then make its parent unwritable as a file.
        std::fs::remove_file(&parent).unwrap();
        let review = sessions
            .review_trust(root.clone(), "production".into())
            .await
            .unwrap();
        std::fs::write(&parent, "x").unwrap();
        assert_eq!(
            sessions
                .approve_trust(&review.handle, "production")
                .await
                .unwrap_err()
                .code,
            ErrorCode::Local
        );
        std::fs::remove_file(parent).unwrap();
        sessions
            .approve_trust(&review.handle, "production")
            .await
            .unwrap();
        assert!(LoadedConfig::load(&root, None)
            .unwrap()
            .trusted_target("production", &trust_path)
            .is_ok());
        assert_eq!(sessions.connector.0.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn completed_run_replays_the_newest_two_thousand_output_lines() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        remote.output_lines.store(2_100, Ordering::SeqCst);
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let review = sessions.review_up(root, "production".into()).await.unwrap();
        let run = sessions
            .confirm(&review.handle, Some("production".into()))
            .await
            .unwrap();
        until(|| sessions.current_runs().runs.is_empty());
        let recent = sessions.current_runs().recent;
        let snapshot = &recent[0];
        assert_eq!(snapshot.info.handle, run.handle);
        assert_eq!(snapshot.output.len(), 2_000);
        assert_eq!(snapshot.dropped_output, 100);
        assert_eq!(snapshot.output[0].text, "line 100");
        assert_eq!(snapshot.output[1_999].text, "line 2099");
        assert!(snapshot.state.terminal.is_some());
        assert!(!remote.held.load(Ordering::SeqCst));
    });
}

#[test]
fn replay_retains_maintenance_progress_and_output_source_after_completion() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote);
        approve(&sessions, &root).await;
        let review = sessions.review_up(root, "production".into()).await.unwrap();
        sessions
            .confirm(&review.handle, Some("production".into()))
            .await
            .unwrap();
        until(|| sessions.current_runs().runs.is_empty());
        let recent = sessions.current_runs().recent;
        let replay = serde_json::to_value(&recent[0]).unwrap();
        assert_eq!(replay["state"]["maintenanceUp"]["status"], "Ok");
        assert_eq!(replay["state"]["maintenanceUp"]["exitCode"], 0);
        assert_eq!(replay["output"][0]["source"]["kind"], "maintenance");
        assert_eq!(replay["output"][0]["source"]["phase"], "Up");
        assert_eq!(replay["state"]["terminal"]["succeeded"], true);
        assert_eq!(replay["state"]["terminal"]["unknown"], false);
        assert_eq!(replay["info"]["stage"], "finished");
    });
}

fn capture_channel() -> (
    tauri::ipc::Channel<events::RunBatch>,
    std::sync::mpsc::Receiver<serde_json::Value>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let channel = tauri::ipc::Channel::new(move |body| {
        let tauri::ipc::InvokeResponseBody::Json(json) = body else {
            panic!("expected a JSON batch");
        };
        tx.send(serde_json::from_str(&json).unwrap()).unwrap();
        Ok(())
    });
    (channel, rx)
}

#[test]
fn a_frozen_ui_receives_one_batch_while_the_run_finishes_and_can_catch_up() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        remote.output_lines.store(2_100, Ordering::SeqCst);
        let (release, gate) = tauri::async_runtime::channel(1);
        *remote.observe_gate.lock().await = Some(gate);
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let review = sessions
            .review_up(root.clone(), "production".into())
            .await
            .unwrap();
        let run = sessions
            .confirm(&review.handle, Some("production".into()))
            .await
            .unwrap();
        until(|| remote.observe_entered.load(Ordering::SeqCst) == 1);
        let saved = receipt::find_open(&fx.receipts(), "example", "production", &root)
            .unwrap()
            .unwrap();
        let (channel, received) = capture_channel();
        let subscription = sessions.subscribe_run(&run.handle, channel).unwrap();
        let first = received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(first["subscription"], subscription);
        assert_eq!(first["replay"], true);
        release.send(()).await.unwrap();
        until(|| sessions.current_runs().runs.is_empty());
        assert!(!remote.held.load(Ordering::SeqCst));
        assert_eq!(receipt::read(&saved).unwrap().status, ReceiptStatus::Final);
        assert!(received
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        sessions
            .ack_run(
                &run.handle,
                &subscription,
                first["through"].as_u64().unwrap(),
            )
            .unwrap();
        let catchup = received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(catchup["replay"], false);
        assert_eq!(catchup["output"].as_array().unwrap().len(), 2_000);
        assert_eq!(catchup["output"][0]["text"], "line 100");
        assert_eq!(catchup["droppedOutput"], 100);
        assert_eq!(catchup["state"]["terminal"]["succeeded"], true);
        sessions
            .ack_run(
                &run.handle,
                &subscription,
                catchup["through"].as_u64().unwrap(),
            )
            .unwrap();
    });
}

#[test]
fn reload_replaces_the_view_subscription_and_replays_without_remote_attach() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let remote = Arc::new(Remote::default());
        remote.output_lines.store(2_100, Ordering::SeqCst);
        let (release, gate) = tauri::async_runtime::channel(1);
        *remote.observe_gate.lock().await = Some(gate);
        let sessions = Sessions::new(fx.receipts(), fx.trust(), remote.clone());
        approve(&sessions, &root).await;
        let review = sessions.review_up(root, "production".into()).await.unwrap();
        let run = sessions
            .confirm(&review.handle, Some("production".into()))
            .await
            .unwrap();
        until(|| remote.observe_entered.load(Ordering::SeqCst) == 1);
        let (old_channel, old_packets) = capture_channel();
        let old = sessions.subscribe_run(&run.handle, old_channel).unwrap();
        let initial = old_packets
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        sessions.invalidate_page();
        assert_eq!(
            sessions
                .ack_run(&run.handle, &old, initial["through"].as_u64().unwrap())
                .unwrap_err()
                .code,
            ErrorCode::InvalidHandle
        );
        assert_eq!(sessions.current_runs().runs[0].info.handle, run.handle);
        release.send(()).await.unwrap();
        until(|| sessions.current_runs().runs.is_empty());
        let (channel, packets) = capture_channel();
        let subscription = sessions.subscribe_run(&run.handle, channel).unwrap();
        let replay = packets
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_ne!(old, subscription);
        assert_eq!(replay["replay"], true);
        assert_eq!(replay["output"][0]["text"], "line 100");
        assert_eq!(replay["state"]["terminal"]["succeeded"], true);
        assert_eq!(remote.launches.load(Ordering::SeqCst), 1);
        assert_eq!(remote.observe_entered.load(Ordering::SeqCst), 1);
        sessions
            .unsubscribe_run(&run.handle, &subscription)
            .unwrap();
        assert_eq!(
            sessions
                .ack_run(
                    &run.handle,
                    &subscription,
                    replay["through"].as_u64().unwrap()
                )
                .unwrap_err()
                .code,
            ErrorCode::InvalidHandle
        );
    });
}

fn display_feed() -> Arc<RunFeed> {
    RunFeed::new(RunInfo {
        handle: "display-run".into(),
        checkout: "/work/app".into(),
        env: "production".into(),
        run_id: Some("recorded-run".into()),
        target_sha: Some(crate::fixture::NEW.into()),
        operation: "deploy",
        stage: "starting".into(),
    })
}

#[test]
fn event_replay_preserves_step_results_and_untrusted_text_as_display_data() {
    let feed = display_feed();
    feed.record(DeployEvent::StepStarted {
        index: 2,
        name: "migrate\x1b[2J".into(),
    });
    feed.record(DeployEvent::Output {
        index: 2,
        line: "<b>error</b>\r\x1b[2J".into(),
    });
    feed.record(DeployEvent::StepFinished {
        index: 2,
        status: shipslip::StepStatus::Failed,
        exit_code: Some(9),
    });
    feed.record(DeployEvent::RunError {
        reason: "journal\nfailed".into(),
    });
    feed.close();
    let view = serde_json::to_value(feed.snapshot()).unwrap();
    assert_eq!(view["state"]["steps"]["2"]["name"], "migrate\\x1b[2J");
    assert_eq!(view["state"]["steps"]["2"]["status"], "Failed");
    assert_eq!(view["state"]["steps"]["2"]["exitCode"], 9);
    assert_eq!(view["output"][0]["text"], "<b>error</b>\\x0d\\x1b[2J");
    assert_eq!(view["state"]["terminal"]["kind"], "error");
    assert_eq!(view["state"]["terminal"]["reason"], "journal\\x0afailed");
    assert_eq!(view["info"]["stage"], "needs attention");
}

#[test]
fn replay_keeps_log_watch_smoke_and_unknown_result_details_safely() {
    let feed = display_feed();
    feed.record(DeployEvent::Warning {
        reason: "warning\x1b[2J".into(),
    });
    feed.record(DeployEvent::AppLeftDown);
    feed.record(DeployEvent::ServerState {
        head: Some(crate::fixture::NEW.into()),
        tree_dirty: Some(true),
    });
    feed.record(DeployEvent::NewLogError {
        phase: shipslip::LogPhase::After,
        message: "boom\x1b[2J".into(),
        file_line: Some("app.php:3\r".into()),
    });
    feed.record(DeployEvent::WatchStarted {
        window: std::time::Duration::from_secs(120),
    });
    let mut watch = shipslip::WatchResult::not_run();
    watch.status = shipslip::WatchStatus::Partial;
    watch.log_path = Some("storage/logs/app\n.log".into());
    watch.warnings.push("read\nfailed".into());
    watch.new_errors.push(shipslip::ErrorGroup {
        exception: "Exception\x1b[2J".into(),
        file: None,
        count: 1,
        overflow_variants: 0,
        variants: vec![shipslip::ErrorVariant {
            signature: "sig".into(),
            level: "ERROR".into(),
            message: "boom\x1b[2J".into(),
            display_file_line: None,
            count: 1,
            phase: shipslip::LogPhase::After,
        }],
    });
    feed.record(DeployEvent::WatchFinished(watch));
    feed.record(DeployEvent::SmokeFinished(shipslip::SmokeResult::Failed {
        status: Some(503),
        reason: "unavailable\nnow".into(),
    }));
    feed.record(DeployEvent::Finished(shipslip::DeployOutcome::Unknown {
        step: 2,
        reason: "lost\nconnection".into(),
    }));
    feed.close();
    let view = serde_json::to_value(feed.snapshot()).unwrap();
    assert_eq!(view["state"]["warnings"][0], "warning\\x1b[2J");
    assert_eq!(view["state"]["appLeftDown"], true);
    assert_eq!(view["state"]["treeDirty"], true);
    assert_eq!(view["output"][0]["source"]["kind"], "log");
    assert_eq!(view["output"][0]["source"]["fileLine"], "app.php:3\\x0d");
    assert_eq!(view["state"]["watchWindowMs"], 120_000);
    assert_eq!(
        view["state"]["watch"]["groups"][0]["exception"],
        "Exception\\x1b[2J"
    );
    assert_eq!(view["state"]["watch"]["warnings"][0], "read\\x0afailed");
    assert_eq!(view["state"]["smoke"]["httpStatus"], 503);
    assert_eq!(view["state"]["smoke"]["reason"], "unavailable\\x0anow");
    assert_eq!(view["state"]["terminal"]["unknown"], true);
    assert_eq!(view["state"]["terminal"]["succeeded"], false);
}

struct DelayedRefusal {
    entered: Arc<AtomicUsize>,
    release: tauri::async_runtime::Mutex<tauri::async_runtime::Receiver<()>>,
}
impl Connector for DelayedRefusal {
    type Remote = Never;
    async fn connect(&self, _: &str) -> Result<Arc<Never>, TransportError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.release.lock().await.recv().await;
        Err(TransportError::Connect("refused after reload".into()))
    }
}

#[test]
fn a_startup_failure_after_reload_remains_available_to_the_new_view() {
    tauri::async_runtime::block_on(async {
        let fx = Fixture::new();
        let root = checkout(&fx, "one");
        let entered = Arc::new(AtomicUsize::new(0));
        let (release, gate) = tauri::async_runtime::channel(1);
        let sessions = Sessions::new(
            fx.receipts(),
            fx.trust(),
            DelayedRefusal {
                entered: entered.clone(),
                release: tauri::async_runtime::Mutex::new(gate),
            },
        );
        approve(&sessions, &root).await;
        let review = sessions.review_up(root, "production".into()).await.unwrap();
        let handle = review.handle.clone();
        let owner = sessions.clone();
        let request = tauri::async_runtime::spawn(async move {
            owner
                .confirm(&review.handle, Some("production".into()))
                .await
        });
        until(|| entered.load(Ordering::SeqCst) == 1);
        let (channel, received) = capture_channel();
        sessions.subscribe_run(&handle, channel).unwrap();
        assert_eq!(
            received
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()["state"]["terminal"],
            serde_json::Value::Null
        );
        sessions.invalidate_page();
        release.send(()).await.unwrap();
        assert_eq!(
            request.await.unwrap().unwrap_err().code,
            ErrorCode::Connection
        );
        assert!(sessions.current_runs().runs.is_empty());
        let (channel, received) = capture_channel();
        let subscription = sessions.subscribe_run(&handle, channel).unwrap();
        let batch = received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(batch["state"]["terminal"]["kind"], "error");
        assert!(batch["state"]["terminal"]["reason"]
            .as_str()
            .unwrap()
            .contains("refused after reload"));
        sessions.unsubscribe_run(&handle, &subscription).unwrap();
    });
}
