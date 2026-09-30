//! Transport and deploy behavior against a real sshd in Docker.
//! Run with `cargo test --features integration`.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use shipslip::transport::{SshTransport, Transport, TransportError};
use shipslip::{execute, prepare, Confirmation, DeployEvent, DeployOutcome, DeployTarget};
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
        use std::io::Write;
        let mut child = Command::new("docker")
            .args([
                "exec",
                "-i",
                "-u",
                "deploy",
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
    }
}

async fn deploy(transport: Arc<SshTransport>, target: DeployTarget) -> Vec<DeployEvent> {
    let preview = prepare(target, &*transport).await.unwrap();
    let confirmation = Confirmation::from(&preview, None).unwrap();
    let (mut rx, _handle) = execute(preview, confirmation, transport).unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
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
            Some(_) => DeployOutcome::FailedAtStep(1),
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
