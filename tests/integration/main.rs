//! Transport and deploy behavior against a real sshd in Docker.
//! Run with `cargo test --features integration`.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use shipslip::receipt::{ReceiptJournal, ReceiptStatus};
use shipslip::transport::{SshTransport, Transport, TransportError};
use shipslip::{
    attach, break_lock, bring_app_up, cancel, execute, execute_recorded, lock_status, prepare,
    prepare_with_plan, AbortReason, BlockReason, BreakLockError, BringUpError, Confirmation,
    DeployEvent, DeployOutcome, DeployTarget, ExecutionHandle, MaintenancePhase, PrepareError,
    RunPlan, SmokeResult, StepStatus, StopReason, WatchStatus, POST_DEPLOY_WATCH,
};
use tokio::sync::mpsc;

const IMAGE: &str = "shipslip-test-sshd";

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker")
        .args(args)
        .output()
        .expect("docker is required for integration tests");
    assert!(
        out.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn temp_dir() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("shipslip-it-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A throwaway sshd container, reachable as `server` through `config`.
struct Server {
    container: String,
    dir: PathBuf,
    port: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.container])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Server {
    async fn start() -> Self {
        static BUILD: Once = Once::new();
        BUILD.call_once(|| {
            docker(&[
                "build",
                "-q",
                "-t",
                IMAGE,
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/integration"),
            ]);
        });

        let container = docker(&["run", "-d", "--rm", "-p", "127.0.0.1::22", IMAGE]);
        let port = docker(&["port", &container, "22"]);
        let port = port.lines().next().unwrap().rsplit(':').next().unwrap();
        let server = Self {
            container,
            dir: temp_dir(),
            port: port.to_string(),
        };

        let key = server.dir.join("id_ed25519");
        let keygen = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .status()
            .unwrap();
        assert!(keygen.success());
        let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
        server.exec(
            "mkdir -p -m 700 ~/.ssh && cat > ~/.ssh/authorized_keys",
            &public,
        );

        let host_key = docker(&[
            "exec",
            &server.container,
            "cat",
            "/etc/ssh/ssh_host_ed25519_key.pub",
        ]);
        server.write_config("trusted", &host_key);
        server.write_config("untrusted", "");

        server.wait_until_ready().await;
        server
    }

    /// Writes an ssh config named `name` that trusts `host_key` (if any).
    fn write_config(&self, name: &str, host_key: &str) {
        let known_hosts = self.dir.join(format!("{name}.known_hosts"));
        let entry = if host_key.is_empty() {
            String::new()
        } else {
            format!("[127.0.0.1]:{} {host_key}\n", self.port)
        };
        std::fs::write(&known_hosts, entry).unwrap();
        // ForwardAgent is on to prove the transport never forwards anyway.
        let config = format!(
            "Host server\n  HostName 127.0.0.1\n  Port {}\n  User deploy\n  \
             IdentityFile {}\n  IdentitiesOnly yes\n  UserKnownHostsFile {}\n  \
             GlobalKnownHostsFile /dev/null\n  ForwardAgent yes\n",
            self.port,
            self.dir.join("id_ed25519").display(),
            known_hosts.display(),
        );
        std::fs::write(self.dir.join(name), config).unwrap();
    }

    fn config(&self) -> PathBuf {
        self.dir.join("trusted")
    }

    async fn wait_until_ready(&self) {
        for _ in 0..50 {
            if SshTransport::connect_with_config("server", &self.config())
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("sshd in {} never accepted a connection", self.container);
    }

    async fn connect(&self) -> SshTransport {
        SshTransport::connect_with_config("server", &self.config())
            .await
            .unwrap()
    }

    /// Runs `script` as `deploy` inside the container, bypassing ssh.
    fn exec(&self, script: &str, stdin: &str) -> String {
        self.exec_as("deploy", script, stdin)
    }

    fn exec_as(&self, user: &str, script: &str, stdin: &str) -> String {
        use std::io::Write;
        let mut child = Command::new("docker")
            .args([
                "exec",
                "-i",
                "-u",
                user,
                &self.container,
                "bash",
                "-c",
                script,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "docker exec failed: {script}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn lock_exists(&self, path: &str) -> bool {
        self.exec(
            &format!("test -e {path}/.git/shipslip.lock && echo yes || echo no"),
            "",
        ) == "yes"
    }

    /// Adds a commit to origin, so a deployed app is behind again.
    fn push_commit(&self, name: &str) {
        self.exec(
            &format!(
                "git clone -q ~/origin.git /tmp/{name} && cd /tmp/{name} \
                 && echo {name} > file && git commit -qam {name} && git push -q origin main"
            ),
            "",
        );
    }

    /// A clone of the fixture repo, one commit behind origin.
    fn app(&self, name: &str) -> String {
        let path = format!("/home/deploy/{name}");
        self.exec(
            &format!("git clone -q ~/origin.git {path} && git -C {path} reset -q --hard HEAD~1"),
            "",
        );
        path
    }
}

async fn run(transport: &SshTransport, script: &str) -> (Result<i32, TransportError>, Vec<String>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let result = transport.run(script, tx).await;
    let mut lines = Vec::new();
    while let Some(line) = rx.recv().await {
        lines.push(line);
    }
    (result, lines)
}

fn target(path: &str, steps: &[&str]) -> DeployTarget {
    DeployTarget {
        env: "staging".into(),
        production: false,
        ssh_alias: "server".into(),
        path: path.into(),
        branch: "main".into(),
        steps: steps.iter().map(|s| s.to_string()).collect(),
        maintenance: false,
        watch_log: false,
        log: None,
        log_daily: false,
        smoke_url: None,
    }
}

fn maintenance_target(path: &str, steps: &[&str]) -> DeployTarget {
    DeployTarget {
        maintenance: true,
        ..target(path, steps)
    }
}

async fn deploy(transport: Arc<SshTransport>, target: DeployTarget) -> Vec<DeployEvent> {
    deploy_with_plan(transport, target, RunPlan::Deploy).await
}

async fn deploy_with_plan(
    transport: Arc<SshTransport>,
    target: DeployTarget,
    run_plan: RunPlan,
) -> Vec<DeployEvent> {
    let preview = prepare_with_plan(target, run_plan, &*transport)
        .await
        .unwrap();
    let confirmation = Confirmation::from(&preview, None).unwrap();
    let (mut rx, _handle) = execute(preview, confirmation, transport).unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

/// Starts a deploy and returns its events once `until` matches one.
async fn deploy_until(
    transport: Arc<SshTransport>,
    target: DeployTarget,
    until: impl Fn(&DeployEvent) -> bool,
) -> (Vec<DeployEvent>, Running) {
    let preview = prepare(target, &*transport).await.unwrap();
    let run_id = preview.run_id().to_string();
    let confirmation = Confirmation::from(&preview, None).unwrap();
    let (mut rx, handle) = execute(preview, confirmation, transport).unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        let done = until(&event);
        events.push(event);
        if done {
            break;
        }
    }
    (events, Running { rx, handle, run_id })
}

struct Running {
    rx: mpsc::UnboundedReceiver<DeployEvent>,
    handle: ExecutionHandle,
    run_id: String,
}

impl Running {
    async fn rest(mut self, mut events: Vec<DeployEvent>) -> Vec<DeployEvent> {
        let rx = &mut self.rx;
        tokio::time::timeout(Duration::from_secs(60), async {
            while let Some(event) = rx.recv().await {
                events.push(event);
            }
        })
        .await
        .expect("deploy did not finish");
        events
    }
}

fn is_output(line: &'static str) -> impl Fn(&DeployEvent) -> bool {
    move |e| matches!(e, DeployEvent::Output { line: l, .. } if l == line)
}

fn step_output(events: &[DeployEvent], step: usize) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            DeployEvent::Output { index, line } if *index == step => Some(line.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn exit_codes_are_reported_as_is() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    for code in [0, 1, 97, 127, 255] {
        let (result, _) = run(&ssh, &format!("exit {code}")).await;
        assert_eq!(result, Ok(code), "exit {code}");
    }
}

#[tokio::test]
async fn output_is_split_into_lines() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    let (result, lines) = run(&ssh, r"printf 'one\ntwo\r\n\xff\nlast'").await;
    assert_eq!(result, Ok(0));
    assert_eq!(lines, ["one", "two", "\u{fffd}", "last"]);
}

#[tokio::test]
async fn script_killed_by_signal_is_connection_lost() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    let (result, _) = run(&ssh, "kill -9 $$").await;
    assert!(
        matches!(result, Err(TransportError::ConnectionLost(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn agent_is_not_forwarded() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    let (_, lines) = run(&ssh, r#"echo "${SSH_AUTH_SOCK:-none}""#).await;
    assert_eq!(lines, ["none"]);
}

#[tokio::test]
async fn server_lost_mid_command_is_connection_lost() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let running = tokio::spawn({
        let ssh = ssh.clone();
        async move { run(&ssh, "echo started; sleep 60").await }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    docker(&["kill", &server.container]);

    let (result, lines) = tokio::time::timeout(Duration::from_secs(30), running)
        .await
        .expect("lost connection was not noticed")
        .unwrap();
    assert_eq!(lines, ["started"]);
    assert!(
        matches!(result, Err(TransportError::ConnectionLost(_))),
        "{result:?}"
    );

    let (after, _) = run(&ssh, "true").await;
    assert!(
        matches!(after, Err(TransportError::Connect(_))),
        "{after:?}"
    );
}

#[tokio::test]
async fn unreachable_server_is_a_connect_error() {
    let dir = temp_dir();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = dir.join("config");
    std::fs::write(
        &config,
        format!("Host server\n  HostName 127.0.0.1\n  Port {port}\n  ConnectTimeout 5\n"),
    )
    .unwrap();
    let result = SshTransport::connect_with_config("server", &config).await;
    assert!(
        matches!(result, Err(TransportError::Connect(_))),
        "{result:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn unknown_host_key_is_refused() {
    let server = Server::start().await;
    let result = SshTransport::connect_with_config("server", &server.dir.join("untrusted")).await;
    assert!(
        matches!(result, Err(TransportError::Connect(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn option_like_alias_is_rejected() {
    let result = SshTransport::connect("-oProxyCommand=true").await;
    assert!(
        matches!(result, Err(TransportError::Connect(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn deploy_runs_steps_verbatim_and_fast_forwards() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    let step = r#"echo "it's" '$HOME' "$HOME" `echo tick`; echo out; echo err >&2; echo end"#;

    let events = deploy(ssh.clone(), target(&path, &[step])).await;
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::Succeeded)),
        "{events:#?}"
    );
    assert_eq!(
        step_output(&events, 1),
        ["it's $HOME /home/deploy tick", "out", "err", "end"]
    );
    let head = server.exec(&format!("git -C {path} log -1 --format=%s"), "");
    assert_eq!(head, "Second");
    assert_eq!(
        server.exec(&format!("git -C {path} status --porcelain"), ""),
        ""
    );
    assert!(!server.lock_exists(&path));
}

#[tokio::test]
async fn steps_run_in_strict_mode() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let cases: [(&str, Option<i32>); 5] = [
        ("false; echo done", Some(1)),
        ("false | cat", Some(1)),
        ("false || true", None),
        ("exit 97", Some(97)),
        ("no-such-command", Some(127)),
    ];
    for (n, (step, failure)) in cases.into_iter().enumerate() {
        let path = server.app(&format!("strict{n}"));
        let events = deploy(ssh.clone(), target(&path, &[step])).await;
        let expected = match failure {
            None => DeployOutcome::Succeeded,
            Some(_) => DeployOutcome::FailedAtStep {
                step: 1,
                partial_update: false,
            },
        };
        assert_eq!(
            events.last(),
            Some(&DeployEvent::Finished(expected)),
            "{step}"
        );
        if let Some(code) = failure {
            assert!(
                events.iter().any(|e| matches!(
                    e,
                    DeployEvent::StepFinished { index: 1, exit_code: Some(c), .. } if *c == code
                )),
                "{step}: {events:#?}"
            );
            assert!(!step_output(&events, 1).contains(&"done"), "{step}");
        }
    }
}

#[tokio::test]
async fn missing_app_path_blocks_prepare() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    let result = prepare(target("/home/deploy/missing", &[]), &ssh).await;
    assert!(result.is_err(), "{result:?}");
}

#[tokio::test]
async fn step_survives_a_dropped_connection() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    let step = "echo before; sleep 2; echo after; exit 3";

    let (events, running) = deploy_until(ssh, target(&path, &[step]), is_output("before")).await;
    server.exec_as("root", "pkill -f 'sshd: deploy'", "");
    let events = running.rest(events).await;

    assert_eq!(step_output(&events, 1), ["before", "after"], "{events:#?}");
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::FailedAtStep {
            step: 1,
            partial_update: false
        }))
    );
    assert!(events.contains(&DeployEvent::StepFinished {
        index: 1,
        status: StepStatus::Failed,
        exit_code: Some(3),
    }));
}

#[tokio::test]
async fn detached_step_keeps_running_on_the_server() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    let step = "echo started; sleep 1; echo ok > ~/marker";

    let (events, running) = deploy_until(ssh, target(&path, &[step]), is_output("started")).await;
    running.handle.detach();
    let run_id = running.run_id.clone();
    let events = running.rest(events).await;
    assert_eq!(events.last(), Some(&DeployEvent::Detached { index: 1 }));

    let exit_file = format!("~/.shipslip/runs/{run_id}/step-1/exit");
    let result = server.exec(
        &format!("for _ in $(seq 50); do [ -e {exit_file} ] && break; sleep 0.1; done; cat ~/marker {exit_file}"),
        "",
    );
    assert_eq!(result, "ok\n0");
}

#[tokio::test]
async fn attach_recovers_a_detached_run_without_relaunching_the_step() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    let preview = prepare(
        target(
            &path,
            &[
                "echo first >> ~/receipt-counter; echo started; sleep 2; echo done",
                "echo second >> ~/receipt-counter",
            ],
        ),
        &*ssh,
    )
    .await
    .unwrap();
    let journal = Arc::new(
        ReceiptJournal::create(&server.dir.join("receipts"), "app", &server.dir, &preview).unwrap(),
    );
    let receipt_path = journal.path().to_path_buf();
    let confirmation = Confirmation::from(&preview, None).unwrap();
    let (mut events, handle) =
        execute_recorded(preview, confirmation, ssh.clone(), journal.clone()).unwrap();
    while let Some(event) = events.recv().await {
        if matches!(event, DeployEvent::Output { index: 1, line } if line == "started") {
            break;
        }
    }
    handle.detach();
    assert_eq!(
        events.recv().await,
        Some(DeployEvent::Detached { index: 1 })
    );
    drop(events);
    drop(handle);
    drop(journal);
    drop(ssh);

    let resumed = Arc::new(ReceiptJournal::load(&receipt_path).unwrap());
    let ssh = Arc::new(server.connect().await);
    let (mut events, _handle) = attach(resumed.clone(), ssh).unwrap();
    let mut final_event = None;
    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = events.recv().await {
            final_event = Some(event);
        }
    })
    .await
    .unwrap();
    assert_eq!(
        final_event,
        Some(DeployEvent::Finished(DeployOutcome::Succeeded))
    );
    assert_eq!(resumed.snapshot().status, ReceiptStatus::Final);
    assert_eq!(server.exec("cat ~/receipt-counter", ""), "first\nsecond");
}

#[tokio::test]
async fn killed_step_process_is_unknown() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");

    let (events, running) = deploy_until(
        ssh,
        target(&path, &["echo started; sleep 30"]),
        is_output("started"),
    )
    .await;
    server.exec(
        &format!(
            "kill -9 -- -$(cat ~/.shipslip/runs/{}/step-1/pid)",
            running.run_id
        ),
        "",
    );
    let events = running.rest(events).await;
    assert!(
        matches!(
            events.last(),
            Some(DeployEvent::Finished(DeployOutcome::Unknown {
                step: 1,
                ..
            }))
        ),
        "{events:#?}"
    );
    assert!(server.lock_exists(&path));
}

#[tokio::test]
async fn unreachable_server_mid_step_is_interrupted() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");

    let (events, running) = deploy_until(
        ssh,
        target(&path, &["echo started; sleep 30"]),
        is_output("started"),
    )
    .await;
    docker(&["kill", &server.container]);
    let events = running.rest(events).await;
    assert!(
        matches!(
            events.last(),
            Some(DeployEvent::Interrupted { index: 1, .. })
        ),
        "{events:#?}"
    );
    assert!(!events.iter().any(|e| matches!(e, DeployEvent::Finished(_))));
}

#[tokio::test]
async fn run_files_are_private() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    let (events, running) = deploy_until(ssh, target(&path, &["true"]), |_| false).await;
    let run_id = running.run_id.clone();
    let events = running.rest(events).await;
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
    );

    let modes = server.exec(
        &format!(
            "cd ~/.shipslip && stat -c '%a %F %n' . runs runs/{run_id} runs/{run_id}/step-1 runs/{run_id}/step-1/*"
        ),
        "",
    );
    // 4 directories, then exit, ident, log, pid and script.sh.
    assert_eq!(modes.lines().count(), 9, "{modes}");
    for line in modes.lines() {
        let expected = if line.contains(" directory ") {
            "700"
        } else {
            "600"
        };
        assert!(line.starts_with(expected), "{line}");
    }
}

#[tokio::test]
async fn lock_blocks_a_second_deploy_until_cancelled() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    let path = server.app("app");

    let first = prepare(target(&path, &[]), &ssh).await.unwrap();
    let err = prepare(target(&path, &[]), &ssh).await.unwrap_err();
    let PrepareError::LockHeld(info) = err else {
        panic!("{err:?}");
    };
    assert!(info.age_secs < 10 && !info.is_stale(), "{info:?}");
    let owner = info.owner.unwrap();
    assert_eq!(owner.run_id, first.run_id());
    assert_eq!(owner.target_sha, first.target_sha());

    cancel(first, &ssh).await.unwrap();
    assert!(!server.lock_exists(&path));
    let second = prepare(target(&path, &[]), &ssh).await.unwrap();
    cancel(second, &ssh).await.unwrap();
}

#[tokio::test]
async fn only_a_stale_lock_can_be_broken() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    let path = server.app("app");
    let t = target(&path, &[]);
    let held = prepare(t.clone(), &ssh).await.unwrap();

    assert!(matches!(
        break_lock(&t, &ssh, "staging").await,
        Err(BreakLockError::Live(_))
    ));
    server.exec(&format!("echo 0 > {path}/.git/shipslip.lock/heartbeat"), "");
    assert_eq!(
        break_lock(&t, &ssh, "production").await,
        Err(BreakLockError::EnvNameRequired("staging".into()))
    );
    assert_eq!(break_lock(&t, &ssh, "staging").await, Ok(()));
    assert!(!server.lock_exists(&path));
    assert_eq!(
        server.exec(
            &format!("ls -d {path}/.git/shipslip.lock.broken-* | wc -l"),
            ""
        ),
        "1"
    );

    // The displaced holder cancelling must not touch anything.
    cancel(held, &ssh).await.unwrap();
    assert_eq!(
        server.exec(
            &format!("ls -d {path}/.git/shipslip.lock.broken-* | wc -l"),
            ""
        ),
        "1"
    );
}

#[tokio::test]
async fn lock_broken_mid_deploy_stops_after_the_step_and_spares_the_successor() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let other = server.connect().await;
    let path = server.app("app");
    let t = target(&path, &["echo one; sleep 4", "echo two"]);

    let (events, running) = deploy_until(ssh, t.clone(), is_output("one")).await;
    server.exec(&format!("echo 0 > {path}/.git/shipslip.lock/heartbeat"), "");
    break_lock(&t, &other, "staging").await.unwrap();
    server.push_commit("third");
    let successor = prepare(target(&path, &[]), &other).await.unwrap();

    let events = running.rest(events).await;
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::StoppedAfterStep {
            step: 1,
            reason: StopReason::LockLost
        })),
        "{events:#?}"
    );
    assert!(!step_output(&events, 2).contains(&"two"));
    assert_eq!(
        server.exec(&format!("cat {path}/.git/shipslip.lock/run_id"), ""),
        successor.run_id()
    );
    cancel(successor, &other).await.unwrap();
}

#[tokio::test]
async fn unsafe_checkouts_are_blocked_before_the_lock() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    type Case = (
        &'static str,
        &'static str,
        &'static [&'static str],
        fn(&BlockReason) -> bool,
    );
    let cases: [Case; 6] = [
        ("dirty", "echo x >> file", &[], |r| {
            *r == BlockReason::DirtyTree(vec![" M file".into()])
        }),
        ("branch", "git checkout -q -b other", &[], |r| {
            *r == BlockReason::WrongBranch {
                expected: "main".into(),
                actual: "other".into(),
            }
        }),
        (
            "merging",
            "touch \"$(git rev-parse --absolute-git-dir)/MERGE_HEAD\"",
            &[],
            |r| *r == BlockReason::OperationInProgress("MERGE_HEAD".into()),
        ),
        (
            "diverged",
            "git commit -q --allow-empty -m local",
            &[],
            |r| *r == BlockReason::NotFastForward,
        ),
        (
            "nofetch",
            "git remote set-url origin /nonexistent",
            &[],
            |r| matches!(r, BlockReason::FetchFailed(m) if m.contains("/nonexistent")),
        ),
        (
            "syntax",
            "true",
            &["echo ok", "echo 'unclosed"],
            |r| matches!(r, BlockReason::InvalidStep { step: 2, message } if message.contains("unexpected EOF")),
        ),
    ];
    for (name, setup, steps, expected) in cases {
        let path = server.app(name);
        server.exec(&format!("cd {path} && {setup}"), "");
        let err = prepare(target(&path, steps), &ssh).await.unwrap_err();
        assert!(
            matches!(&err, PrepareError::Blocked(r) if expected(r)),
            "{name}: {err:?}"
        );
        assert!(!server.lock_exists(&path), "{name}");
    }
}

#[tokio::test]
async fn head_moved_after_the_preview_aborts_before_changes() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    let preview = prepare(target(&path, &["true"]), &*ssh).await.unwrap();
    let run_id = preview.run_id().to_string();
    server.exec(
        &format!("git -C {path} commit -q --allow-empty -m sneaky"),
        "",
    );

    let confirmation = Confirmation::from(&preview, None).unwrap();
    let (mut rx, _handle) = execute(preview, confirmation, ssh).unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    assert!(
        matches!(
            &events[..],
            [DeployEvent::Finished(DeployOutcome::AbortedBeforeChanges(
                AbortReason::HeadMoved { .. }
            ))]
        ),
        "{events:#?}"
    );
    assert_eq!(
        server.exec(
            &format!("test -e ~/.shipslip/runs/{run_id} && echo ran || echo none"),
            ""
        ),
        "none"
    );
    assert!(!server.lock_exists(&path));
}

#[tokio::test]
async fn fast_forward_that_fails_partway_is_a_partial_update() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    server.exec(
        "git clone -q ~/origin.git /tmp/seed && cd /tmp/seed \
         && mkdir locked && echo 1 > locked/f && git add locked \
         && git commit -qm 'add locked' && git push -q origin main \
         && echo 2 > file && echo 2 > locked/f && git commit -qam 'change both' \
         && git push -q origin main",
        "",
    );
    let path = server.app("app");
    server.exec_as(
        "root",
        &format!("chown root:root {path}/locked && chmod 555 {path}/locked"),
        "",
    );
    let from = server.exec(&format!("git -C {path} rev-parse HEAD"), "");

    let events = deploy(ssh, target(&path, &["echo never"])).await;
    let n = events.len();
    assert_eq!(
        &events[n - 2..],
        [
            DeployEvent::ServerState {
                head: Some(from),
                tree_dirty: Some(true),
            },
            DeployEvent::Finished(DeployOutcome::FailedAtStep {
                step: 0,
                partial_update: true,
            }),
        ],
        "{events:#?}"
    );
    assert!(!step_output(&events, 1).contains(&"never"));
}

#[tokio::test]
async fn maintenance_mode_runs_around_a_successful_deploy() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");

    let events = deploy(ssh, maintenance_target(&path, &["echo deployed"])).await;
    let down = events
        .iter()
        .position(|event| {
            *event
                == DeployEvent::MaintenanceStarted {
                    phase: MaintenancePhase::Down,
                }
        })
        .unwrap();
    let fast_forward = events
        .iter()
        .position(|event| matches!(event, DeployEvent::StepStarted { index: 0, .. }))
        .unwrap();
    let up = events
        .iter()
        .position(|event| {
            *event
                == DeployEvent::MaintenanceStarted {
                    phase: MaintenancePhase::Up,
                }
        })
        .unwrap();

    assert!(down < fast_forward && fast_forward < up, "{events:#?}");
    assert!(events.contains(&DeployEvent::MaintenanceOutput {
        phase: MaintenancePhase::Down,
        line: "down".into(),
    }));
    assert!(events.contains(&DeployEvent::MaintenanceOutput {
        phase: MaintenancePhase::Up,
        line: "up".into(),
    }));
    assert!(!events.contains(&DeployEvent::AppLeftDown));
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
    );
    assert_eq!(
        server.exec(
            &format!("test -e {path}/storage/framework/down && echo down || echo up"),
            ""
        ),
        "up"
    );
}

#[tokio::test]
async fn failed_deploy_leaves_the_app_in_maintenance_mode() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");

    let events = deploy(ssh, maintenance_target(&path, &["exit 8"])).await;
    assert!(events.contains(&DeployEvent::AppLeftDown), "{events:#?}");
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::FailedAtStep {
            step: 1,
            partial_update: false,
        }))
    );
    assert_eq!(
        server.exec(
            &format!("test -e {path}/storage/framework/down && echo down || echo up"),
            ""
        ),
        "down"
    );
    assert!(!events.iter().any(|event| {
        *event
            == DeployEvent::MaintenanceStarted {
                phase: MaintenancePhase::Up,
            }
    }));
}

#[tokio::test]
async fn bring_app_up_restores_a_deploy_left_in_maintenance_mode() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    let path = server.app("app");
    server.exec(&format!("cd {path} && php artisan down"), "");

    let output = bring_app_up(&target(&path, &[]), &ssh).await.unwrap();
    assert_eq!(output, ["up"]);
    assert_eq!(
        server.exec(
            &format!("test -e {path}/storage/framework/down && echo down || echo up"),
            ""
        ),
        "up"
    );
    assert!(!server.lock_exists(&path));
}

#[tokio::test]
async fn bring_app_up_does_not_run_while_a_deploy_holds_the_lock() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    let path = server.app("app");
    let preview = prepare(target(&path, &[]), &ssh).await.unwrap();

    let error = bring_app_up(&target(&path, &[]), &ssh).await.unwrap_err();
    assert!(matches!(error, BringUpError::LockHeld(_)));
    assert_eq!(
        server.exec(
            "test -e ~/.shipslip-php-calls && cat ~/.shipslip-php-calls || true",
            ""
        ),
        ""
    );
    cancel(preview, &ssh).await.unwrap();
}

#[tokio::test]
async fn maintenance_down_failure_aborts_before_fast_forward() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    let from = server.exec(&format!("git -C {path} rev-parse HEAD"), "");
    server.exec("touch ~/.shipslip-fail-down", "");

    let events = deploy(ssh, maintenance_target(&path, &["echo never"])).await;
    assert!(
        events.contains(&DeployEvent::Finished(DeployOutcome::AbortedBeforeChanges(
            AbortReason::MaintenanceDownFailed(9)
        )))
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event, DeployEvent::StepStarted { index: 0, .. })));
    assert_eq!(
        server.exec(&format!("git -C {path} rev-parse HEAD"), ""),
        from
    );
    assert!(!server.lock_exists(&path));
}

#[tokio::test]
async fn maintenance_up_failure_keeps_deploy_success_and_reports_app_down() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    server.exec("touch ~/.shipslip-fail-up", "");

    let events = deploy(ssh, maintenance_target(&path, &["echo deployed"])).await;
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
    assert_eq!(
        server.exec(
            &format!("test -e {path}/storage/framework/down && echo down || echo up"),
            ""
        ),
        "down"
    );
}

#[tokio::test]
async fn maintenance_disabled_never_invokes_php() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");

    let events = deploy(ssh, target(&path, &["echo deployed"])).await;
    assert!(!events.iter().any(|event| matches!(
        event,
        DeployEvent::MaintenanceStarted { .. }
            | DeployEvent::MaintenanceOutput { .. }
            | DeployEvent::MaintenanceFinished { .. }
            | DeployEvent::AppLeftDown
    )));
    assert_eq!(
        server.exec(
            "test -e ~/.shipslip-php-calls && cat ~/.shipslip-php-calls || true",
            ""
        ),
        ""
    );
}

#[tokio::test]
async fn rerun_executes_recipe_again_without_a_git_fast_forward() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    server.exec(&format!("git -C {path} merge --ff-only origin/main"), "");

    let events = deploy_with_plan(
        ssh,
        target(&path, &["echo rerun > rerun-marker"]),
        RunPlan::Rerun,
    )
    .await;
    assert!(events.contains(&DeployEvent::StepStarted {
        index: 1,
        name: "echo rerun > rerun-marker".into(),
    }));
    assert!(!events
        .iter()
        .any(|event| matches!(event, DeployEvent::StepStarted { index: 0, .. })));
    assert_eq!(
        server.exec(&format!("cat {path}/rerun-marker"), ""),
        "rerun"
    );
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
    );
}

#[tokio::test]
async fn from_step_starts_at_the_selected_recipe_step() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    server.exec(&format!("git -C {path} merge --ff-only origin/main"), "");

    let events = deploy_with_plan(
        ssh,
        target(
            &path,
            &[
                "echo first > selected-marker",
                "echo second > selected-marker",
            ],
        ),
        RunPlan::FromStep(2),
    )
    .await;
    assert!(!events
        .iter()
        .any(|event| matches!(event, DeployEvent::StepStarted { index: 0 | 1, .. })));
    assert!(events.contains(&DeployEvent::StepStarted {
        index: 2,
        name: "echo second > selected-marker".into(),
    }));
    assert_eq!(
        server.exec(&format!("cat {path}/selected-marker"), ""),
        "second"
    );
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
    );
}

#[tokio::test]
async fn log_watch_reports_only_errors_written_during_the_deploy() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    server.exec(
        &format!(
            "mkdir -p {path}/storage/logs && printf '%s\\n' \
             '[2026-09-30 10:00:00] production.ERROR: OldException: already broken' \
             '[2026-09-30 11:00:00] production.ERROR: OtherException: also old' \
             > {path}/storage/logs/laravel.log"
        ),
        "",
    );
    let step = format!(
        "printf '%s\\n' '[2026-10-01 10:00:00] production.ERROR: NewException: broke at \
         {path}/app/Http/Kernel.php:12' >> storage/logs/laravel.log"
    );
    let t = DeployTarget {
        watch_log: true,
        ..target(&path, &[&step])
    };

    let (events, running) =
        deploy_until(ssh, t, |e| matches!(e, DeployEvent::NewLogError { .. })).await;
    // The watch would otherwise continue for its full post-deploy window.
    running.handle.cancel_watch();
    let events = running.rest(events).await;

    let new_errors: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            DeployEvent::NewLogError {
                message, file_line, ..
            } => Some((message.as_str(), file_line.as_deref())),
            _ => None,
        })
        .collect();
    let kernel = format!("{path}/app/Http/Kernel.php:12");
    assert_eq!(
        new_errors,
        [(
            &*format!("NewException: broke at {kernel}"),
            Some(kernel.as_str())
        )],
        "{events:#?}"
    );
    let watch = events
        .iter()
        .find_map(|e| match e {
            DeployEvent::WatchFinished(result) => Some(result),
            _ => None,
        })
        .unwrap();
    assert_eq!(watch.status, WatchStatus::Cancelled);
    assert_eq!(watch.baseline_signatures, 2);
    assert_eq!(watch.new_errors.len(), 1);
    assert_eq!(watch.new_errors[0].exception, "NewException");
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
    );
}

#[tokio::test]
async fn stopping_the_post_deploy_watch_finishes_the_run_and_releases_the_lock() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let path = server.app("app");
    let t = DeployTarget {
        watch_log: true,
        ..target(&path, &["true"])
    };

    let (events, running) = deploy_until(ssh.clone(), t.clone(), |e| {
        matches!(e, DeployEvent::WatchStarted { .. })
    })
    .await;
    assert_eq!(
        events.last(),
        Some(&DeployEvent::WatchStarted {
            window: POST_DEPLOY_WATCH
        })
    );
    running.handle.cancel_watch();
    let events = tokio::time::timeout(Duration::from_secs(30), running.rest(events))
        .await
        .expect("stopping the watch must not wait for its full window");

    assert!(
        events.iter().any(
            |e| matches!(e, DeployEvent::WatchFinished(w) if w.status == WatchStatus::Cancelled)
        ),
        "{events:#?}"
    );
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
    );
    assert_eq!(lock_status(&t, &*ssh).await, Ok(None));
}

/// Answers one HTTP request with 204 on a local port.
fn http_204() -> String {
    use std::io::{Read, Write};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/health", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request);
        let _ = stream.write_all(
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    });
    url
}

#[tokio::test]
async fn smoke_check_runs_after_a_successful_deploy() {
    let server = Server::start().await;
    let ssh = Arc::new(server.connect().await);
    let smoke = |events: &[DeployEvent]| {
        events
            .iter()
            .find_map(|e| match e {
                DeployEvent::SmokeFinished(result) => Some(result.clone()),
                _ => None,
            })
            .unwrap()
    };

    let passing = DeployTarget {
        smoke_url: Some(http_204()),
        ..target(&server.app("app"), &["true"])
    };
    let events = deploy(ssh.clone(), passing).await;
    assert!(
        matches!(smoke(&events), SmokeResult::Passed { status: 204, .. }),
        "{events:#?}"
    );
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
    );

    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/health", closed.local_addr().unwrap());
    drop(closed);
    let failing = DeployTarget {
        smoke_url: Some(url),
        ..target(&server.app("other"), &["true"])
    };
    let events = deploy(ssh, failing).await;
    // No HTTP response: no status, and the deploy itself still succeeded.
    assert!(
        matches!(smoke(&events), SmokeResult::Failed { status: None, .. }),
        "{events:#?}"
    );
    assert_eq!(
        events.last(),
        Some(&DeployEvent::Finished(DeployOutcome::Succeeded))
    );
}

#[tokio::test]
async fn stale_lock_recovery_shows_the_holder_then_breaks_and_brings_the_app_up() {
    let server = Server::start().await;
    let ssh = server.connect().await;
    let path = server.app("app");
    let t = target(&path, &[]);
    assert_eq!(lock_status(&t, &ssh).await, Ok(None));

    // An abandoned run: it holds the lock, left the app down and stopped heartbeating.
    let held = prepare(t.clone(), &ssh).await.unwrap();
    server.exec(&format!("cd {path} && php artisan down"), "");
    server.exec(&format!("echo 0 > {path}/.git/shipslip.lock/heartbeat"), "");

    let info = lock_status(&t, &ssh).await.unwrap().unwrap();
    assert!(info.is_stale());
    assert_eq!(info.owner.unwrap().run_id, held.run_id());
    assert!(matches!(
        bring_app_up(&t, &ssh).await,
        Err(BringUpError::LockHeld(_))
    ));

    assert_eq!(break_lock(&t, &ssh, "staging").await, Ok(()));
    assert_eq!(lock_status(&t, &ssh).await, Ok(None));
    assert_eq!(bring_app_up(&t, &ssh).await.unwrap(), ["up"]);
    assert_eq!(
        server.exec(
            &format!("test -e {path}/storage/framework/down && echo down || echo up"),
            ""
        ),
        "up"
    );
    assert!(!server.lock_exists(&path));
}
