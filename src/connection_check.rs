//! The connection check: a person-started, read-only look at whether this
//! Mac reaches an environment, before or without an approval. It sits beside
//! [`crate::read_only`], not inside it: that connector keeps refusing
//! targets nobody approved, while this check connects only because a person
//! asked and only runs commands that read.
//! Probes after a failed connect are not run. Nothing retries or reconnects
//! on its own.

use std::future::Future;
use std::path::PathBuf;

use crate::runner::run_collect;
use crate::script::shell_quote;
use crate::ssh_aliases::{self, ResolveError, Resolved};
use crate::transport::{ConnectFailure, Transport, TransportError};
use crate::DeployTarget;

/// What to check: one environment's server, path, branch and log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub alias: String,
    pub path: String,
    pub branch: String,
    /// A log file, or the path prefix of daily files when `log_daily`.
    pub log_path: String,
    pub log_daily: bool,
}

impl Request {
    pub fn for_target(target: &DeployTarget) -> Self {
        Self {
            alias: target.ssh_alias.clone(),
            path: target.path.clone(),
            branch: target.branch.clone(),
            log_path: target.log_path().to_string(),
            log_daily: target.log_daily,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Alias,
    Connect,
    Checkout,
    Branch,
    /// Boots the app with `php artisan --version`. Laravel may refresh its own
    /// cache files while booting: the one accepted side effect (ADR 0005).
    Artisan,
    Log,
}

impl Probe {
    pub const ALL: [Probe; 6] = [
        Probe::Alias,
        Probe::Connect,
        Probe::Checkout,
        Probe::Branch,
        Probe::Artisan,
        Probe::Log,
    ];
}

/// Remote text in these values is server text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Passed {
    Alias(Resolved),
    Connected,
    Checkout,
    Branch,
    /// The first line `php artisan --version` printed.
    Artisan(String),
    /// The log file read, relative to the remote path unless absolute.
    Log(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failed {
    /// `ssh -G` could not resolve the alias.
    Alias(String),
    Connect {
        failure: ConnectFailure,
        message: String,
    },
    /// The connection dropped during this probe.
    Lost(String),
    /// `cd` into the remote path failed.
    NoFolder,
    NotCheckout(Vec<String>),
    OtherBranch(String),
    Detached,
    NoArtisan,
    NoPhp,
    /// `php artisan --version` exited non-zero; its first lines.
    Artisan {
        code: i32,
        output: Vec<String>,
    },
    NoLog(String),
    LogUnreadable(String),
    /// A probe script ended in a way none of the above describes.
    Unexpected {
        code: i32,
        output: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass(Passed),
    Fail(Failed),
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckEvent {
    Started(Probe),
    Finished(Probe, Outcome),
}

/// How the check reaches ssh. [`SystemSsh`] is the user's own client; tests
/// substitute a fake at this boundary.
pub trait Ssh {
    type Connection: Transport;
    /// Asks what `alias` resolves to without connecting.
    fn resolve(&self, alias: &str) -> Result<Resolved, ResolveError>;
    fn connect(
        &self,
        alias: &str,
    ) -> impl Future<Output = Result<Self::Connection, TransportError>> + Send;
}

/// The system OpenSSH client, with `StrictHostKeyChecking=yes`.
#[derive(Debug, Default, Clone)]
pub struct SystemSsh {
    /// Replaces `~/.ssh/config` (`ssh -F`).
    pub config: Option<PathBuf>,
}

#[cfg(unix)]
impl Ssh for SystemSsh {
    type Connection = crate::transport::SshTransport;

    fn resolve(&self, alias: &str) -> Result<Resolved, ResolveError> {
        ssh_aliases::resolve(alias, self.config.as_deref())
    }

    async fn connect(&self, alias: &str) -> Result<Self::Connection, TransportError> {
        match &self.config {
            Some(config) => {
                crate::transport::SshTransport::connect_with_config(alias, config).await
            }
            None => crate::transport::SshTransport::connect(alias).await,
        }
    }
}

/// Reports each probe as it starts and ends; returns every outcome.
pub async fn run<S: Ssh>(
    ssh: &S,
    request: &Request,
    mut events: impl FnMut(CheckEvent),
) -> Vec<(Probe, Outcome)> {
    let mut results = Vec::new();
    let mut report = |event: CheckEvent| {
        if let CheckEvent::Finished(probe, outcome) = &event {
            results.push((*probe, outcome.clone()));
        }
        events(event);
    };
    check(ssh, request, &mut report).await;
    results
}

async fn check<S: Ssh>(ssh: &S, request: &Request, report: &mut impl FnMut(CheckEvent)) {
    report(CheckEvent::Started(Probe::Alias));
    let alias = match ssh.resolve(&request.alias) {
        Ok(resolved) => Outcome::Pass(Passed::Alias(resolved)),
        Err(error) => Outcome::Fail(Failed::Alias(error.to_string())),
    };
    let resolved = matches!(alias, Outcome::Pass(_));
    report(CheckEvent::Finished(Probe::Alias, alias));
    if !resolved {
        return not_run(&Probe::ALL[1..], report);
    }

    report(CheckEvent::Started(Probe::Connect));
    let connection = match ssh.connect(&request.alias).await {
        Ok(connection) => connection,
        Err(error) => {
            let failed = match error.connect_failure() {
                Some(failure) => Failed::Connect {
                    failure,
                    message: error_text(&error),
                },
                None => Failed::Lost(error_text(&error)),
            };
            report(CheckEvent::Finished(Probe::Connect, Outcome::Fail(failed)));
            return not_run(&Probe::ALL[2..], report);
        }
    };
    report(CheckEvent::Finished(
        Probe::Connect,
        Outcome::Pass(Passed::Connected),
    ));

    let remote = [Probe::Checkout, Probe::Branch, Probe::Artisan, Probe::Log];
    let mut skip_branch = false;
    for (index, probe) in remote.into_iter().enumerate() {
        if probe == Probe::Branch && skip_branch {
            report(CheckEvent::Finished(probe, Outcome::NotRun));
            continue;
        }
        report(CheckEvent::Started(probe));
        let outcome = match run_collect(&connection, &script(probe, request)).await {
            (Err(error), _) => {
                report(CheckEvent::Finished(
                    probe,
                    Outcome::Fail(Failed::Lost(error_text(&error))),
                ));
                return not_run(&remote[index + 1..], report);
            }
            (Ok(code), lines) => parse(probe, request, code, &lines),
        };
        match &outcome {
            Outcome::Fail(Failed::NoFolder) => {
                report(CheckEvent::Finished(probe, outcome));
                return not_run(&remote[index + 1..], report);
            }
            Outcome::Fail(_) if probe == Probe::Checkout => skip_branch = true,
            _ => {}
        }
        report(CheckEvent::Finished(probe, outcome));
    }
}

fn not_run(probes: &[Probe], report: &mut impl FnMut(CheckEvent)) {
    for &probe in probes {
        report(CheckEvent::Finished(probe, Outcome::NotRun));
    }
}

fn error_text(error: &TransportError) -> String {
    match error {
        TransportError::Connect(message) | TransportError::ConnectionLost(message) => {
            message.clone()
        }
    }
}

/// The probe's script. Every command in it reads: none writes a file, the
/// git index, or the server's state. Booting Laravel for [`Probe::Artisan`]
/// is the one exception Shipslip doesn't control (ADR 0005).
fn script(probe: Probe, request: &Request) -> String {
    let mut s = format!(
        "cd -- {} 2>/dev/null || {{ echo '@no_folder'; exit 0; }}\n",
        shell_quote(&request.path)
    );
    s.push_str("export GIT_TERMINAL_PROMPT=0 GIT_OPTIONAL_LOCKS=0\n");
    match probe {
        Probe::Checkout => s.push_str(
            r#"if out=$(git rev-parse --is-inside-work-tree 2>&1 </dev/null) && [ "$out" = true ]; then
  echo '@checkout'
else
  printf '%s\n' "$out" | head -n 5 | sed 's/^/@message /'
fi
"#,
        ),
        Probe::Branch => s.push_str(
            r#"if b=$(git symbolic-ref --quiet --short HEAD 2>/dev/null </dev/null); then
  echo "@branch $b"
else
  echo '@detached'
fi
"#,
        ),
        Probe::Artisan => s.push_str(
            r#"[ -f artisan ] || { echo '@no_artisan'; exit 0; }
command -v php >/dev/null 2>&1 || { echo '@no_php'; exit 0; }
if command -v timeout >/dev/null 2>&1; then t='timeout 30'; else t=; fi
out=$($t php artisan --version --no-ansi 2>&1 </dev/null)
echo "@code $?"
printf '%s\n' "$out" | head -n 5 | sed 's/^/@message /'
"#,
        ),
        Probe::Log => {
            s.push_str(&format!("f={}\n", shell_quote(&request.log_path)));
            if request.log_daily {
                s.push_str(
                    r#"shopt -s nullglob
set -- "$f"-[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].log
[ $# -gt 0 ] || { echo "@no_log $f-YYYY-MM-DD.log"; exit 0; }
f=${!#}
"#,
                );
            }
            s.push_str(
                r#"if [ ! -f "$f" ]; then
  echo "@no_log $f"
elif { : < "$f"; } 2>/dev/null; then
  echo "@log $f"
else
  echo "@unreadable $f"
fi
"#,
            );
        }
        Probe::Alias | Probe::Connect => unreachable!("not a remote probe"),
    }
    s
}

fn facts(lines: &[String]) -> impl Iterator<Item = (&str, &str)> {
    lines.iter().filter_map(|line| {
        let line = line.strip_prefix('@')?;
        Some(line.split_once(' ').unwrap_or((line, "")))
    })
}

fn parse(probe: Probe, request: &Request, code: i32, lines: &[String]) -> Outcome {
    let messages = || {
        facts(lines)
            .filter(|(key, _)| *key == "message")
            .map(|(_, value)| value.to_string())
            .collect::<Vec<_>>()
    };
    let unexpected = || {
        Outcome::Fail(Failed::Unexpected {
            code,
            output: lines.iter().rev().take(5).rev().cloned().collect(),
        })
    };
    if facts(lines).any(|(key, _)| key == "no_folder") {
        return Outcome::Fail(Failed::NoFolder);
    }
    if code != 0 {
        return unexpected();
    }
    let fact = |name: &str| facts(lines).find(|(key, _)| *key == name).map(|(_, v)| v);
    match probe {
        Probe::Checkout => match fact("checkout") {
            Some(_) => Outcome::Pass(Passed::Checkout),
            None => Outcome::Fail(Failed::NotCheckout(messages())),
        },
        Probe::Branch => match (fact("branch"), fact("detached")) {
            (Some(branch), _) if branch == request.branch => Outcome::Pass(Passed::Branch),
            (Some(branch), _) => Outcome::Fail(Failed::OtherBranch(branch.to_string())),
            (None, Some(_)) => Outcome::Fail(Failed::Detached),
            (None, None) => unexpected(),
        },
        Probe::Artisan => {
            if fact("no_artisan").is_some() {
                return Outcome::Fail(Failed::NoArtisan);
            }
            if fact("no_php").is_some() {
                return Outcome::Fail(Failed::NoPhp);
            }
            match fact("code").and_then(|code| code.parse::<i32>().ok()) {
                Some(0) => Outcome::Pass(Passed::Artisan(
                    messages().into_iter().next().unwrap_or_default(),
                )),
                Some(code) => Outcome::Fail(Failed::Artisan {
                    code,
                    output: messages(),
                }),
                None => unexpected(),
            }
        }
        Probe::Log => {
            if let Some(file) = fact("log") {
                Outcome::Pass(Passed::Log(file.to_string()))
            } else if let Some(file) = fact("no_log") {
                Outcome::Fail(Failed::NoLog(file.to_string()))
            } else if let Some(file) = fact("unreadable") {
                Outcome::Fail(Failed::LogUnreadable(file.to_string()))
            } else {
                unexpected()
            }
        }
        Probe::Alias | Probe::Connect => unreachable!("not a remote probe"),
    }
}

#[cfg(test)]
mod tests;
