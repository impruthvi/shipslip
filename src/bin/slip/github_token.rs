//! CLI-only token entry and GitHub account validation.

use std::fs::File;
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use serde::Deserialize;
use shipslip::github_auth::{GitHubRepository, GitHubToken};
use shipslip::logs::escape;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use zeroize::{Zeroize, Zeroizing};

use super::{ask, invalid_input, read_answer, Interrupts};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenSource {
    Prompt,
    Env,
    Gh,
}

impl TokenSource {
    pub(super) fn parse(value: &str) -> io::Result<Self> {
        match value {
            "prompt" => Ok(Self::Prompt),
            "env" => Ok(Self::Env),
            "gh" => Ok(Self::Gh),
            _ => Err(invalid_input(
                "--github-token-source must be prompt, env, or gh",
            )),
        }
    }
}

/// Capture only before starting Tokio, while this process is single-threaded.
/// Removing ambient token variables prevents OpenSSH SendEnv and unrelated
/// child commands from inheriting them. The parent shell is not changed.
#[derive(Default)]
pub(super) struct LocalTokens {
    gh: Option<Zeroizing<String>>,
    github: Option<Zeroizing<String>>,
}

impl LocalTokens {
    pub(super) fn capture(enabled: bool) -> Self {
        if !enabled {
            return Self::default();
        }
        let result = Self {
            gh: std::env::var("GH_TOKEN").ok().map(Zeroizing::new),
            github: std::env::var("GITHUB_TOKEN").ok().map(Zeroizing::new),
        };
        for key in TOKEN_ENV {
            std::env::remove_var(key);
        }
        result
    }

    fn take(&mut self) -> io::Result<(GitHubToken, &'static str)> {
        let (value, label) = if let Some(value) = self.gh.take() {
            (value, "GH_TOKEN")
        } else if let Some(value) = self.github.take() {
            (value, "GITHUB_TOKEN")
        } else {
            return Err(invalid_input(
                "no local GH_TOKEN or GITHUB_TOKEN is set; use --github-token for hidden entry",
            ));
        };
        let token = GitHubToken::new(value.trim().to_string()).map_err(io::Error::other)?;
        Ok((token, label))
    }
}

const TOKEN_ENV: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "SHIPSLIP_GITHUB_TOKEN",
];

fn clean_child(command: &mut Command) {
    for key in TOKEN_ENV {
        command.env_remove(key);
    }
    command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
}

/// Guard lives on the async task, rather than on the blocking reader, so
/// cancelling the read restores echo before the CLI exits.
struct EchoGuard {
    terminal: File,
    original: libc::termios,
}

impl EchoGuard {
    fn new(terminal: File) -> io::Result<Self> {
        let fd = terminal.as_raw_fd();
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: fd refers to a live terminal; tcgetattr initializes termios
        // on success, which is checked before assume_init.
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        let mut hidden = original;
        hidden.c_lflag &= !(libc::ECHO | libc::ECHONL);
        // Canonical mode makes poll ready only when a full line is available.
        hidden.c_lflag |= libc::ICANON | libc::ISIG;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { terminal, original })
    }
}

impl Drop for EchoGuard {
    fn drop(&mut self) {
        // SAFETY: the terminal is still owned by the guard. Retry EINTR so a
        // signal during cleanup does not leave the user's terminal hidden.
        while unsafe { libc::tcsetattr(self.terminal.as_raw_fd(), libc::TCSANOW, &self.original) }
            != 0
        {
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
    }
}

async fn hidden_token(interrupts: &mut Interrupts) -> io::Result<Option<GitHubToken>> {
    if !io::stdin().is_terminal() {
        return Err(invalid_input("hidden token entry needs an interactive terminal; select --github-token-source env or gh explicitly"));
    }
    // Duplicate the actual terminal fd. On macOS the /dev/tty alias does not
    // reliably support poll, while the terminal's own device does.
    let terminal = File::from(io::stdin().as_fd().try_clone_to_owned()?);
    let guard = EchoGuard::new(terminal)?;
    print!("GitHub token (hidden; Enter cancels): ");
    io::stdout().flush()?;
    let answer = read_hidden(guard, interrupts).await;
    println!();
    let Some(answer) = answer? else {
        return Ok(None);
    };
    let answer = Zeroizing::new(answer);
    if answer.is_empty() {
        return Ok(None);
    }
    GitHubToken::new(answer.to_string())
        .map(Some)
        .map_err(io::Error::other)
}

async fn auth_answer(
    read: impl FnOnce() -> io::Result<String> + Send + 'static,
    interrupts: &mut Interrupts,
) -> io::Result<Option<String>> {
    let answer = ask(read, interrupts).await?;
    if answer.is_none() {
        interrupts.pending = true;
    }
    Ok(answer)
}

async fn read_hidden(guard: EchoGuard, interrupts: &mut Interrupts) -> io::Result<Option<String>> {
    let reader = guard.terminal.try_clone()?;
    let answer = read_terminal(reader, interrupts).await;
    drop(guard);
    answer
}

/// Cooperatively cancel the blocking terminal reader before exiting. Leaving
/// a thread blocked in a terminal read can stall shutdown on some platforms.
async fn read_terminal(
    mut reader: File,
    interrupts: &mut Interrupts,
) -> io::Result<Option<String>> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = cancelled.clone();
    let mut task = tokio::task::spawn_blocking(move || {
        let mut poll = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        while !stop.load(Ordering::SeqCst) {
            // SAFETY: one valid pollfd referring to the owned, live terminal.
            let ready = unsafe { libc::poll(&mut poll, 1, 50) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if ready > 0 {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if poll.revents & libc::POLLIN != 0 {
                    // One canonical read returns one line or an EOF record.
                    // read_line would wait for another record after Ctrl-D
                    // with partial input and could not then be cancelled.
                    let mut bytes = Zeroizing::new([0u8; 4097]);
                    let count = reader.read(&mut bytes[..])?;
                    let line = std::str::from_utf8(&bytes[..count])
                        .map_err(|_| io::Error::other("token input is not valid UTF-8"))?;
                    if !line.ends_with('\n') {
                        return Ok(String::new());
                    }
                    return Ok(line.trim().to_string());
                }
                return Err(io::Error::other("terminal closed during token entry"));
            }
        }
        Ok(String::new())
    });
    tokio::select! {
        answer = &mut task => answer.map_err(io::Error::other)?.map(Some),
        () = interrupts.recv() => {
            interrupts.pending = true;
            cancelled.store(true, Ordering::SeqCst);
            // Poll wakes within 50 ms; wait for the reader before dropping the
            // echo guard or exiting. Wipe a line if input won the race.
            if let Ok(Ok(answer)) = task.await { drop(Zeroizing::new(answer)); }
            Ok(None)
        }
    }
}

async fn gh_token() -> io::Result<GitHubToken> {
    let mut command = Command::new("gh");
    clean_child(&mut command);
    command
        .args(["auth", "token", "--hostname", "github.com"])
        .env("GH_PROMPT_DISABLED", "1");
    let mut output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .map_err(|_| io::Error::other("GitHub CLI token lookup timed out"))?
        .map_err(|_| {
            io::Error::other(
                "could not run gh; install GitHub CLI and run gh auth login, or use --github-token",
            )
        })?;
    if !output.status.success() {
        output.stdout.zeroize();
        return Err(io::Error::other(
            "no usable GitHub CLI login for github.com; run gh auth login or use --github-token",
        ));
    }
    let value = Zeroizing::new(String::from_utf8_lossy(&output.stdout).trim().to_string());
    output.stdout.zeroize();
    GitHubToken::new(value.to_string()).map_err(io::Error::other)
}

struct ApiResponse {
    status: u16,
    headers: Zeroizing<String>,
    body: Zeroizing<String>,
}

impl ApiResponse {
    fn parse(output: &str) -> io::Result<Self> {
        let (response, status) = output
            .rsplit_once("\n@shipslip-http ")
            .ok_or_else(|| io::Error::other("GitHub returned an invalid HTTP response"))?;
        let status = status
            .trim()
            .parse::<u16>()
            .map_err(|_| io::Error::other("GitHub returned an invalid HTTP status"))?;
        let mut body = response;
        let mut headers = String::new();
        // curl can include a proxy CONNECT response before the API headers.
        while body.starts_with("HTTP/") {
            let (block, rest) = body
                .split_once("\r\n\r\n")
                .or_else(|| body.split_once("\n\n"))
                .ok_or_else(|| io::Error::other("GitHub returned invalid HTTP headers"))?;
            headers = block.to_ascii_lowercase();
            body = rest;
        }
        Ok(Self {
            status,
            headers: Zeroizing::new(headers),
            body: Zeroizing::new(body.to_string()),
        })
    }

    fn check(&self, repository: bool) -> io::Result<()> {
        let reason = match self.status {
            200 => return Ok(()),
            401 => "GitHub rejected the token; it may be invalid, expired, or revoked",
            403 if self.headers.lines().any(|line| line.starts_with("x-github-sso:") && line.contains("required")) =>
                "GitHub requires SSO authorization for this token; authorize it for the organization and retry",
            403 if self.headers.lines().any(|line| line.trim() == "x-ratelimit-remaining: 0") =>
                "GitHub API rate limit reached; retry after the limit resets",
            429 => "GitHub API rate limit reached; retry later",
            404 if repository =>
                "the repository is missing or unavailable to this token; check its path, repository selection, and organization approval",
            403 => "GitHub denied access; check token permissions and organization policies or approval",
            300..=399 => "GitHub redirected the request; redirects are refused for token authentication",
            500..=599 => "GitHub is temporarily unavailable; retry later",
            _ => "GitHub could not validate this token; use a personal access token with repository read access",
        };
        Err(io::Error::other(reason))
    }
}

async fn api(token: &GitHubToken, endpoint: &str) -> io::Result<ApiResponse> {
    let mut command = Command::new("curl");
    clean_child(&mut command);
    // --disable MUST be first: ignore curlrc files that could enable tracing,
    // redirects, or insecure TLS. The secret header goes only through stdin.
    command
        .args([
            "--disable",
            "--config",
            "-",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--max-time",
            "15",
            "--connect-timeout",
            "5",
            "--max-filesize",
            "262144",
            "--include",
            "--write-out",
            "\n@shipslip-http %{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| io::Error::other("curl is required to validate the GitHub token"))?;
    let config = Zeroizing::new(format!(
        "url = \"https://api.github.com{endpoint}\"\nheader = \"Authorization: Bearer {}\"\nheader = \"Accept: application/vnd.github+json\"\nheader = \"X-GitHub-Api-Version: 2022-11-28\"\nuser-agent = \"Shipslip/{}\"\n",
        token.expose_secret(), env!("CARGO_PKG_VERSION")
    ));
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin
        .write_all(config.as_bytes())
        .await
        .map_err(|_| io::Error::other("could not send credentials to curl"))?;
    drop(stdin);
    drop(config);
    let mut output = child.wait_with_output().await?;
    if !output.status.success() {
        output.stdout.zeroize();
        let message = match output.status.code() {
            Some(6) => "could not resolve api.github.com; check your network or DNS",
            Some(7) => "could not connect to GitHub; check your network or proxy",
            Some(28) => "GitHub token validation timed out; check your network and retry",
            Some(35 | 60) => "could not verify the secure connection to GitHub; check TLS certificates or your proxy",
            _ => "could not contact GitHub to validate the token",
        };
        return Err(io::Error::other(message));
    }
    let response = ApiResponse::parse(&String::from_utf8_lossy(&output.stdout));
    output.stdout.zeroize();
    response
}

#[derive(Deserialize)]
struct Account {
    login: String,
}

async fn identify_account(token: &GitHubToken) -> io::Result<String> {
    let user = api(token, "/user").await?;
    user.check(false)?;
    let account: Account = serde_json::from_str(&user.body)
        .map_err(|_| io::Error::other("GitHub did not return an account identity"))?;
    if account.login.is_empty()
        || account.login.len() > 100
        || !account
            .login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(io::Error::other(
            "GitHub returned an invalid account identity",
        ));
    }
    Ok(account.login)
}

async fn check_repository(token: &GitHubToken, repository: &GitHubRepository) -> io::Result<()> {
    api(token, &format!("/repos/{}", repository.name()))
        .await?
        .check(true)
}

fn print_guidance(repository: &GitHubRepository) {
    println!(
        "Need a token? Create one while signed into the account with access to {}:",
        repository.name()
    );
    println!("  https://github.com/settings/personal-access-tokens/new?target_name={}&contents=read&expires_in=30", repository.owner());
    println!(
        "Select this repository and Contents: Read-only. Organization approval may be required."
    );
    println!("Classic tokens may require SSO authorization. Use the minimum access your organization permits.");
}

#[derive(Debug, PartialEq, Eq)]
enum AccountChoice {
    Continue,
    Replace,
    Cancel,
}

fn account_choice(answer: &str) -> AccountChoice {
    match answer.to_ascii_lowercase().as_str() {
        "y" | "yes" => AccountChoice::Continue,
        "r" | "replace" => AccountChoice::Replace,
        _ => AccountChoice::Cancel,
    }
}

/// No token reaches the server until the user has approved its account.
pub(super) async fn select_token(
    mut source: TokenSource,
    local: &mut LocalTokens,
    repository: &GitHubRepository,
    environment: &str,
    interrupts: &mut Interrupts,
) -> io::Result<Option<GitHubToken>> {
    println!(
        "Environment: {}\nRepository: {}",
        escape(environment),
        repository.name()
    );
    let mut guidance_shown = false;
    loop {
        if source == TokenSource::Prompt && !guidance_shown {
            print_guidance(repository);
            guidance_shown = true;
        }
        let loaded = match source {
            TokenSource::Prompt => hidden_token(interrupts)
                .await
                .map(|token| token.map(|t| (t, "entered for this deployment"))),
            TokenSource::Env => local.take().map(Some),
            TokenSource::Gh => tokio::select! {
                result = gh_token() => result.map(|token| Some((token, "GitHub CLI login for github.com"))),
                () = interrupts.recv() => {
                    interrupts.pending = true;
                    return Ok(None);
                }
            },
        };
        let (token, label) = match loaded {
            Ok(Some(value)) => value,
            Ok(None) => return Ok(None),
            Err(error)
                if source == TokenSource::Prompt
                    && error.get_ref().is_some_and(|e| {
                        e.downcast_ref::<shipslip::github_auth::InvalidGitHubToken>()
                            .is_some()
                    }) =>
            {
                eprintln!("{}", escape(&error.to_string()));
                continue;
            }
            Err(error) => return Err(error),
        };
        let validating = async {
            let account = identify_account(&token).await?;
            println!("Token account: {account}\nToken source: {label}");
            check_repository(&token, repository).await
        };
        let validation = tokio::select! {
            result = validating => result,
            () = interrupts.recv() => {
                interrupts.pending = true;
                return Ok(None);
            }
        };
        match validation {
            Ok(()) => {
                println!("Repository visible; fetch access will be checked on the server.");
            }
            Err(error) => {
                eprintln!("{}", escape(&error.to_string()));
                if !io::stdin().is_terminal() {
                    return Err(error);
                }
                print!("Enter another token? [r/N] ");
                io::stdout().flush()?;
                let Some(answer) = auth_answer(read_answer, interrupts).await? else {
                    return Ok(None);
                };
                if account_choice(&answer) != AccountChoice::Replace {
                    return Ok(None);
                }
                source = TokenSource::Prompt;
                continue;
            }
        }
        println!("The token will be sent over SSH for this fetch only and will not be saved by Shipslip.");
        print!("Use this account? [y] Continue / [r] Enter another token / [N] Cancel: ");
        io::stdout().flush()?;
        let Some(answer) = auth_answer(read_answer, interrupts).await? else {
            return Ok(None);
        };
        match account_choice(&answer) {
            AccountChoice::Continue => return Ok(Some(token)),
            AccountChoice::Cancel => return Ok(None),
            AccountChoice::Replace => source = TokenSource::Prompt,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::FromRawFd;

    use super::*;

    #[test]
    fn api_errors_are_specific_without_echoing_response_bodies() {
        for (status, headers, expected) in [
            (401, "", "invalid, expired, or revoked"),
            (
                403,
                "X-GitHub-SSO: required; url=https://github.com/secret",
                "SSO authorization",
            ),
            (403, "X-RateLimit-Remaining: 0", "rate limit"),
            (429, "", "rate limit"),
            (404, "", "missing or unavailable"),
            (403, "", "organization policies"),
            (302, "", "redirects are refused"),
            (503, "", "temporarily unavailable"),
        ] {
            let response = ApiResponse::parse(&format!("HTTP/2 {status}\r\n{headers}\r\n\r\n{{\"message\":\"secret-token\"}}\n@shipslip-http {status}")).unwrap();
            let error = response.check(true).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("secret-token"));
        }
        let response = ApiResponse::parse("HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 200\r\nContent-Type: application/json\r\n\r\n{\"login\":\"work-user\"}\n@shipslip-http 200").unwrap();
        response.check(false).unwrap();
        assert_eq!(
            serde_json::from_str::<Account>(&response.body)
                .unwrap()
                .login,
            "work-user"
        );
        assert!(ApiResponse::parse("malformed").is_err());
    }

    #[test]
    fn env_source_priority_and_child_environment_are_explicit() {
        let mut tokens = LocalTokens {
            gh: Some(Zeroizing::new("ghp_work".into())),
            github: Some(Zeroizing::new("ghp_personal".into())),
        };
        let (token, label) = tokens.take().unwrap();
        assert_eq!(token.expose_secret(), "ghp_work");
        assert_eq!(label, "GH_TOKEN");
        assert!(LocalTokens::default().take().is_err());
        let mut command = Command::new("curl");
        clean_child(&mut command);
        for key in TOKEN_ENV {
            assert!(command
                .as_std()
                .get_envs()
                .any(|(name, value)| name == *key && value.is_none()));
        }
        assert_eq!(account_choice("r"), AccountChoice::Replace);
        assert_eq!(account_choice("yes"), AccountChoice::Continue);
        assert_eq!(account_choice(""), AccountChoice::Cancel);
        assert_eq!(account_choice("work-user"), AccountChoice::Cancel);
    }

    fn pty() -> (File, File) {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: valid out-pointers; default terminal attributes and size.
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
        // SAFETY: openpty returned owned file descriptors on success.
        unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) }
    }

    fn echo(file: &File) -> bool {
        let mut state = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: live terminal fd and a valid output pointer.
        assert_eq!(
            unsafe { libc::tcgetattr(file.as_raw_fd(), state.as_mut_ptr()) },
            0
        );
        unsafe { state.assume_init().c_lflag & libc::ECHO != 0 }
    }

    #[tokio::test]
    async fn hidden_entry_restores_echo_after_success_eof_and_cancellation() {
        let (mut master, slave) = pty();
        assert!(echo(&slave));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut interrupts = Interrupts::from_channel(rx);

        let reader = slave.try_clone().unwrap();
        let eof = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            master.write_all(b"ghp_TEST_ONLY\x04").unwrap();
        };
        let (answer, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                read_hidden(EchoGuard::new(reader).unwrap(), &mut interrupts),
                eof
            )
        })
        .await
        .expect("partial EOF blocked token entry");
        assert_eq!(answer.unwrap(), Some(String::new()));
        assert!(echo(&slave));
        let reader = slave.try_clone().unwrap();
        let input = async {
            // Yield to the guard before simulating terminal input.
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(!echo(&slave));
            master.write_all(b"ghp_TEST_ONLY\n").unwrap();
        };
        let (answer, ()) = tokio::join!(
            read_hidden(EchoGuard::new(reader).unwrap(), &mut interrupts),
            input
        );
        assert_eq!(answer.unwrap().unwrap(), "ghp_TEST_ONLY");
        assert!(echo(&slave));

        let reader = slave.try_clone().unwrap();
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(!echo(&slave));
            tx.send(()).unwrap();
        };
        let (answer, ()) = tokio::join!(
            read_hidden(EchoGuard::new(reader).unwrap(), &mut interrupts),
            cancel
        );
        assert!(answer.unwrap().is_none());
        assert!(interrupts.pending);
        assert!(echo(&slave));
    }
}
