use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::Sender;

use super::*;

/// A temp folder holding a Laravel-like git checkout and a `bin` with a
/// fake `php`, removed on drop.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "shipslip-connection-check-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        let fixture = Self {
            root: fs::canonicalize(root).unwrap(),
        };
        fixture.sh("mkdir app && cd app && git init -q -b main \
             && echo '<?php' > artisan && mkdir -p storage/logs \
             && echo '[2026-11-13 10:00:00] production.ERROR: x' > storage/logs/laravel.log \
             && git add artisan && git -c user.email=t@t -c user.name=t commit -qm init");
        fixture.php("echo 'Laravel Framework 11.9.2'");
        fixture
    }

    fn app(&self) -> String {
        self.root.join("app").to_string_lossy().into_owned()
    }

    fn sh(&self, script: &str) {
        let status = Command::new("bash")
            .arg("-c")
            .arg(script)
            .current_dir(&self.root)
            .status()
            .unwrap();
        assert!(status.success(), "{script}");
    }

    /// The fake `php`: refuses anything but `artisan --version --no-ansi`.
    fn php(&self, body: &str) {
        let path = self.root.join("bin/php");
        fs::write(
            &path,
            format!(
                "#!/bin/bash\n[ \"$*\" = 'artisan --version --no-ansi' ] || {{ echo \"php $*\" >&2; exit 99; }}\n{body}\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn request(&self) -> Request {
        Request {
            alias: "web".into(),
            path: self.app(),
            branch: "main".into(),
            log_path: "storage/logs/laravel.log".into(),
            log_daily: false,
        }
    }

    fn ssh(&self) -> FakeSsh {
        FakeSsh {
            resolve: Ok(resolved()),
            connect: Ok(()),
            path: format!(
                "{}:{}",
                self.root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
            lose_on: None,
            prefix: String::new(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new("chmod")
            .args(["-R", "u+rwx"])
            .arg(&self.root)
            .status();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn resolved() -> Resolved {
    Resolved {
        host_name: "203.0.113.7".into(),
        user: "forge".into(),
        port: 22,
    }
}

/// Resolves and connects as told; the connection runs each script in a
/// local `bash` with `path` as `PATH`, or drops on the `lose_on`th script.
struct FakeSsh {
    resolve: Result<Resolved, String>,
    connect: Result<(), TransportError>,
    path: String,
    lose_on: Option<usize>,
    /// Shell run before each probe script, standing in for a login profile.
    prefix: String,
}

impl Ssh for FakeSsh {
    type Connection = LocalBash;

    fn resolve(&self, alias: &str) -> Result<Resolved, ResolveError> {
        assert_eq!(alias, "web");
        self.resolve.clone().map_err(ResolveError::Failed)
    }

    async fn connect(&self, alias: &str) -> Result<LocalBash, TransportError> {
        assert_eq!(alias, "web");
        self.connect.clone()?;
        Ok(LocalBash {
            path: self.path.clone(),
            lose_on: self.lose_on,
            prefix: self.prefix.clone(),
            scripts: Mutex::default(),
        })
    }
}

struct LocalBash {
    path: String,
    lose_on: Option<usize>,
    prefix: String,
    scripts: Mutex<Vec<String>>,
}

impl Transport for LocalBash {
    async fn run(
        &self,
        script: &str,
        output: Sender<String>,
        _line_limit: usize,
    ) -> Result<i32, TransportError> {
        let count = {
            let mut scripts = self.scripts.lock().unwrap();
            scripts.push(script.to_string());
            scripts.len()
        };
        if self.lose_on == Some(count) {
            return Err(TransportError::ConnectionLost("broken pipe".into()));
        }
        let mut child = tokio::process::Command::new("/bin/bash")
            .arg("-s")
            .env("PATH", &self.path)
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(format!("{}\n{script}", self.prefix).as_bytes())
            .await
            .unwrap();
        drop(stdin);
        let out = child.wait_with_output().await.unwrap();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            output.send(line.to_string()).await.unwrap();
        }
        Ok(out.status.code().unwrap())
    }

    async fn reconnect(&self) -> Result<(), TransportError> {
        panic!("the connection check never reconnects")
    }
}

async fn check(ssh: &FakeSsh, request: &Request) -> Vec<(Probe, Outcome)> {
    let mut events = Vec::new();
    let results = run(ssh, request, |event| events.push(event)).await;
    let mut expected = Vec::new();
    for (probe, outcome) in &results {
        if *outcome != Outcome::NotRun {
            expected.push(CheckEvent::Started(*probe));
        }
        expected.push(CheckEvent::Finished(*probe, outcome.clone()));
    }
    assert_eq!(
        events, expected,
        "each probe starts, then finishes, in order"
    );
    assert_eq!(
        results.iter().map(|(probe, _)| *probe).collect::<Vec<_>>(),
        Probe::ALL
    );
    results
}

fn outcomes(results: &[(Probe, Outcome)]) -> Vec<Outcome> {
    results.iter().map(|(_, outcome)| outcome.clone()).collect()
}

fn not_run(n: usize) -> Vec<Outcome> {
    vec![Outcome::NotRun; n]
}

#[tokio::test]
async fn every_probe_passes_on_a_reachable_checkout() {
    let fx = Fixture::new();
    let results = check(&fx.ssh(), &fx.request()).await;
    assert_eq!(
        outcomes(&results),
        [
            Outcome::Pass(Passed::Alias(resolved())),
            Outcome::Pass(Passed::Connected),
            Outcome::Pass(Passed::Checkout),
            Outcome::Pass(Passed::Branch),
            Outcome::Pass(Passed::Artisan("Laravel Framework 11.9.2".into())),
            Outcome::Pass(Passed::Log("storage/logs/laravel.log".into())),
        ]
    );
}

#[tokio::test]
async fn an_alias_that_does_not_resolve_runs_nothing_else() {
    let fx = Fixture::new();
    let ssh = FakeSsh {
        resolve: Err("Bad configuration option".into()),
        ..fx.ssh()
    };
    let results = check(&ssh, &fx.request()).await;
    let mut expected = vec![Outcome::Fail(Failed::Alias(
        "ssh -G failed: Bad configuration option".into(),
    ))];
    expected.extend(not_run(5));
    assert_eq!(outcomes(&results), expected);
}

#[tokio::test]
async fn a_refused_connection_is_classified_and_the_rest_is_not_run() {
    let fx = Fixture::new();
    for (message, failure) in [
        (
            "No ED25519 host key is known for server and you have requested strict checking.\n\
             Host key verification failed.",
            ConnectFailure::HostKeyUnknown,
        ),
        ("Host key verification failed.", ConnectFailure::HostKeyUnverified),
        (
            "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\nHost key verification failed.",
            ConnectFailure::HostKeyChanged,
        ),
        ("Permission denied (publickey).", ConnectFailure::KeyNotLoaded),
        ("Connection refused", ConnectFailure::Other),
    ] {
        let ssh = FakeSsh {
            connect: Err(TransportError::Connect(message.into())),
            ..fx.ssh()
        };
        let results = check(&ssh, &fx.request()).await;
        let mut expected = vec![
            Outcome::Pass(Passed::Alias(resolved())),
            Outcome::Fail(Failed::Connect {
                failure,
                message: message.into(),
            }),
        ];
        expected.extend(not_run(4));
        assert_eq!(outcomes(&results), expected, "{message}");
    }
}

#[tokio::test]
async fn a_connection_lost_mid_check_fails_that_probe_and_never_reconnects() {
    let fx = Fixture::new();
    let ssh = FakeSsh {
        lose_on: Some(2),
        ..fx.ssh()
    };
    let results = check(&ssh, &fx.request()).await;
    assert_eq!(
        outcomes(&results)[2..],
        [
            Outcome::Pass(Passed::Checkout),
            Outcome::Fail(Failed::Lost("broken pipe".into())),
            Outcome::NotRun,
            Outcome::NotRun,
        ]
    );
}

#[tokio::test]
async fn a_missing_remote_path_runs_no_probe_inside_it() {
    let fx = Fixture::new();
    let request = Request {
        path: format!("{}/gone", fx.app()),
        ..fx.request()
    };
    let results = check(&fx.ssh(), &request).await;
    let mut expected = vec![Outcome::Fail(Failed::NoFolder)];
    expected.extend(not_run(3));
    assert_eq!(outcomes(&results)[2..], expected);
}

#[tokio::test]
async fn a_folder_outside_git_skips_the_branch_but_checks_artisan_and_the_log() {
    let fx = Fixture::new();
    fx.sh("rm -rf app/.git");
    let results = check(&fx.ssh(), &fx.request()).await;
    let outcomes = outcomes(&results);
    assert!(
        matches!(&outcomes[2], Outcome::Fail(Failed::NotCheckout(messages)) if messages.iter().any(|m| m.contains("not a git repository"))),
        "{:?}",
        outcomes[2]
    );
    assert_eq!(
        outcomes[3..],
        [
            Outcome::NotRun,
            Outcome::Pass(Passed::Artisan("Laravel Framework 11.9.2".into())),
            Outcome::Pass(Passed::Log("storage/logs/laravel.log".into())),
        ]
    );
}

#[tokio::test]
async fn another_branch_or_a_detached_head_fails_the_branch_probe() {
    let fx = Fixture::new();
    fx.sh("git -C app checkout -q -b feature");
    let results = check(&fx.ssh(), &fx.request()).await;
    assert_eq!(
        results[3].1,
        Outcome::Fail(Failed::OtherBranch("feature".into()))
    );
    assert_eq!(
        results[4].1,
        Outcome::Pass(Passed::Artisan("Laravel Framework 11.9.2".into()))
    );

    fx.sh("git -C app checkout -q --detach");
    let results = check(&fx.ssh(), &fx.request()).await;
    assert_eq!(results[3].1, Outcome::Fail(Failed::Detached));
}

#[tokio::test]
async fn artisan_fails_without_the_file_without_php_or_on_an_error() {
    let fx = Fixture::new();
    fx.php("echo 'PHP Fatal error: vendor/autoload.php missing'; exit 255");
    let results = check(&fx.ssh(), &fx.request()).await;
    assert_eq!(
        results[4].1,
        Outcome::Fail(Failed::Artisan {
            code: 255,
            output: vec!["PHP Fatal error: vendor/autoload.php missing".into()],
        })
    );

    // A PATH with the tools the probes use, but no php.
    let tools = fx.root.join("tools");
    fs::create_dir(&tools).unwrap();
    for tool in ["git", "head", "sed"] {
        let found = Command::new("bash")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        let found = String::from_utf8(found.stdout).unwrap();
        std::os::unix::fs::symlink(found.trim(), tools.join(tool)).unwrap();
    }
    let ssh = FakeSsh {
        path: tools.to_string_lossy().into_owned(),
        ..fx.ssh()
    };
    let results = check(&ssh, &fx.request()).await;
    assert_eq!(results[4].1, Outcome::Fail(Failed::NoPhp));
    assert_eq!(results[3].1, Outcome::Pass(Passed::Branch));

    fx.sh("rm app/artisan");
    let results = check(&fx.ssh(), &fx.request()).await;
    assert_eq!(results[4].1, Outcome::Fail(Failed::NoArtisan));
}

#[tokio::test]
async fn the_log_probe_names_a_missing_or_unreadable_file() {
    let fx = Fixture::new();
    let request = Request {
        log_path: "storage/logs/other.log".into(),
        ..fx.request()
    };
    let results = check(&fx.ssh(), &request).await;
    assert_eq!(
        results[5].1,
        Outcome::Fail(Failed::NoLog("storage/logs/other.log".into()))
    );

    // Root reads any file, so the mode alone proves nothing there.
    if unsafe { libc::geteuid() } != 0 {
        fx.sh("chmod 000 app/storage/logs/laravel.log");
        let results = check(&fx.ssh(), &fx.request()).await;
        assert_eq!(
            results[5].1,
            Outcome::Fail(Failed::LogUnreadable("storage/logs/laravel.log".into()))
        );
    }
}

#[tokio::test]
async fn a_daily_log_reads_the_newest_dated_file() {
    let fx = Fixture::new();
    let request = Request {
        log_path: "storage/logs/laravel".into(),
        log_daily: true,
        ..fx.request()
    };
    let results = check(&fx.ssh(), &request).await;
    assert_eq!(
        results[5].1,
        Outcome::Fail(Failed::NoLog("storage/logs/laravel-YYYY-MM-DD.log".into()))
    );

    fx.sh(
        "cd app/storage/logs && touch laravel-2026-11-12.log laravel-2026-11-13.log laravel-x.log",
    );
    let results = check(&fx.ssh(), &request).await;
    assert_eq!(
        results[5].1,
        Outcome::Pass(Passed::Log("storage/logs/laravel-2026-11-13.log".into()))
    );
}

#[tokio::test]
async fn quoting_keeps_a_hostile_path_and_log_as_data() {
    let fx = Fixture::new();
    let request = Request {
        path: format!("{}/x'; touch pwned; '", fx.app()),
        log_path: "$(touch pwned)".into(),
        ..fx.request()
    };
    check(&fx.ssh(), &request).await;
    let request = Request {
        log_path: "$(touch pwned)'\"".into(),
        ..fx.request()
    };
    check(&fx.ssh(), &request).await;
    assert!(!fx.root.join("pwned").exists());
    assert!(!fx.root.join("app/pwned").exists());
    assert!(!Path::new("/pwned").exists());
}

/// Every file's bytes, mode and modification time under `root`.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u32, std::time::SystemTime)> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                dirs.push(path.clone());
            }
            let bytes = if meta.is_file() {
                fs::read(&path).unwrap()
            } else {
                Vec::new()
            };
            files.insert(
                path,
                (bytes, meta.permissions().mode(), meta.modified().unwrap()),
            );
        }
    }
    files
}

#[tokio::test]
async fn no_probe_writes_anything_on_the_server() {
    let fx = Fixture::new();
    fx.sh("cd app/storage/logs && touch laravel-2026-11-13.log");
    let before = snapshot(&fx.root);
    let daily = Request {
        log_path: "storage/logs/laravel".into(),
        log_daily: true,
        ..fx.request()
    };
    for request in [fx.request(), daily] {
        let results = check(&fx.ssh(), &request).await;
        assert!(
            results
                .iter()
                .all(|(_, outcome)| matches!(outcome, Outcome::Pass(_))),
            "{results:?}"
        );
    }
    assert_eq!(snapshot(&fx.root), before);

    for probe in [Probe::Checkout, Probe::Branch, Probe::Artisan, Probe::Log] {
        let text = script(probe, &fx.request());
        for writer in [
            "git fetch",
            "git pull",
            "git checkout",
            "git reset",
            "git status",
            "git gc",
            "rm ",
            "mv ",
            "cp ",
            "touch ",
            "mkdir",
            "tee ",
            "chmod",
            "sudo",
            "artisan down",
            "artisan up",
            "artisan migrate",
            "artisan optimize",
            "artisan cache",
        ] {
            assert!(!text.contains(writer), "{probe:?} runs `{writer}`");
        }
        assert!(
            !text.lines().any(|line| line.contains('>')
                && !line.contains(">/dev/null")
                && !line.contains("2>&1")
                && !line.contains(">&2")),
            "{probe:?} redirects into a file:\n{text}"
        );
    }
}

#[tokio::test]
async fn a_probe_that_exits_non_zero_is_unexpected_with_its_last_lines() {
    let fx = Fixture::new();
    let ssh = FakeSsh {
        prefix: "echo 'profile: broken'; exit 3".into(),
        ..fx.ssh()
    };
    let results = check(&ssh, &fx.request()).await;
    let unexpected = Outcome::Fail(Failed::Unexpected {
        code: 3,
        output: vec!["profile: broken".into()],
    });
    assert_eq!(
        outcomes(&results)[2..],
        [
            unexpected.clone(),
            Outcome::NotRun,
            unexpected.clone(),
            unexpected
        ]
    );
}

#[tokio::test]
async fn a_probe_that_prints_none_of_its_answers_is_unexpected() {
    let fx = Fixture::new();
    let ssh = FakeSsh {
        prefix: "exec >/dev/null".into(),
        ..fx.ssh()
    };
    let results = check(&ssh, &fx.request()).await;
    let unexpected = Outcome::Fail(Failed::Unexpected {
        code: 0,
        output: Vec::new(),
    });
    assert_eq!(
        outcomes(&results)[2..],
        [
            Outcome::Fail(Failed::NotCheckout(Vec::new())),
            Outcome::NotRun,
            unexpected.clone(),
            unexpected.clone()
        ]
    );

    // Only the branch probe goes quiet.
    let ssh = FakeSsh {
        prefix: r#"echo() { case "$1" in @branch*|@detached) ;; *) builtin echo "$@" ;; esac; }"#
            .into(),
        ..fx.ssh()
    };
    let results = check(&ssh, &fx.request()).await;
    assert_eq!(results[2].1, Outcome::Pass(Passed::Checkout));
    assert_eq!(results[3].1, unexpected);
}
