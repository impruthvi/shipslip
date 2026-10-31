use serde::Serialize;

use super::Untrusted;
use crate::receipt::{Receipt, ReceiptKind, ReceiptStatus};
use crate::{DeployOutcome, RunPlan, SmokeResult, WatchStatus};

/// The dashboard summary of one run. It never reduces a run to a single
/// "healthy" state: checks that did not finish are flagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Badge {
    pub outcome: Outcome,
    pub flags: Vec<Flag>,
}

/// How a run ended, as far as its receipt says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Outcome {
    #[serde(flatten)]
    pub kind: OutcomeKind,
    /// The receipt is still in progress: the run is running, was
    /// interrupted, or crashed. `kind` is then only the last recorded one.
    pub unfinished: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutcomeKind {
    Succeeded,
    FailedAtStep {
        step: usize,
        partial_update: bool,
    },
    StoppedAfterStep {
        step: usize,
    },
    /// Stopped before any deploy step ran; maintenance mode may be on.
    StoppedInMaintenance,
    /// A step's result could not be read. Never success or failure.
    Unknown {
        step: usize,
    },
    CancelledBeforeChanges,
    AbortedBeforeChanges,
    /// The receipt holds no outcome.
    NotRecorded,
}

impl From<Option<&DeployOutcome>> for OutcomeKind {
    fn from(outcome: Option<&DeployOutcome>) -> Self {
        match outcome {
            None => Self::NotRecorded,
            Some(DeployOutcome::Succeeded) => Self::Succeeded,
            Some(DeployOutcome::FailedAtStep {
                step,
                partial_update,
            }) => Self::FailedAtStep {
                step: *step,
                partial_update: *partial_update,
            },
            Some(DeployOutcome::StoppedAfterStep { step, .. }) => {
                Self::StoppedAfterStep { step: *step }
            }
            Some(DeployOutcome::StoppedInMaintenance(_)) => Self::StoppedInMaintenance,
            Some(DeployOutcome::Unknown { step, .. }) => Self::Unknown { step: *step },
            Some(DeployOutcome::CancelledBeforeChanges) => Self::CancelledBeforeChanges,
            Some(DeployOutcome::AbortedBeforeChanges(_)) => Self::AbortedBeforeChanges,
        }
    }
}

impl OutcomeKind {
    /// A short label without reasons, e.g. `Failed at step 2`.
    pub fn label(self) -> Option<String> {
        Some(match self {
            Self::Succeeded => "Succeeded".into(),
            Self::FailedAtStep {
                step,
                partial_update: true,
            } => format!("Failed at step {step} (partial update)"),
            Self::FailedAtStep { step, .. } => format!("Failed at step {step}"),
            Self::StoppedAfterStep { step } => format!("Stopped after step {step}"),
            Self::CancelledBeforeChanges => "Cancelled before changes".into(),
            Self::AbortedBeforeChanges => "Aborted before changes".into(),
            Self::StoppedInMaintenance => "Stopped in maintenance mode, no steps ran".into(),
            Self::Unknown { step } => format!("Unknown at step {step}"),
            Self::NotRecorded => return None,
        })
    }
}

impl Outcome {
    /// e.g. `Succeeded` or `Unfinished (last recorded: Succeeded)`.
    pub fn headline(self) -> String {
        match (self.unfinished, self.kind.label()) {
            (false, Some(label)) => label,
            (false, None) => "Finished without an outcome".into(),
            (true, Some(label)) => format!("Unfinished (last recorded: {label})"),
            (true, None) => "Unfinished".into(),
        }
    }

    /// The run did not finish, or ended where the server may need a look.
    pub fn needs_attention(self) -> bool {
        self.unfinished
            || matches!(
                self.kind,
                OutcomeKind::FailedAtStep { .. }
                    | OutcomeKind::StoppedAfterStep { .. }
                    | OutcomeKind::StoppedInMaintenance
                    | OutcomeKind::Unknown { .. }
            )
    }

    /// The run finished and succeeded.
    pub fn succeeded(self) -> bool {
        !self.unfinished && self.kind == OutcomeKind::Succeeded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Flag {
    /// New error groups seen by the log watch.
    NewErrors {
        groups: u64,
    },
    LogNotFullyObserved,
    LogNotRecorded,
    SmokeFailed {
        status: Option<u16>,
    },
    SmokeNotRecorded,
    AppLeftDown,
}

impl Flag {
    /// Something may be wrong now: new errors, a failed smoke check, or the
    /// app left in maintenance mode. Other flags say what was not recorded.
    pub fn is_bad(self) -> bool {
        matches!(
            self,
            Self::NewErrors { .. } | Self::SmokeFailed { .. } | Self::AppLeftDown
        )
    }

    /// The flag for a terminal or a shared receipt.
    pub fn text(self) -> String {
        match self {
            Self::NewErrors { groups: 1 } => "⚠ 1 new error group".into(),
            Self::NewErrors { groups } => format!("⚠ {groups} new error groups"),
            Self::LogNotFullyObserved => "ⓘ log not fully observed".into(),
            Self::LogNotRecorded => "ⓘ log not recorded".into(),
            Self::SmokeFailed {
                status: Some(status),
            } => format!("⚠ smoke {status}"),
            Self::SmokeFailed { status: None } => "⚠ smoke failed".into(),
            Self::SmokeNotRecorded => "ⓘ smoke not recorded".into(),
            Self::AppLeftDown => "⛔ app may be in maintenance mode".into(),
        }
    }
}

impl Receipt {
    pub fn badge(&self) -> Badge {
        let mut flags = Vec::new();
        // Checks never apply to bringing the app up, or to a run that
        // stopped before changing anything.
        let checks_apply = self.kind == ReceiptKind::Deploy
            && !matches!(
                (self.status, &self.outcome),
                (
                    ReceiptStatus::Final,
                    Some(
                        DeployOutcome::CancelledBeforeChanges
                            | DeployOutcome::AbortedBeforeChanges(_)
                            | DeployOutcome::StoppedInMaintenance(_)
                    )
                )
            );
        match &self.watch {
            None if checks_apply => flags.push(Flag::LogNotRecorded),
            None => {}
            Some(watch) => {
                let groups = self.new_error_groups();
                if groups > 0 {
                    flags.push(Flag::NewErrors { groups });
                }
                match watch.status {
                    WatchStatus::Complete | WatchStatus::NotRun => {}
                    WatchStatus::Partial
                    | WatchStatus::Unavailable
                    | WatchStatus::NoLogSeen
                    | WatchStatus::Cancelled => flags.push(Flag::LogNotFullyObserved),
                }
            }
        }
        match &self.smoke {
            None if checks_apply => flags.push(Flag::SmokeNotRecorded),
            None => {}
            Some(SmokeResult::Failed { status, .. }) => {
                flags.push(Flag::SmokeFailed { status: *status })
            }
            Some(
                SmokeResult::Passed { .. } | SmokeResult::NotConfigured | SmokeResult::Skipped,
            ) => {}
        }
        if self.app_left_down {
            flags.push(Flag::AppLeftDown);
        }
        Badge {
            outcome: Outcome {
                kind: self.outcome.as_ref().into(),
                unfinished: self.status == ReceiptStatus::InProgress,
            },
            flags,
        }
    }

    /// New error groups the log watch saw, including those not listed.
    pub(super) fn new_error_groups(&self) -> u64 {
        self.watch.as_ref().map_or(0, |watch| {
            watch.new_errors.len() as u64 + watch.overflow_groups
        })
    }

    /// Who started the run, e.g. `Ana <ana@example.com> (local user ana)`.
    pub fn started_by(&self) -> Option<Untrusted> {
        let actor = self.actor.as_ref()?;
        let git = match (&actor.git_name, &actor.git_email) {
            (Some(name), Some(email)) => Some(format!("{name} <{email}>")),
            (Some(value), None) | (None, Some(value)) => Some(value.clone()),
            (None, None) => None,
        };
        Some(Untrusted::new(match (git, &actor.user) {
            (Some(git), Some(user)) => format!("{git} (local user {user})"),
            (Some(git), None) => git,
            (None, Some(user)) => format!("local user {user}"),
            (None, None) => return None,
        }))
    }

    /// Who started the run without contact details: the Git name, or the
    /// local user when no name was recorded.
    pub fn started_by_name(&self) -> Option<Untrusted> {
        let actor = self.actor.as_ref()?;
        actor
            .git_name
            .clone()
            .or_else(|| actor.user.clone())
            .map(Untrusted::new)
    }

    /// What the run was for, e.g. `deploy` or `rerun on the deployed commit`.
    pub fn plan_text(&self) -> String {
        match self.kind {
            ReceiptKind::Deploy => describe_plan(self.run_plan),
            ReceiptKind::Up => "bring the app up (`php artisan up`)".into(),
        }
    }

    /// How the run ended, in one sentence with any recorded reason.
    pub fn summary(&self) -> Option<Untrusted> {
        let outcome = self.outcome.as_ref()?;
        if self.kind == ReceiptKind::Deploy {
            return Some(outcome.summary().into());
        }
        Some(Untrusted::new(match outcome {
            DeployOutcome::Succeeded => {
                "The app was brought up (`php artisan up` succeeded).".into()
            }
            DeployOutcome::FailedAtStep { .. } => format!(
                "`php artisan up` exited with {}; the app may still be in maintenance mode.",
                self.maintenance_up_exit_code
                    .map_or_else(|| "an error".to_string(), |code| code.to_string())
            ),
            DeployOutcome::StoppedInMaintenance(reason) => format!(
                "`php artisan up` did not run ({reason}); the app may still be in maintenance mode."
            ),
            DeployOutcome::Unknown { reason, .. } => {
                format!("The result of `php artisan up` is unknown: {reason}.")
            }
            outcome => outcome.summary(),
        }))
    }
}

pub fn describe_plan(plan: RunPlan) -> String {
    match plan {
        RunPlan::Deploy => "deploy".into(),
        RunPlan::Rerun => "rerun on the deployed commit".into(),
        RunPlan::FromStep(step) => format!("from step {step} on the deployed commit"),
    }
}
