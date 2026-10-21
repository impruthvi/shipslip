//! What an interrupt (Ctrl-C, closing the window) means at each stage of a
//! followed run. Front-ends map an [`Interrupt`] to `ExecutionHandle` calls.

use crate::{DeployEvent, MaintenancePhase};

/// Where a followed run is, for deciding what an interrupt does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Running,
    BringingUp,
    Watching,
    Finishing,
}

pub fn next_stage(stage: Stage, event: &DeployEvent) -> Stage {
    match event {
        DeployEvent::MaintenanceStarted {
            phase: MaintenancePhase::Up,
        } => Stage::BringingUp,
        DeployEvent::MaintenanceFinished {
            phase: MaintenancePhase::Up,
            ..
        }
        | DeployEvent::WatchFinished(_) => Stage::Finishing,
        DeployEvent::WatchStarted { .. } => Stage::Watching,
        _ => stage,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Interrupt {
    /// Start no new step and stop following the running one.
    Detach,
    /// Maintenance up cannot be left half-done; skip the watch after it.
    SkipWatch,
    StopWatch,
    /// Only the lock release and receipt remain.
    Wait,
    Quit,
}

/// A second Ctrl-C in the same stage quits; a new stage starts over.
pub fn on_interrupt(stage: Stage, again: bool) -> Interrupt {
    if again {
        return Interrupt::Quit;
    }
    match stage {
        Stage::Running => Interrupt::Detach,
        Stage::BringingUp => Interrupt::SkipWatch,
        Stage::Watching => Interrupt::StopWatch,
        Stage::Finishing => Interrupt::Wait,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{StepStatus, WatchResult, WatchStatus};

    fn watch_finished(status: WatchStatus) -> DeployEvent {
        let mut result = WatchResult::not_run();
        result.status = status;
        DeployEvent::WatchFinished(result)
    }

    fn step_started() -> DeployEvent {
        DeployEvent::StepStarted {
            index: 1,
            name: "migrate".into(),
        }
    }

    #[test]
    fn ctrl_c_acts_on_the_current_stage_and_quits_when_repeated() {
        assert_eq!(on_interrupt(Stage::Running, false), Interrupt::Detach);
        assert_eq!(on_interrupt(Stage::BringingUp, false), Interrupt::SkipWatch);
        assert_eq!(on_interrupt(Stage::Watching, false), Interrupt::StopWatch);
        assert_eq!(on_interrupt(Stage::Finishing, false), Interrupt::Wait);
        for stage in [
            Stage::Running,
            Stage::BringingUp,
            Stage::Watching,
            Stage::Finishing,
        ] {
            assert_eq!(on_interrupt(stage, true), Interrupt::Quit);
        }
    }

    #[test]
    fn stages_follow_maintenance_up_and_the_log_watch() {
        let up = |phase| DeployEvent::MaintenanceStarted { phase };
        assert_eq!(
            next_stage(Stage::Running, &up(MaintenancePhase::Down)),
            Stage::Running
        );
        assert_eq!(next_stage(Stage::Running, &step_started()), Stage::Running);
        assert_eq!(
            next_stage(Stage::Running, &up(MaintenancePhase::Up)),
            Stage::BringingUp
        );
        assert_eq!(
            next_stage(
                Stage::BringingUp,
                &DeployEvent::MaintenanceFinished {
                    phase: MaintenancePhase::Up,
                    status: StepStatus::Ok,
                    exit_code: Some(0),
                }
            ),
            Stage::Finishing
        );
        assert_eq!(
            next_stage(
                Stage::Finishing,
                &DeployEvent::WatchStarted {
                    window: Duration::from_secs(120)
                }
            ),
            Stage::Watching
        );
        assert_eq!(
            next_stage(Stage::Watching, &watch_finished(WatchStatus::Complete)),
            Stage::Finishing
        );
    }
}
