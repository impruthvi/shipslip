//! The connection check's lines as the sheet and the checkout menu show
//! them. Remote text is server text: every reason built from it is escaped.

use std::path::Path;

use serde::Serialize;
use shipslip::config::LoadedConfig;
use shipslip::connection_check::{CheckEvent, Failed, Outcome, Passed, Probe, Request};
use shipslip::logs::escape_field;
use shipslip::transport::ConnectFailure;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckState {
    Waiting,
    Running,
    Pass,
    Fail,
    NotRun,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckLine {
    pub label: String,
    pub state: CheckState,
    pub reason: String,
    /// A host key refused for a reason other than "unknown": no command, and
    /// the sheet cannot continue.
    pub hard_stop: Option<HostKeyStop>,
    /// An unknown host key: `ssh <alias> true`, to run in Terminal.
    pub command: Option<String>,
}

/// The check as it stands: sent after each probe starts or ends.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckView {
    pub alias: String,
    pub lines: Vec<CheckLine>,
    /// The first line that is a hard stop, if any.
    pub host_key_stop: Option<HostKeyStop>,
    /// Every line has its outcome.
    pub finished: bool,
}

/// Why the check stopped at the host key. Neither gets a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HostKeyStop {
    Changed,
    Unverified,
}

pub struct Lines {
    alias: String,
    branch: String,
    path: String,
    lines: Vec<CheckLine>,
}

impl Lines {
    pub fn new(request: &Request) -> Self {
        let alias = escape_field(&request.alias);
        let branch = escape_field(&request.branch);
        let lines = Probe::ALL
            .iter()
            .map(|&probe| CheckLine {
                label: match probe {
                    Probe::Alias => format!("Alias {alias} resolves"),
                    Probe::Connect => "ssh connects".into(),
                    Probe::Checkout => "Remote path is a git checkout".into(),
                    Probe::Branch => format!("On branch {branch}"),
                    Probe::Artisan => "php artisan --version runs".into(),
                    Probe::Log => "Laravel log readable".into(),
                },
                state: CheckState::Waiting,
                reason: String::new(),
                hard_stop: None,
                command: None,
            })
            .collect();
        Self {
            alias,
            branch,
            path: escape_field(&request.path),
            lines,
        }
    }

    pub fn apply(&mut self, event: &CheckEvent) {
        let (probe, state, reason, command, hard_stop) = match event {
            CheckEvent::Started(probe) => (
                *probe,
                CheckState::Running,
                "Checking…".to_string(),
                None,
                None,
            ),
            CheckEvent::Finished(probe, Outcome::NotRun) => {
                (*probe, CheckState::NotRun, "Not run".into(), None, None)
            }
            CheckEvent::Finished(probe, Outcome::Pass(passed)) => {
                (*probe, CheckState::Pass, self.passed(passed), None, None)
            }
            CheckEvent::Finished(probe, Outcome::Fail(failed)) => {
                let (reason, command, hard_stop) = self.failed(failed);
                (*probe, CheckState::Fail, reason, command, hard_stop)
            }
        };
        let index = Probe::ALL.iter().position(|p| *p == probe).unwrap();
        let line = &mut self.lines[index];
        line.state = state;
        line.reason = reason;
        line.command = command;
        line.hard_stop = hard_stop;
    }

    pub fn view(&self) -> CheckView {
        CheckView {
            alias: self.alias.clone(),
            lines: self.lines.clone(),
            host_key_stop: self.lines.iter().find_map(|line| line.hard_stop),
            finished: self
                .lines
                .iter()
                .all(|line| !matches!(line.state, CheckState::Waiting | CheckState::Running)),
        }
    }

    fn passed(&self, passed: &Passed) -> String {
        match passed {
            Passed::Alias(resolved) => {
                let port = if resolved.port == 22 {
                    String::new()
                } else {
                    format!(":{}", resolved.port)
                };
                format!(
                    "{} → {}{port} as {} (read with ssh -G)",
                    self.alias,
                    escape_field(&resolved.host_name),
                    escape_field(&resolved.user)
                )
            }
            Passed::Connected => "Connected with the host key this Mac knows".into(),
            Passed::Checkout => self.path.clone(),
            Passed::Branch => format!("{} is checked out", self.branch),
            Passed::Artisan(version) if version.is_empty() => "Exited 0".into(),
            Passed::Artisan(version) => escape_field(version),
            Passed::Log(file) => format!("Reads {}", escape_field(file)),
        }
    }

    fn failed(&self, failed: &Failed) -> (String, Option<String>, Option<HostKeyStop>) {
        let alias = &self.alias;
        let reason = match failed {
            Failed::Alias(message) => format!("ssh -G could not read it: {}", last_line(message)),
            Failed::Connect { failure, message } => match failure {
                ConnectFailure::HostKeyUnknown => {
                    return (
                        format!(
                            "This Mac has not seen the host key for {alias}. Run the command in Terminal, \
                             compare the fingerprint, answer yes, then check again."
                        ),
                        Some(format!("ssh {alias} true")),
                        None,
                    )
                }
                ConnectFailure::HostKeyChanged => {
                    return ("Host key changed".into(), None, Some(HostKeyStop::Changed))
                }
                ConnectFailure::HostKeyUnverified => {
                    return (
                        "Can't verify the host key".into(),
                        None,
                        Some(HostKeyStop::Unverified),
                    )
                }
                ConnectFailure::KeyNotLoaded => format!(
                    "The server turned down this Mac's keys. Load the key with ssh-add, or check \
                     the IdentityFile for {alias}."
                ),
                ConnectFailure::Other => format!("Couldn't connect: {}", last_line(message)),
            },
            Failed::Lost(message) => format!(
                "The connection dropped: {}. Nothing was retried.",
                last_line(message)
            ),
            Failed::NoFolder => format!("No folder at {} for this login", self.path),
            Failed::NotCheckout(messages) => match messages.first() {
                Some(message) => format!("Not a git checkout: {}", escape_field(message)),
                None => "Not a git checkout".into(),
            },
            Failed::OtherBranch(branch) => {
                format!("{} is checked out, not {}", escape_field(branch), self.branch)
            }
            Failed::Detached => format!("HEAD is detached, not on {}", self.branch),
            Failed::NoArtisan => format!("No artisan file in {}", self.path),
            Failed::NoPhp => "php is not on the PATH for this login".into(),
            Failed::Artisan { code, output } => match output.first() {
                Some(line) => format!("Exited {code}: {}", escape_field(line)),
                None => format!("Exited {code}"),
            },
            Failed::NoLog(file) => format!(
                "No file at {} yet. Laravel creates it with its first log entry.",
                escape_field(file)
            ),
            Failed::LogUnreadable(file) => {
                format!("This login can't read {}", escape_field(file))
            }
            Failed::Unexpected { code, output } => match output.last() {
                Some(line) => format!("Ended with exit {code}: {}", escape_field(line)),
                None => format!("Ended with exit {code}"),
            },
        };
        (reason, None, None)
    }
}

fn last_line(message: &str) -> String {
    escape_field(
        message
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim(),
    )
}

/// The check for one environment as `.shipslip.toml` reads now, approved or
/// not: the check never needs an approval.
pub fn request_for(checkout: &Path, env: &str) -> Result<Request, String> {
    let config = LoadedConfig::load(checkout, None).map_err(|error| error.to_string())?;
    let target = config
        .target(env)
        .ok_or_else(|| format!("{env} is no longer in .shipslip.toml."))?;
    Ok(Request::for_target(&target))
}

/// The check for the answers just given: a new config logs to Laravel's
/// default file.
pub fn request_for_answers(answers: &shipslip::config::InitAnswers) -> Request {
    Request {
        alias: answers.ssh_alias.clone(),
        path: answers.path.clone(),
        branch: answers.branch.clone(),
        log_path: "storage/logs/laravel.log".into(),
        log_daily: false,
    }
}

/// The views a check's events produce, for the UI fixtures.
#[cfg(test)]
pub fn replay(request: &Request, events: &[CheckEvent]) -> CheckView {
    let mut lines = Lines::new(request);
    for event in events {
        lines.apply(event);
    }
    lines.view()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shipslip::ssh_aliases::Resolved;

    fn request() -> Request {
        Request {
            alias: "web".into(),
            path: "/var/www/app".into(),
            branch: "main".into(),
            log_path: "storage/logs/laravel.log".into(),
            log_daily: false,
        }
    }

    fn finished(probe: Probe, outcome: Outcome) -> CheckEvent {
        CheckEvent::Finished(probe, outcome)
    }

    fn connect_refused(failure: ConnectFailure) -> CheckView {
        let mut events = vec![
            finished(
                Probe::Alias,
                Outcome::Pass(Passed::Alias(Resolved {
                    host_name: "203.0.113.7".into(),
                    user: "forge".into(),
                    port: 2222,
                })),
            ),
            finished(
                Probe::Connect,
                Outcome::Fail(Failed::Connect {
                    failure,
                    message: "Host key verification failed.".into(),
                }),
            ),
        ];
        events.extend(
            Probe::ALL[2..]
                .iter()
                .map(|&p| finished(p, Outcome::NotRun)),
        );
        replay(&request(), &events)
    }

    #[test]
    fn an_unknown_host_key_offers_the_command_to_compare_it() {
        let view = connect_refused(ConnectFailure::HostKeyUnknown);
        assert_eq!(
            view.lines[0].reason,
            "web → 203.0.113.7:2222 as forge (read with ssh -G)"
        );
        assert_eq!(view.lines[1].state, CheckState::Fail);
        assert_eq!(view.lines[1].command.as_deref(), Some("ssh web true"));
        assert_eq!(view.host_key_stop, None);
        assert!(view.finished);
        assert!(view.lines[2..]
            .iter()
            .all(|line| line.state == CheckState::NotRun && line.reason == "Not run"));
    }

    #[test]
    fn changed_and_unverified_host_keys_are_hard_stops_with_no_command() {
        for (failure, stop, reason) in [
            (
                ConnectFailure::HostKeyChanged,
                HostKeyStop::Changed,
                "Host key changed",
            ),
            (
                ConnectFailure::HostKeyUnverified,
                HostKeyStop::Unverified,
                "Can't verify the host key",
            ),
        ] {
            let view = connect_refused(failure);
            assert_eq!(view.host_key_stop, Some(stop));
            assert_eq!(view.lines[1].hard_stop, Some(stop));
            assert_eq!(view.lines[1].reason, reason);
            assert!(view.lines.iter().all(|line| line.command.is_none()));
        }
    }

    #[test]
    fn lines_wait_then_run_then_end() {
        let mut lines = Lines::new(&request());
        assert!(lines
            .view()
            .lines
            .iter()
            .all(|line| line.state == CheckState::Waiting));
        lines.apply(&CheckEvent::Started(Probe::Alias));
        let view = lines.view();
        assert_eq!(view.lines[0].state, CheckState::Running);
        assert!(!view.finished);
        assert_eq!(
            view.lines
                .iter()
                .map(|l| l.label.as_str())
                .collect::<Vec<_>>(),
            [
                "Alias web resolves",
                "ssh connects",
                "Remote path is a git checkout",
                "On branch main",
                "php artisan --version runs",
                "Laravel log readable",
            ]
        );
    }

    #[test]
    fn each_failure_says_what_to_fix() {
        let cases = [
            (
                Probe::Alias,
                Failed::Alias("ssh -G failed: line 4: Bad configuration option".into()),
                "ssh -G could not read it: ssh -G failed: line 4: Bad configuration option",
            ),
            (
                Probe::Connect,
                Failed::Connect {
                    failure: ConnectFailure::KeyNotLoaded,
                    message: "Permission denied (publickey).".into(),
                },
                "The server turned down this Mac's keys. Load the key with ssh-add, or check the IdentityFile for web.",
            ),
            (
                Probe::Connect,
                Failed::Connect {
                    failure: ConnectFailure::Other,
                    message: "debug\nssh: connect to host x port 22: Connection refused\n".into(),
                },
                "Couldn't connect: ssh: connect to host x port 22: Connection refused",
            ),
            (
                Probe::Checkout,
                Failed::Lost("broken pipe".into()),
                "The connection dropped: broken pipe. Nothing was retried.",
            ),
            (
                Probe::Checkout,
                Failed::NoFolder,
                "No folder at /var/www/app for this login",
            ),
            (
                Probe::Checkout,
                Failed::NotCheckout(vec!["fatal: not a git repository".into()]),
                "Not a git checkout: fatal: not a git repository",
            ),
            (Probe::Checkout, Failed::NotCheckout(Vec::new()), "Not a git checkout"),
            (Probe::Branch, Failed::Detached, "HEAD is detached, not on main"),
            (Probe::Artisan, Failed::NoArtisan, "No artisan file in /var/www/app"),
            (Probe::Artisan, Failed::NoPhp, "php is not on the PATH for this login"),
            (
                Probe::Artisan,
                Failed::Artisan { code: 1, output: Vec::new() },
                "Exited 1",
            ),
            (
                Probe::Log,
                Failed::NoLog("storage/logs/laravel.log".into()),
                "No file at storage/logs/laravel.log yet. Laravel creates it with its first log entry.",
            ),
            (
                Probe::Log,
                Failed::LogUnreadable("storage/logs/laravel.log".into()),
                "This login can't read storage/logs/laravel.log",
            ),
            (
                Probe::Log,
                Failed::Unexpected { code: 3, output: vec!["one".into(), "last".into()] },
                "Ended with exit 3: last",
            ),
            (
                Probe::Log,
                Failed::Unexpected { code: 3, output: Vec::new() },
                "Ended with exit 3",
            ),
        ];
        for (probe, failed, reason) in cases {
            let view = replay(&request(), &[finished(probe, Outcome::Fail(failed))]);
            let line = &view.lines[Probe::ALL.iter().position(|p| *p == probe).unwrap()];
            assert_eq!(line.state, CheckState::Fail);
            assert_eq!(line.reason, reason);
            assert_eq!(line.command, None);
            assert_eq!(line.hard_stop, None);
        }
    }

    #[test]
    fn the_check_reads_the_environment_from_the_config_as_it_is_now() {
        let fx = crate::fixture::Fixture::new();
        let root = fx.checkout(
            "crm",
            "[project]\nname = \"crm\"\nstack = \"laravel\"\n[env.staging]\nssh = \"web1\"\npath = \"/srv/crm\"\nbranch = \"main\"\nproduction = false\nlog = \"storage/logs/app\"\nlog_daily = true\n",
        );
        let request = request_for(&root, "staging").unwrap();
        assert_eq!(
            request,
            Request {
                alias: "web1".into(),
                path: "/srv/crm".into(),
                branch: "main".into(),
                log_path: "storage/logs/app".into(),
                log_daily: true,
            }
        );
        assert_eq!(
            request_for(&root, "production").unwrap_err(),
            "production is no longer in .shipslip.toml."
        );
        std::fs::write(root.join(".shipslip.toml"), "not toml [").unwrap();
        assert!(request_for(&root, "staging").is_err());
    }

    #[test]
    fn server_text_in_a_reason_is_escaped() {
        let view = replay(
            &request(),
            &[
                finished(
                    Probe::Branch,
                    Outcome::Fail(Failed::OtherBranch("x\u{1b}[31mred\nnext".into())),
                ),
                finished(
                    Probe::Artisan,
                    Outcome::Fail(Failed::Artisan {
                        code: 1,
                        output: vec!["<b>\u{7}bell".into()],
                    }),
                ),
            ],
        );
        for line in &view.lines[3..5] {
            assert!(
                !line.reason.chars().any(|c| c.is_control()),
                "{:?}",
                line.reason
            );
        }
        assert!(view.lines[3].reason.ends_with("is checked out, not main"));
    }
}
