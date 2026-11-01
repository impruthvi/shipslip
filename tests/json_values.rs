//! Fixed-value strings must agree with the schema-1 reference.
use serde::Serialize;
use serde_json::{json, Value};
use shipslip::logs::*;
use shipslip::receipt::{ReceiptKind, ReceiptStepStatus};
use shipslip::records::*;
use shipslip::{LogPhase, RunPlan, WatchStatus};
use std::collections::BTreeSet;

fn check<T: Serialize>(registry: &Value, field: &str, cases: Vec<T>, tag: Option<&str>) {
    let actual = cases
        .into_iter()
        .map(|case| {
            let value = serde_json::to_value(case).unwrap();
            let value = tag.map_or(&value, |tag| &value[tag]);
            value.as_str().unwrap().to_string()
        })
        .collect::<BTreeSet<_>>();
    let expected = registry[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert_eq!(actual, expected, "{field}");
}

#[test]
fn serialized_fixed_values_match_the_json_reference() {
    let doc = include_str!("../docs/json-reference.md");
    let registry: Value = serde_json::from_str(
        doc.split("```json\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap(),
    )
    .unwrap();
    check(
        &registry,
        "log.badge",
        vec![LogBadge::New, LogBadge::Seen, LogBadge::NoBaseline],
        None,
    );
    check(
        &registry,
        "log.level",
        [
            "debug",
            "info",
            "notice",
            "warning",
            "error",
            "critical",
            "alert",
            "emergency",
        ]
        .map(|name| Level::parse(name).unwrap().name())
        .to_vec(),
        None,
    );
    check(
        &registry,
        "log.channel.kind",
        vec![
            LogChannelKind::Single,
            LogChannelKind::Daily,
            LogChannelKind::Mixed,
        ],
        None,
    );
    check(
        &registry,
        "log.coverage.kind",
        vec![LogCoverage::Complete, LogCoverage::Partial { from: None }],
        Some("kind"),
    );
    check(
        &registry,
        "log.format.kind",
        vec![
            LogFormat::Recognized,
            LogFormat::Partial { percent: 20 },
            LogFormat::Unrecognized,
            LogFormat::Empty,
            LogFormat::NotRead,
        ],
        Some("kind"),
    );
    check(
        &registry,
        "log.anchor.kind",
        vec![
            Anchor::Run {
                run_id: "x".into(),
                plan: "x".into(),
                target_commit: "x".into(),
                check: RunCheck::Verified,
                server_marker_outcome: None,
                reflog_recorded: false,
            },
            Anchor::RunInProgress {
                run_id: "x".into(),
                step: None,
            },
            Anchor::CheckoutChange { commit: "x".into() },
            Anchor::Since {
                since: Since::Ago(1),
            },
            Anchor::Default { seconds: 86400 },
        ],
        Some("kind"),
    );
    check(
        &registry,
        "log.since.kind",
        vec![Since::Ago(1), Since::At("x".into())],
        Some("kind"),
    );
    check(
        &registry,
        "log.check.kind",
        vec![
            RunCheck::Verified,
            RunCheck::Unverified { reason: "x".into() },
        ],
        Some("kind"),
    );
    check(
        &registry,
        "log.fallback.kind",
        vec![
            AnchorFallback::Run {
                run_id: "x".into(),
                reason: RunFallbackReason::ReflogMissing,
            },
            AnchorFallback::LastRunMarker {
                reason: MarkerFallbackReason::Missing,
            },
            AnchorFallback::CheckoutChange {
                reason: CheckoutFallbackReason::ReflogMissing,
            },
        ],
        Some("kind"),
    );
    check(
        &registry,
        "log.run_fallback.kind",
        vec![
            RunFallbackReason::ReflogMissing,
            RunFallbackReason::TargetNotReached {
                target_commit: "x".into(),
            },
            RunFallbackReason::CheckoutChanged,
            RunFallbackReason::InvalidStartTime,
        ],
        Some("kind"),
    );
    check(
        &registry,
        "log.marker_fallback.kind",
        vec![
            MarkerFallbackReason::Missing,
            MarkerFallbackReason::Corrupt,
            MarkerFallbackReason::Unreadable {
                message: "x".into(),
            },
        ],
        Some("kind"),
    );
    check(
        &registry,
        "log.checkout_fallback.kind",
        vec![
            CheckoutFallbackReason::ReflogMissing,
            CheckoutFallbackReason::ReflogReadFailed {
                message: "x".into(),
            },
            CheckoutFallbackReason::HeadReadFailed {
                message: "x".into(),
            },
            CheckoutFallbackReason::InvalidStartTime { commit: "x".into() },
        ],
        Some("kind"),
    );
    check(
        &registry,
        "log.same_request.kind",
        vec![
            SameRequest::NoId,
            SameRequest::Cut,
            SameRequest::Shared {
                key: "request_id",
                value: "x".into(),
                conflict: false,
                before: vec![],
                after: vec![],
                more: 0,
                may_be_missing: false,
            },
        ],
        Some("kind"),
    );
    let mut keys = Vec::new();
    for key in [
        "request_id",
        "trace_id",
        "correlation_id",
        "job_id",
        "batch_id",
    ] {
        // Request-ID recognition is observable through a real snapshot, covered by logs tests.
        let record = SameRequest::Shared {
            key,
            value: "x".into(),
            conflict: false,
            before: vec![],
            after: vec![],
            more: 0,
            may_be_missing: false,
        };
        keys.push(serde_json::to_value(record).unwrap()["key"].clone());
    }
    check(&registry, "log.same_request.key", keys, None);
    check(
        &registry,
        "log.error.kind",
        vec![
            LogsQueryError::invalid("x"),
            LogsQueryError::Unreadable {
                message: "x".into(),
            },
            LogsQueryError::Missing,
            LogsQueryError::Ambiguous { ids: vec![] },
            LogsQueryError::SshRefused {
                reason: SshRefusal::Other,
                message: "x".into(),
            },
            LogsQueryError::ConnectionLost {
                message: "x".into(),
            },
            LogsQueryError::LogPathNotTrusted {
                path: "x".into(),
                environment: "x".into(),
            },
            LogsQueryError::ZoneMissing { zone: "x".into() },
            LogsQueryError::ServerReadFailed {
                message: "x".into(),
            },
            LogsQueryError::NotApproved {
                environment: "x".into(),
            },
        ],
        Some("kind"),
    );
    check(
        &registry,
        "log.error.reason",
        vec![
            SshRefusal::HostKeyUnknown,
            SshRefusal::HostKeyChanged,
            SshRefusal::KeyNotLoaded,
            SshRefusal::Other,
        ],
        None,
    );
    check(
        &registry,
        "receipt.error.kind",
        vec![
            ReceiptQueryError::Missing,
            ReceiptQueryError::Ambiguous { ids: vec![] },
            ReceiptQueryError::NewerVersion { version: 99 },
            ReceiptQueryError::Unreadable {
                message: "x".into(),
            },
        ],
        Some("kind"),
    );
    check(
        &registry,
        "receipt.kind",
        vec![ReceiptKind::Deploy, ReceiptKind::Up],
        None,
    );
    let plans = [RunPlan::Deploy, RunPlan::Rerun, RunPlan::FromStep(1)].map(|plan| {
        let value = serde_json::to_value(plan).unwrap();
        if value.is_string() {
            value
        } else {
            json!(value.as_object().unwrap().keys().next().unwrap())
        }
    });
    check(&registry, "receipt.run_plan", plans.to_vec(), None);
    check(
        &registry,
        "receipt.outcome.kind",
        vec![
            OutcomeKind::Succeeded,
            OutcomeKind::FailedAtStep {
                step: 1,
                partial_update: false,
            },
            OutcomeKind::StoppedAfterStep { step: 1 },
            OutcomeKind::StoppedInMaintenance,
            OutcomeKind::Unknown { step: 1 },
            OutcomeKind::CancelledBeforeChanges,
            OutcomeKind::AbortedBeforeChanges,
            OutcomeKind::NotRecorded,
        ],
        Some("kind"),
    );
    check(
        &registry,
        "receipt.flag.kind",
        vec![
            Flag::NewErrors { groups: 1 },
            Flag::LogNotFullyObserved,
            Flag::LogNotRecorded,
            Flag::SmokeFailed { status: None },
            Flag::SmokeNotRecorded,
            Flag::AppLeftDown,
        ],
        Some("kind"),
    );
    check(
        &registry,
        "receipt.step.kind",
        vec![
            StepKind::MaintenanceDown,
            StepKind::Step { index: 0 },
            StepKind::MaintenanceUp,
        ],
        Some("kind"),
    );
    check(
        &registry,
        "receipt.step.status",
        vec![
            StepState::Pending,
            StepState::Running,
            StepState::Ok,
            StepState::Failed,
            StepState::Unknown,
            StepState::NotStarted,
            StepState::Skipped,
        ],
        None,
    );
    check(
        &registry,
        "receipt.watch.status",
        vec![
            WatchStatus::Complete,
            WatchStatus::Partial,
            WatchStatus::Unavailable,
            WatchStatus::Cancelled,
            WatchStatus::NotRun,
            WatchStatus::NoLogSeen,
        ],
        None,
    );
    check(
        &registry,
        "receipt.watch.phase",
        vec![LogPhase::During, LogPhase::After],
        None,
    );
    check(
        &registry,
        "receipt.smoke.kind",
        vec![
            Smoke::NotConfigured,
            Smoke::Skipped,
            Smoke::Passed {
                status: 200,
                latency_ms: 1,
            },
            Smoke::Failed {
                status: None,
                reason: "x".into(),
            },
        ],
        Some("kind"),
    );
    let parts = [Part::Text("x".into()), Part::Command(vec![])].map(|part| {
        json!(serde_json::to_value(part)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap())
    });
    check(&registry, "receipt.next_step.part", parts.to_vec(), None);
    check(
        &registry,
        "status.mark",
        vec![
            Mark::Recorded,
            Mark::Incomplete,
            Mark::Observed,
            Mark::NotKnown,
            Mark::NoCode,
        ],
        None,
    );
    check(
        &registry,
        "status.code.kind",
        vec![
            CodeState::Recorded {
                sha: "x".into(),
                complete: true,
                evidence: Evidence::FastForward,
                run_id: "x".into(),
                at_ms: 0,
            },
            CodeState::Observed {
                sha: "x".into(),
                dirty: None,
                expected: "x".into(),
                run_id: "x".into(),
                outcome: None,
                at_ms: 0,
            },
            CodeState::NotKnown {
                run_id: None,
                reason: NotKnownReason::NoStepZero,
            },
            CodeState::NeverDeployed,
            CodeState::NoCodeChangeInLast { records: 0 },
            CodeState::TargetChanged {
                run_id: "x".into(),
                target: "x".into(),
            },
        ],
        Some("kind"),
    );
    check(
        &registry,
        "status.code.evidence",
        vec![Evidence::FastForward, Evidence::CheckedOutAtStart],
        None,
    );
    check(
        &registry,
        "status.code.reason.kind",
        vec![
            NotKnownReason::StepZero(ReceiptStepStatus::Unknown),
            NotKnownReason::NoStepZero,
            NotKnownReason::Unreadable,
        ],
        Some("kind"),
    );
    check(
        &registry,
        "status.code.reason.status",
        vec![
            ReceiptStepStatus::Pending,
            ReceiptStepStatus::Running,
            ReceiptStepStatus::Ok,
            ReceiptStepStatus::Failed,
            ReceiptStepStatus::Unknown,
            ReceiptStepStatus::NotStarted,
            ReceiptStepStatus::Skipped,
        ],
        None,
    );
    check(
        &registry,
        "status.stale_hint.kind",
        vec![
            StaleHint::NewestUnreadable { path: "x".into() },
            StaleHint::UnattributedNewer { path: "x".into() },
        ],
        Some("kind"),
    );
    check(
        &registry,
        "status.problem.kind",
        vec![
            ProblemKind::Trust,
            ProblemKind::ProjectDir,
            ProblemKind::Receipt,
        ],
        None,
    );
    let summary = ReceiptSummary {
        run_id: "x".into(),
        env: "x".into(),
        repo_root: "x".into(),
        started_at_ms: 0,
        kind: ReceiptKind::Deploy,
        run_plan: RunPlan::Deploy,
        from_sha: "x".into(),
        target_sha: "x".into(),
        outcome: Outcome {
            kind: OutcomeKind::Succeeded,
            unfinished: false,
        },
        flags: vec![],
        app_left_down: false,
        recipe_steps: 0,
        steps: vec![],
    };
    check(
        &registry,
        "receipt.row.state",
        vec![
            ReceiptListRow::Receipt {
                id: "x".into(),
                path: "x".into(),
                receipt: Box::new(summary),
            },
            ReceiptListRow::Newer {
                id: "x".into(),
                env: "x".into(),
                started_at_ms: None,
                version: 99,
                path: "x".into(),
            },
            ReceiptListRow::Unreadable {
                id: "x".into(),
                env: "x".into(),
                started_at_ms: None,
                path: "x".into(),
                reason: "x".into(),
            },
        ],
        Some("state"),
    );
}
