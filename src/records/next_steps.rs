use serde::Serialize;

use super::Untrusted;
use crate::logs::escape_field;
use crate::receipt::{Receipt, ReceiptKind, ReceiptStatus};
use crate::DeployOutcome;

/// A receipt's recovery hints, built only from what it recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NextSteps {
    /// Whether the commands may be offered for copying. Nothing
    /// interpolated into them can have changed their words.
    pub copy_safe: bool,
    pub steps: Vec<NextStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct NextStep(pub Vec<Part>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Text(Untrusted),
    /// A `slip` command as its argument list, starting with `slip`.
    Command(Vec<Untrusted>),
}

impl NextStep {
    /// The step names `slip <subcommand>`.
    pub fn runs(&self, subcommand: &str) -> bool {
        self.0.iter().any(|part| match part {
            Part::Command(args) => args.get(1).map(Untrusted::raw) == Some(subcommand),
            Part::Text(_) => false,
        })
    }

    fn text(text: impl Into<String>) -> Self {
        Self(vec![Part::Text(Untrusted::new(text))])
    }

    /// `before`, the command, then `after`.
    fn command(before: impl Into<String>, args: &[&str], after: &str) -> Self {
        let mut command = vec![Untrusted::new("slip")];
        command.extend(args.iter().map(|arg| Untrusted::new(*arg)));
        Self(vec![
            Part::Text(Untrusted::new(before)),
            Part::Command(command),
            Part::Text(Untrusted::new(after)),
        ])
    }
}

/// A command built from `env`, run in `repo_root`, can be copied: the env
/// name is one plain word and the checkout path has no backtick or control
/// character.
pub fn copy_is_safe(env: &str, repo_root: &str) -> bool {
    let plain_word = !env.is_empty()
        && env
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    plain_word && !repo_root.contains('`') && escape_field(repo_root) == repo_root
}

/// Retrying the same commit only helps when the cause was outside the code.
pub fn failed_step_options(env: &str, step: usize) -> [NextStep; 2] {
    [
        NextStep::command(
            "If the cause was on the server (permissions, .env, database), fix it there, then ",
            &["from-step", env, &step.to_string()],
            " runs the remaining steps on the deployed commit.",
        ),
        NextStep::command(
            "If it needs a code change, push the fix and run ",
            &["deploy", env],
            ".",
        ),
    ]
}

impl Receipt {
    pub fn next_steps(&self) -> NextSteps {
        let env = self.target.env.as_str();
        let mut steps = Vec::new();
        if self.status == ReceiptStatus::InProgress {
            steps.push(NextStep::command(
                format!(
                    "The run did not finish. If no deploy is running, resume it from {} with ",
                    self.repo_root
                ),
                &["attach", env],
                ".",
            ));
        }
        if self.kind == ReceiptKind::Up {
            if let Some(DeployOutcome::Unknown { .. }) = &self.outcome {
                steps.push(NextStep::text(
                    "Check the server before running anything again; the deploy lock was kept.",
                ));
            } else if self.app_left_down && self.status == ReceiptStatus::Final {
                steps.push(NextStep::command(
                    "The app may still be in maintenance mode. After checking the server, ",
                    &["up", env],
                    " tries again.",
                ));
            }
        } else {
            self.deploy_next_steps(env, &mut steps);
        }
        NextSteps {
            copy_safe: copy_is_safe(env, &self.repo_root),
            steps,
        }
    }

    fn deploy_next_steps(&self, env: &str, steps: &mut Vec<NextStep>) {
        match &self.outcome {
            Some(DeployOutcome::FailedAtStep { step: 0, .. }) => steps.push(NextStep::text(
                "The fast-forward failed. Check the server checkout before deploying again.",
            )),
            Some(DeployOutcome::FailedAtStep { step, .. }) => {
                steps.push(NextStep::text(format!(
                    "Step {step} failed and later steps did not run."
                )));
                steps.extend(failed_step_options(env, *step));
            }
            Some(DeployOutcome::StoppedAfterStep { step, .. })
                if *step < self.target.steps.len() =>
            {
                steps.push(NextStep::command(
                    format!("Steps after {step} did not run. "),
                    &["from-step", env, &(step + 1).to_string()],
                    " runs them on the deployed commit.",
                ))
            }
            Some(DeployOutcome::StoppedInMaintenance(_)) => steps.push(NextStep::text(
                "No deploy steps ran, so the code on the server did not change.",
            )),
            Some(DeployOutcome::Unknown { step, .. }) => steps.push(NextStep::text(format!(
                "The result of step {step} is unknown. Check the server before running anything again."
            ))),
            _ => {}
        }
        let groups = self.new_error_groups();
        if groups > 0 {
            let noun = if groups == 1 { "group" } else { "groups" };
            steps.push(NextStep::command(
                format!("The log showed {groups} new error {noun} after this run. "),
                &["logs", env],
                " shows the log.",
            ));
        }
        if self.app_left_down {
            steps.push(NextStep::command(
                "The app may still be in maintenance mode. After checking the server, ",
                &["up", env],
                " turns it off.",
            ));
        }
    }
}
