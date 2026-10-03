//! Deterministic auth tests: real SSH, Git credentials and receipts; only the
//! external GitHub API/network fetch are replaced. No real tokens are used.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;

use shipslip::github_auth::{discover_repository, GitHubToken};
use shipslip::prepare_with_github_token;

use super::*;

const REMOTE_PROFILE: &str = r#"git() {
  if [ "$1" != -c ]; then command git "$@"; return; fi
  configs=()
  while [ "$1" = -c ]; do configs+=("$1" "$2"); shift 2; done
  [ "$1" = fetch ] && [ "$2" = --no-recurse-submodules ] &&
    [ "$3" = https://github.com/acme/app.git ] || return 81
  credentials=$(printf 'protocol=https\nhost=github.com\npath=acme/app.git\n\n' |
    command git "${configs[@]}" credential fill) || return 82
  [[ "$credentials" == *"password=$SHIPSLIP_GITHUB_TOKEN"* ]] || return 83
  printf '%s\n\n' "$credentials" | command git "${configs[@]}" credential approve || return 84
  echo fetched >> "$HOME/auth-fetches"
  command git fetch "$HOME/origin.git" "$4"
}
"#;

fn install_remote_auth(server: &Server, path: &str) {
    server.exec("cat > ~/.bash_profile", REMOTE_PROFILE);
    server.exec(&format!(
        "cd {}; git remote set-url origin git@github.com:acme/app.git; git config credential.helper '!cat >> /home/deploy/credential-spy'; git config credential.https://github.com/acme/app.git.helper '!cat >> /home/deploy/credential-spy'",
        quote(path)
    ), "");
}

#[tokio::test]
async fn temporary_github_token_fetches_over_ssh_without_persisting_or_reaching_steps() {
    let server = Server::start().await;
    let path = server.app("auth-app");
    install_remote_auth(&server, &path);
    let ssh = Arc::new(server.connect().await);
    let repository = discover_repository(ssh.as_ref(), &path).await.unwrap();
    assert_eq!(repository.name(), "acme/app");
    let token = GitHubToken::new("ghp_TEST_ONLY_work".into()).unwrap();
    let deploy_target = target(&path, &["test -z \"${SHIPSLIP_GITHUB_TOKEN+x}\" && test -z \"${GH_TOKEN+x}\" && echo token-not-in-step"]);
    let preview = prepare_with_github_token(
        deploy_target,
        RunPlan::Deploy,
        ssh.as_ref(),
        &repository,
        &token,
    )
    .await
    .unwrap();
    assert!(!format!("{preview:?}").contains(token.expose_secret()));
    let root = temp_dir();
    let journal =
        Arc::new(ReceiptJournal::create(&root, "auth-app", &server.dir, &preview).unwrap());
    let receipt_path = journal.path().to_path_buf();
    let confirmation = Confirmation::from(&preview, None).unwrap();
    let (mut events, _handle) =
        execute_recorded(preview, confirmation, ssh.clone(), journal).unwrap();
    let mut succeeded = false;
    while let Some(event) = events.recv().await {
        assert!(!format!("{event:?}").contains(token.expose_secret()));
        if matches!(event, DeployEvent::Finished(DeployOutcome::Succeeded)) {
            succeeded = true;
        }
    }
    assert!(succeeded);
    let receipt = std::fs::read_to_string(receipt_path).unwrap();
    assert!(!receipt.contains(token.expose_secret()));
    assert!(receipt.contains("token-not-in-step"));
    let saved = server.exec(
        "find ~/.shipslip -type f -exec cat {} +; test ! -e ~/credential-spy",
        "",
    );
    assert!(!saved.contains(token.expose_secret()));
    assert_eq!(
        server.exec(
            &format!("git -C {} config --get remote.origin.url", quote(&path)),
            ""
        ),
        "git@github.com:acme/app.git"
    );
    assert!(!server.lock_exists(&path));
    let _ = std::fs::remove_dir_all(root);

    // The repository shown to the user cannot change before authenticated use.
    server.exec(
        &format!(
            "git -C {} remote set-url origin https://github.com/other/app.git",
            quote(&path)
        ),
        "",
    );
    let error = prepare_with_github_token(
        target(&path, &["true"]),
        RunPlan::Rerun,
        ssh.as_ref(),
        &repository,
        &token,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("origin changed"));
    assert_eq!(server.exec("wc -l < ~/auth-fetches", ""), "1");
    assert!(!server.lock_exists(&path));
}

struct CliFixture {
    client: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

impl CliFixture {
    fn new(server: &Server, path: &str) -> Self {
        let client = server.dir.join("client");
        // OpenSSH adds a random suffix to its socket path. Keep this fake
        // home short enough for macOS's Unix socket path limit.
        let home = PathBuf::from("/tmp").join(format!(
            "{}-home",
            server.dir.file_name().unwrap().to_string_lossy()
        ));
        let bin = server.dir.join("bin");
        std::fs::create_dir_all(&client).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(&client)
            .status()
            .unwrap()
            .success());
        std::fs::write(client.join(".shipslip.toml"), format!(
            "[project]\nname = \"auth-test\"\nstack = \"laravel\"\n[env.staging]\nssh = \"server\"\npath = \"{path}\"\nbranch = \"main\"\nproduction = false\n[recipe.deploy]\nsteps = [\"true\"]\n"
        )).unwrap();
        let fixture = Self { client, home, bin };
        fixture.executable("ssh", &format!("#!/bin/sh\n[ -z \"${{GH_TOKEN+x}}\" ] && [ -z \"${{GITHUB_TOKEN+x}}\" ] || exit 91\nexec /usr/bin/ssh -F {} \"$@\"\n", quote(&server.config().to_string_lossy())));
        fixture.executable("curl", MOCK_CURL);
        fixture.executable("gh", "#!/bin/sh\n[ -z \"${GH_TOKEN+x}\" ] && [ -z \"${GITHUB_TOKEN+x}\" ] || exit 92\nprintf called > \"$HOME/gh-called\"\nprintf ghp_TEST_ONLY_personal\n");
        // Trust is a separate operation, and does not use token auth. Its
        // child environment is deliberately kept free of test tokens.
        let trusted = fixture.run(&["trust", "staging"], "staging\n");
        assert!(
            trusted.status.success(),
            "{}",
            String::from_utf8_lossy(&trusted.stderr)
        );
        fixture
    }

    fn executable(&self, name: &str, script: &str) {
        let path = self.bin.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_slip"));
        cmd.args(args)
            .current_dir(&self.client)
            .env("HOME", &self.home)
            .env(
                "PATH",
                format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            )
            .env_remove("SHIPSLIP_CONFIG")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    fn run(&self, args: &[&str], input: &str) -> std::process::Output {
        let mut child = self.command(args).spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for CliFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

const MOCK_CURL: &str = r#"#!/bin/sh
[ "$1" = --disable ] && [ "$2" = --config ] && [ "$3" = - ] || exit 93
case "$*" in *ghp_TEST_ONLY*|*Authorization:*|*--insecure*|*--location*) exit 96 ;; esac
[ -z "${GH_TOKEN+x}" ] && [ -z "${GITHUB_TOKEN+x}" ] || exit 94
config=$(cat)
case "$config" in
  *'Bearer ghp_TEST_ONLY_work'*) account=work-user ;;
  *'Bearer ghp_TEST_ONLY_personal'*) account=personal-user ;;
  *) printf 'HTTP/2 401\r\n\r\n{"message":"Bad credentials"}\n@shipslip-http 401'; exit 0 ;;
esac
case "$config" in
  *'https://api.github.com/user"'*) body="{\"login\":\"$account\"}" ;;
  *'https://api.github.com/repos/acme/app"'*) body='{"full_name":"acme/app"}' ;;
  *) exit 95 ;;
esac
printf 'HTTP/2 200\r\n\r\n%s\n@shipslip-http 200' "$body"
"#;

#[tokio::test]
async fn cli_github_token_sources_show_the_actual_account_and_cancel_before_fetch() {
    let server = Server::start().await;
    let path = server.app("cli-auth-app");
    install_remote_auth(&server, &path);
    let fixture = CliFixture::new(&server, &path);
    let mut command = fixture.command(&["deploy", "staging", "--github-token-source", "env"]);
    command
        .env("GH_TOKEN", "ghp_TEST_ONLY_work")
        .env("GITHUB_TOKEN", "ghp_TEST_ONLY_personal");
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"n\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Token account: work-user"));
    assert!(!text.contains("ghp_TEST_ONLY"));
    assert!(!fixture.home.join("gh-called").exists());
    assert_eq!(
        server.exec("test ! -e ~/auth-fetches && echo no-fetch", ""),
        "no-fetch"
    );

    let output = fixture.run(&["deploy", "staging", "--github-token-source", "gh"], "n\n");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Token account: personal-user"));
    assert!(fixture.home.join("gh-called").exists());
    std::fs::remove_file(fixture.home.join("gh-called")).unwrap();

    let output = fixture.run(&["deploy", "staging", "--github-token"], "");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert!(!fixture.home.join("gh-called").exists());
    assert!(!server.lock_exists(&path));
    assert_eq!(
        server.exec("test ! -e ~/auth-fetches && echo no-fetch", ""),
        "no-fetch"
    );

    // Approve only the account, then decline the normal deploy preview.
    let mut child = fixture
        .command(&["deploy", "staging", "--github-token-source", "env"])
        .env("GH_TOKEN", "ghp_TEST_ONLY_work")
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"y\nn\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Repository fetch access: verified"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("ghp_TEST_ONLY"));
    assert!(!server.lock_exists(&path));
}

fn terminal_pair() -> (std::fs::File, std::fs::File) {
    use std::os::fd::FromRawFd;
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: valid output pointers, default terminal attributes and size.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    // openpty does not set CLOEXEC. Inheriting the master into the CLI can
    // stall session-leader exit on macOS, so only its stdin keeps the slave.
    for fd in [master, slave] {
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    // The CLI's stdout is a pipe; drain echoed terminal input without blocking
    // so macOS can close its controlling terminal when the child exits.
    let flags = unsafe { libc::fcntl(master, libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_eq!(
        unsafe { libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    // SAFETY: openpty returned these owned file descriptors.
    unsafe {
        (
            std::fs::File::from_raw_fd(master),
            std::fs::File::from_raw_fd(slave),
        )
    }
}

fn echo_enabled(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    let mut settings = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: live terminal fd, valid output pointer; initialization checked.
    assert_eq!(
        unsafe { libc::tcgetattr(file.as_raw_fd(), settings.as_mut_ptr()) },
        0
    );
    unsafe { settings.assume_init().c_lflag & libc::ECHO != 0 }
}

fn terminal_cli(
    fixture: &CliFixture,
    slave: std::fs::File,
    args: &[&str],
) -> tokio::process::Child {
    let mut command = tokio::process::Command::from(fixture.command(args));
    command
        .stdin(Stdio::from(slave))
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    // Like a CLI launched by a shell, the child uses the terminal without
    // owning its session. A session-leader exit revokes the PTY on macOS and
    // prevents checking echo afterwards.
    command.spawn().unwrap()
}

async fn await_text(stdout: &mut tokio::process::ChildStdout, output: &mut String, text: &str) {
    use tokio::io::AsyncReadExt;
    tokio::time::timeout(Duration::from_secs(15), async {
        while !output.contains(text) {
            let mut bytes = [0u8; 4096];
            let count = stdout.read(&mut bytes).await.unwrap();
            assert!(count != 0, "CLI exited before {text}: {output}");
            output.push_str(&String::from_utf8_lossy(&bytes[..count]));
        }
    })
    .await
    .unwrap_or_else(|_| panic!("CLI did not reach {text}: {output}"));
}

async fn terminal_exit(
    child: &mut tokio::process::Child,
    master: &mut std::fs::File,
) -> std::process::ExitStatus {
    use std::io::Read;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut bytes = [0u8; 4096];
            while let Ok(count) = master.read(&mut bytes) {
                if count == 0 {
                    break;
                }
                assert!(
                    !String::from_utf8_lossy(&bytes[..count]).contains("ghp_TEST_ONLY"),
                    "token was echoed"
                );
            }
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("CLI did not exit after terminal input")
}

#[tokio::test]
async fn cli_github_token_can_replace_accounts_and_restores_terminal_on_ctrl_c() {
    use tokio::io::AsyncReadExt;
    let server = Server::start().await;
    let path = server.app("terminal-auth-app");
    install_remote_auth(&server, &path);
    let fixture = CliFixture::new(&server, &path);
    let (mut master, slave) = terminal_pair();
    let monitor = slave.try_clone().unwrap();
    let mut child = terminal_cli(
        &fixture,
        slave,
        &["deploy", "staging", "--github-token-source", "gh"],
    );
    let mut stdout = child.stdout.take().unwrap();
    let mut output = String::new();
    await_text(&mut stdout, &mut output, "Use this account?").await;
    assert!(output.contains("Token account: personal-user"));
    master.write_all(b"r\n").unwrap();
    await_text(&mut stdout, &mut output, "GitHub token (hidden").await;
    assert!(!echo_enabled(&monitor));
    master.write_all(b"ghp_TEST_ONLY_work\n").unwrap();
    await_text(&mut stdout, &mut output, "Token account: work-user").await;
    assert!(echo_enabled(&monitor));
    master.write_all(b"y\n").unwrap();
    await_text(&mut stdout, &mut output, "Proceed with this run?").await;
    master.write_all(b"n\n").unwrap();
    let status = terminal_exit(&mut child, &mut master).await;
    assert!(status.success());
    stdout.read_to_string(&mut output).await.unwrap();
    assert!(!output.contains("ghp_TEST_ONLY"));
    assert!(!server.lock_exists(&path));

    let (mut master, slave) = terminal_pair();
    let monitor = slave.try_clone().unwrap();
    let mut child = terminal_cli(&fixture, slave, &["deploy", "staging", "--github-token"]);
    let mut stdout = child.stdout.take().unwrap();
    let mut output = String::new();
    await_text(&mut stdout, &mut output, "GitHub token (hidden").await;
    assert!(!echo_enabled(&monitor));
    // SAFETY: target is our live child; send the same signal as terminal Ctrl-C.
    assert_eq!(
        unsafe { libc::kill(child.id().unwrap() as libc::pid_t, libc::SIGINT) },
        0
    );
    let status = terminal_exit(&mut child, &mut master).await;
    assert_eq!(status.code(), Some(130));
    stdout.read_to_string(&mut output).await.unwrap();
    assert!(echo_enabled(&monitor));
    drop(master);
    assert_eq!(server.exec("wc -l < ~/auth-fetches", ""), "1");
    assert!(!server.lock_exists(&path));
}
