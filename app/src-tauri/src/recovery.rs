//! What an env's newest run left to do: one sentence and the actions that
//! can follow it, recommended first. Built only from the saved receipt.

use serde::Serialize;
use shipslip::logs::escape_field;
use shipslip::receipt::{Receipt, ReceiptKind, ReceiptStatus, ReceiptStepStatus};
use shipslip::DeployOutcome;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RecoveryOp {
    FromStep { step: usize },
    Attach,
    Up,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryAction {
    #[serde(flatten)]
    pub op: RecoveryOp,
    /// The CLI command that does the same, shown under the button.
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryView {
    pub run_id: String,
    pub sentence: String,
    /// What to do first when nothing is safe to run yet.
    pub advice: Option<String>,
    pub actions: Vec<RecoveryAction>,
}

const CHECK_FIRST: &str = "Check the server before running anything.";

/// What the checkout's runs for an env left to do, newest first. A bring-up
/// that succeeded turns maintenance off but leaves the deploy before it
/// unfinished, so that deploy's actions stay, without bringing the app up.
pub fn recovery_from(runs: &[Receipt]) -> Option<RecoveryView> {
    let (newest, older) = runs.split_first()?;
    let view = recovery(newest);
    if view.is_some() || newest.kind != ReceiptKind::Up {
        return view;
    }
    let deploy = older.iter().find(|run| run.kind != ReceiptKind::Up)?;
    let mut view = recovery(deploy)?;
    view.actions.retain(|action| action.op != RecoveryOp::Up);
    (!view.actions.is_empty() || view.advice.is_some()).then_some(view)
}

/// `None` when the run left nothing to recover.
pub fn recovery(receipt: &Receipt) -> Option<RecoveryView> {
    let env = escape_field(&receipt.target.env);
    let action = |op: RecoveryOp| {
        let command = match &op {
            RecoveryOp::FromStep { step } => format!("slip from-step {env} {step}"),
            RecoveryOp::Attach => format!("slip attach {env}"),
            RecoveryOp::Up => format!("slip up {env}"),
        };
        RecoveryAction { op, command }
    };
    let view = |sentence: String, advice: Option<&str>, actions: Vec<RecoveryAction>| {
        Some(RecoveryView {
            run_id: escape_field(&receipt.run_id),
            sentence,
            advice: advice.map(Into::into),
            actions,
        })
    };
    let lost = |step: Option<usize>| match step {
        Some(step) => {
            format!(
                "Lost the connection during step {step}. It may still be running on the server."
            )
        }
        None => "The run did not finish. It may still be running on the server.".into(),
    };

    if receipt.status == ReceiptStatus::InProgress {
        let step = receipt
            .steps
            .iter()
            .find(|s| {
                matches!(
                    s.status,
                    ReceiptStepStatus::Running | ReceiptStepStatus::Unknown
                )
            })
            .map(|s| s.index);
        return view(lost(step), None, vec![action(RecoveryOp::Attach)]);
    }

    let mut actions = Vec::new();
    if receipt.app_left_down {
        actions.push(action(RecoveryOp::Up));
    }
    let (sentence, advice) = match (&receipt.kind, &receipt.outcome) {
        (ReceiptKind::Up, Some(DeployOutcome::Unknown { .. })) => (
            "The result of php artisan up is unknown.".to_string(),
            Some(CHECK_FIRST),
        ),
        (ReceiptKind::Up, _) => ("The app may still be in maintenance mode.".into(), None),
        (_, Some(DeployOutcome::FailedAtStep { step: 0, .. })) => (
            "The fast-forward failed.".into(),
            Some("Check the server checkout before deploying again."),
        ),
        (_, Some(DeployOutcome::FailedAtStep { step, .. })) => {
            let failed = receipt.steps.iter().find(|s| s.index == *step);
            let exit = failed
                .and_then(|s| s.exit_code)
                .map(|code| format!(" (exit {code})"))
                .unwrap_or_default();
            let command = failed
                .map(|s| format!(": {}", escape_field(&s.command)))
                .unwrap_or_default();
            actions.push(action(RecoveryOp::FromStep { step: *step }));
            (format!("Step {step} failed{exit}{command}"), None)
        }
        (_, Some(DeployOutcome::StoppedAfterStep { step, .. }))
            if *step < receipt.target.steps.len() =>
        {
            let last = receipt.target.steps.len();
            let rest = if step + 1 == last {
                format!("Step {last} did not run.")
            } else {
                format!("Steps {}–{last} did not run.", step + 1)
            };
            actions.push(action(RecoveryOp::FromStep { step: step + 1 }));
            (format!("Stopped after step {step}. {rest}"), None)
        }
        (_, Some(DeployOutcome::Unknown { step, .. })) => (lost(Some(*step)), Some(CHECK_FIRST)),
        (_, Some(DeployOutcome::StoppedInMaintenance(_))) => (
            "No deploy steps ran, so the code on the server did not change.".into(),
            None,
        ),
        _ if receipt.app_left_down => ("The app may still be in maintenance mode.".into(), None),
        _ => return None,
    };
    if receipt.kind == ReceiptKind::Up && actions.is_empty() && advice.is_none() {
        return None;
    }
    view(sentence, advice, actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// A production deploy with recipe steps 1–3, every step done.
    fn receipt(outcome: Value, edit: impl FnOnce(&mut Value)) -> Receipt {
        let mut value = crate::fixture::run(1, "production");
        value["target"]["steps"] = json!([
            "composer install",
            "php artisan migrate",
            "php artisan optimize"
        ]);
        value["steps"] = json!((0..=3)
            .map(|index| json!({"index": index, "command": format!("step {index}"), "status": "Ok", "exit_code": 0, "output": []}))
            .collect::<Vec<_>>());
        value["outcome"] = outcome;
        edit(&mut value);
        serde_json::from_value(value).unwrap()
    }

    fn ops(view: &RecoveryView) -> Vec<RecoveryOp> {
        view.actions.iter().map(|a| a.op.clone()).collect()
    }

    #[test]
    fn a_failed_step_recommends_running_from_it() {
        let view = recovery(&receipt(
            json!({"FailedAtStep": {"step": 2, "partial_update": false}}),
            |v| {
                v["steps"][2]["status"] = json!("Failed");
                v["steps"][2]["exit_code"] = json!(1);
            },
        ))
        .unwrap();
        assert!(
            view.sentence.starts_with("Step 2 failed (exit 1): "),
            "{}",
            view.sentence
        );
        assert_eq!(ops(&view), [RecoveryOp::FromStep { step: 2 }]);
        assert_eq!(view.actions[0].command, "slip from-step production 2");
    }

    #[test]
    fn stopped_runs_continue_after_the_last_step_that_ran() {
        let view = recovery(&receipt(
            json!({"StoppedAfterStep": {"step": 1, "reason": "Requested"}}),
            |_| {},
        ))
        .unwrap();
        assert_eq!(
            view.sentence,
            "Stopped after step 1. Steps 2–3 did not run."
        );
        assert_eq!(ops(&view), [RecoveryOp::FromStep { step: 2 }]);

        let last = |v: &mut Value| v["target"]["steps"] = json!(["a", "b"]);
        let view = recovery(&receipt(
            json!({"StoppedAfterStep": {"step": 1, "reason": "Requested"}}),
            last,
        ))
        .unwrap();
        assert_eq!(view.sentence, "Stopped after step 1. Step 2 did not run.");

        // Stopping after the last step leaves nothing to run.
        assert_eq!(
            recovery(&receipt(
                json!({"StoppedAfterStep": {"step": 3, "reason": "Requested"}}),
                |_| {}
            )),
            None
        );
    }

    #[test]
    fn unknown_results_offer_attach_only_while_the_receipt_is_open() {
        let open = recovery(&receipt(Value::Null, |v| {
            v["status"] = json!("InProgress");
            v["steps"][3]["status"] = json!("Running");
        }))
        .unwrap();
        assert_eq!(
            open.sentence,
            "Lost the connection during step 3. It may still be running on the server."
        );
        assert_eq!(ops(&open), [RecoveryOp::Attach]);

        let finished = recovery(&receipt(
            json!({"Unknown": {"step": 3, "reason": "connection lost"}}),
            |_| {},
        ))
        .unwrap();
        assert_eq!(finished.sentence, open.sentence);
        assert_eq!(finished.advice.as_deref(), Some(CHECK_FIRST));
        assert!(finished.actions.is_empty());
    }

    #[test]
    fn an_app_left_down_puts_bringing_it_up_first() {
        let view = recovery(&receipt(
            json!({"FailedAtStep": {"step": 2, "partial_update": false}}),
            |v| v["app_left_down"] = json!(true),
        ))
        .unwrap();
        assert_eq!(
            ops(&view),
            [RecoveryOp::Up, RecoveryOp::FromStep { step: 2 }]
        );
        assert_eq!(view.actions[0].command, "slip up production");

        let succeeded = recovery(&receipt(json!("Succeeded"), |v| {
            v["app_left_down"] = json!(true)
        }))
        .unwrap();
        assert_eq!(ops(&succeeded), [RecoveryOp::Up]);
    }

    #[test]
    fn clean_runs_and_fast_forward_failures_offer_no_action() {
        assert_eq!(recovery(&receipt(json!("Succeeded"), |_| {})), None);
        assert_eq!(
            recovery(&receipt(json!("CancelledBeforeChanges"), |_| {})),
            None
        );
        let ff = recovery(&receipt(
            json!({"FailedAtStep": {"step": 0, "partial_update": true}}),
            |_| {},
        ))
        .unwrap();
        assert!(ff.actions.is_empty());
        assert!(ff.advice.is_some());
    }

    fn up(outcome: Value) -> Receipt {
        receipt(outcome, |v| {
            v["kind"] = json!("up");
            v["target"]["steps"] = json!([]);
            v["steps"] = json!([]);
        })
    }

    #[test]
    fn a_successful_bring_up_keeps_the_failed_deploy_before_it() {
        let failed = receipt(
            json!({"FailedAtStep": {"step": 2, "partial_update": false}}),
            |v| v["app_left_down"] = json!(true),
        );
        let view = recovery_from(&[up(json!("Succeeded")), failed.clone()]).unwrap();
        assert!(
            view.sentence.starts_with("Step 2 failed"),
            "{}",
            view.sentence
        );
        assert_eq!(ops(&view), [RecoveryOp::FromStep { step: 2 }]);

        // Several bring-ups in a row still reach the deploy.
        let twice = recovery_from(&[up(json!("Succeeded")), up(json!("Succeeded")), failed]);
        assert_eq!(ops(&twice.unwrap()), [RecoveryOp::FromStep { step: 2 }]);
    }

    #[test]
    fn a_bring_up_after_a_deploy_that_only_left_the_app_down_leaves_nothing() {
        let stopped = receipt(json!({"StoppedInMaintenance": "Cancelled"}), |v| {
            v["app_left_down"] = json!(true)
        });
        assert_eq!(recovery_from(&[up(json!("Succeeded")), stopped]), None);
        let clean = receipt(json!("Succeeded"), |_| {});
        assert_eq!(recovery_from(&[up(json!("Succeeded")), clean]), None);
        assert_eq!(recovery_from(&[up(json!("Succeeded"))]), None);
        assert_eq!(recovery_from(&[]), None);
    }

    #[test]
    fn a_bring_up_that_did_not_finish_speaks_for_itself() {
        let failed = receipt(
            json!({"FailedAtStep": {"step": 2, "partial_update": false}}),
            |_| {},
        );
        let view = recovery_from(&[
            up(json!({"Unknown": {"step": 0, "reason": "connection lost"}})),
            failed,
        ])
        .unwrap();
        assert_eq!(view.sentence, "The result of php artisan up is unknown.");
    }

    #[test]
    fn untrusted_receipt_text_is_escaped() {
        let view = recovery(&receipt(
            json!({"FailedAtStep": {"step": 1, "partial_update": false}}),
            |v| {
                v["target"]["env"] = json!("prod\u{1b}[2J");
                v["steps"][1]["command"] = json!("migrate\n<b>x</b>");
            },
        ))
        .unwrap();
        assert!(!view.sentence.contains('\n'), "{}", view.sentence);
        assert!(!view.actions[0].command.contains('\u{1b}'));
    }
}
