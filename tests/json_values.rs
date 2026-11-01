//! Fixed-value strings must agree with the schema-1 reference.
use serde::Serialize;
use serde_json::{json, Map, Value};
use shipslip::logs::*;
use shipslip::receipt::{ReceiptKind, ReceiptStepStatus};
use shipslip::records::*;
use shipslip::{LogPhase, RunPlan, WatchStatus};
use std::collections::BTreeSet;
use std::process::Command;

/// One case per variant. The exhaustive match makes a new variant a compile
/// error here, so it can't reach JSON without being listed and checked.
macro_rules! every {
    ($ty:ident { $($variant:ident $(($($arg:expr),*))? $({ $($field:ident: $value:expr),* $(,)? })?),+ $(,)? }) => {{
        fn _exhaustive(value: &$ty) {
            match value {
                $($ty::$variant { .. } => {})+
            }
        }
        vec![$($ty::$variant $(($($arg),*))? $({ $($field: $value),* })?),+]
    }};
}

struct Reference {
    fields: Map<String, Value>,
    checked: BTreeSet<String>,
}

impl Reference {
    fn new(fields: Value) -> Self {
        Self {
            fields: fields.as_object().unwrap().clone(),
            checked: BTreeSet::new(),
        }
    }

    fn from_doc() -> Self {
        let doc = include_str!("../docs/json-reference.md");
        let registry = doc
            .split("```json\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        Self::new(serde_json::from_str(registry).unwrap())
    }

    fn check<T: Serialize>(&mut self, field: &str, cases: Vec<T>, tag: Option<&str>) {
        let values = cases.into_iter().map(|case| {
            let value = serde_json::to_value(case).unwrap();
            match tag {
                Some(tag) => value[tag].clone(),
                None => value,
            }
        });
        self.check_values(field, values);
    }

    fn check_values(&mut self, field: &str, values: impl IntoIterator<Item = Value>) {
        let actual = values
            .into_iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect::<BTreeSet<_>>();
        let expected = self.fields[field]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect();
        assert_eq!(actual, expected, "{field}");
        self.checked.insert(field.to_string());
    }

    fn assert_all_checked(&self) {
        let listed = self.fields.keys().cloned().collect::<BTreeSet<_>>();
        assert_eq!(self.checked, listed, "every documented field has a check");
    }
}

/// The first key of an externally tagged enum, or the string itself.
fn external_tag(value: Value) -> Value {
    match value {
        Value::Object(object) => json!(object.keys().next().unwrap()),
        value => value,
    }
}

/// Runs outside any git repository so `--config` points at nothing usable.
fn early_error_kind(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_slip"))
        .current_dir(std::env::temp_dir())
        .env_remove("SHIPSLIP_CONFIG")
        .args(args)
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    value["error"]["kind"].clone()
}

#[test]
#[should_panic(expected = "log.badge")]
fn a_value_missing_from_the_reference_fails() {
    let mut reference = Reference::new(json!({"log.badge": ["new", "seen"]}));
    reference.check(
        "log.badge",
        every!(LogBadge {
            New,
            Seen,
            NoBaseline
        }),
        None,
    );
}

#[test]
#[should_panic(expected = "every documented field has a check")]
fn a_documented_field_without_a_check_fails() {
    let mut reference =
        Reference::new(json!({"log.badge": ["new", "seen", "no_baseline"], "log.extra": ["x"]}));
    reference.check(
        "log.badge",
        every!(LogBadge {
            New,
            Seen,
            NoBaseline
        }),
        None,
    );
    reference.assert_all_checked();
}

#[test]
fn serialized_fixed_values_match_the_json_reference() {
    let mut reference = Reference::from_doc();
    reference.check(
        "log.badge",
        every!(LogBadge {
            New,
            Seen,
            NoBaseline
        }),
        None,
    );
    reference.check(
        "log.level",
        every!(Level {
            Debug,
            Info,
            Notice,
            Warning,
            Error,
            Critical,
            Alert,
            Emergency,
        })
        .into_iter()
        .map(Level::name)
        .collect(),
        None,
    );
    reference.check(
        "log.channel.kind",
        every!(LogChannelKind {
            Single,
            Daily,
            Mixed
        }),
        None,
    );
    reference.check(
        "log.coverage.kind",
        every!(LogCoverage {
            Complete,
            Partial { from: None }
        }),
        Some("kind"),
    );
    reference.check(
        "log.format.kind",
        every!(LogFormat {
            Recognized,
            Partial { percent: 20 },
            Unrecognized,
            Empty,
            NotRead,
        }),
        Some("kind"),
    );
    reference.check(
        "log.anchor.kind",
        every!(Anchor {
            Run {
                run_id: "x".into(),
                plan: "x".into(),
                target_commit: "x".into(),
                check: RunCheck::Verified,
                server_marker_outcome: None,
                reflog_recorded: false,
            },
            RunInProgress {
                run_id: "x".into(),
                step: None,
            },
            CheckoutChange { commit: "x".into() },
            Since {
                since: Since::Ago(1)
            },
            Default { seconds: 86400 },
        }),
        Some("kind"),
    );
    reference.check(
        "log.since.kind",
        every!(Since {
            Ago(1),
            At("x".into())
        }),
        Some("kind"),
    );
    reference.check(
        "log.check.kind",
        every!(RunCheck {
            Verified,
            Unverified { reason: "x".into() },
        }),
        Some("kind"),
    );
    reference.check(
        "log.fallback.kind",
        every!(AnchorFallback {
            Run {
                run_id: "x".into(),
                reason: RunFallbackReason::ReflogMissing,
            },
            LastRunMarker {
                reason: MarkerFallbackReason::Missing,
            },
            CheckoutChange {
                reason: CheckoutFallbackReason::ReflogMissing,
            },
        }),
        Some("kind"),
    );
    reference.check(
        "log.run_fallback.kind",
        every!(RunFallbackReason {
            ReflogMissing,
            TargetNotReached {
                target_commit: "x".into(),
            },
            CheckoutChanged,
            InvalidStartTime,
        }),
        Some("kind"),
    );
    reference.check(
        "log.marker_fallback.kind",
        every!(MarkerFallbackReason {
            Missing,
            Corrupt,
            Unreadable {
                message: "x".into(),
            },
        }),
        Some("kind"),
    );
    reference.check(
        "log.checkout_fallback.kind",
        every!(CheckoutFallbackReason {
            ReflogMissing,
            ReflogReadFailed {
                message: "x".into(),
            },
            HeadReadFailed {
                message: "x".into(),
            },
            InvalidStartTime { commit: "x".into() },
        }),
        Some("kind"),
    );
    let shared = |key| SameRequest::Shared {
        key,
        value: "x".into(),
        conflict: false,
        before: vec![],
        after: vec![],
        more: 0,
        may_be_missing: false,
    };
    reference.check(
        "log.same_request.kind",
        every!(SameRequest {
            NoId,
            Cut,
            Shared {
                key: "request_id",
                value: "x".into(),
                conflict: false,
                before: vec![],
                after: vec![],
                more: 0,
                may_be_missing: false,
            },
        }),
        Some("kind"),
    );
    reference.check(
        "log.same_request.key",
        ID_KEYS.into_iter().map(shared).collect(),
        Some("key"),
    );
    reference.check(
        "log.error.kind",
        every!(LogsQueryError {
            InvalidInput {
                message: "x".into()
            },
            Unreadable {
                message: "x".into(),
            },
            Missing,
            Ambiguous { ids: vec![] },
            SshRefused {
                reason: SshRefusal::Other,
                message: "x".into(),
            },
            ConnectionLost {
                message: "x".into(),
            },
            LogPathNotTrusted {
                path: "x".into(),
                environment: "x".into(),
            },
            ZoneMissing { zone: "x".into() },
            ServerReadFailed {
                message: "x".into(),
            },
            NotApproved {
                environment: "x".into(),
            },
        }),
        Some("kind"),
    );
    reference.check(
        "log.error.reason",
        every!(SshRefusal {
            HostKeyUnknown,
            HostKeyChanged,
            KeyNotLoaded,
            Other,
        }),
        None,
    );
    reference.check(
        "receipt.error.kind",
        every!(ReceiptQueryError {
            Missing,
            Ambiguous { ids: vec![] },
            NewerVersion { version: 99 },
            Unreadable {
                message: "x".into(),
            },
        }),
        Some("kind"),
    );
    // Status and receipts argument and config failures are plain strings in the binary.
    let missing = std::env::temp_dir().join(format!("shipslip-no-config-{}", std::process::id()));
    reference.check_values(
        "cli.early_error.kind",
        [
            early_error_kind(&["status", "--json", "bogus"]),
            early_error_kind(&["--config", missing.to_str().unwrap(), "receipts", "--json"]),
        ],
    );
    reference.check("receipt.kind", every!(ReceiptKind { Deploy, Up }), None);
    reference.check_values(
        "receipt.run_plan",
        every!(RunPlan {
            Deploy,
            Rerun,
            FromStep(1)
        })
        .into_iter()
        .map(|plan| external_tag(serde_json::to_value(plan).unwrap())),
    );
    reference.check(
        "receipt.outcome.kind",
        every!(OutcomeKind {
            Succeeded,
            FailedAtStep {
                step: 1,
                partial_update: false,
            },
            StoppedAfterStep { step: 1 },
            StoppedInMaintenance,
            Unknown { step: 1 },
            CancelledBeforeChanges,
            AbortedBeforeChanges,
            NotRecorded,
        }),
        Some("kind"),
    );
    reference.check(
        "receipt.flag.kind",
        every!(Flag {
            NewErrors { groups: 1 },
            LogNotFullyObserved,
            LogNotRecorded,
            SmokeFailed { status: None },
            SmokeNotRecorded,
            AppLeftDown,
        }),
        Some("kind"),
    );
    reference.check(
        "receipt.step.kind",
        every!(StepKind {
            MaintenanceDown,
            Step { index: 0 },
            MaintenanceUp,
        }),
        Some("kind"),
    );
    reference.check(
        "receipt.step.status",
        every!(StepState {
            Pending,
            Running,
            Ok,
            Failed,
            Unknown,
            NotStarted,
            Skipped,
        }),
        None,
    );
    reference.check(
        "receipt.watch.status",
        every!(WatchStatus {
            Complete,
            Partial,
            Unavailable,
            Cancelled,
            NotRun,
            NoLogSeen,
        }),
        None,
    );
    reference.check(
        "receipt.watch.phase",
        every!(LogPhase { During, After }),
        None,
    );
    reference.check(
        "receipt.smoke.kind",
        every!(Smoke {
            NotConfigured,
            Skipped,
            Passed {
                status: 200,
                latency_ms: 1,
            },
            Failed {
                status: None,
                reason: "x".into(),
            },
        }),
        Some("kind"),
    );
    reference.check_values(
        "receipt.next_step.part",
        every!(Part {
            Text("x".into()),
            Command(vec![])
        })
        .into_iter()
        .map(|part| external_tag(serde_json::to_value(part).unwrap())),
    );
    reference.check(
        "status.mark",
        every!(Mark {
            Recorded,
            Incomplete,
            Observed,
            NotKnown,
            NoCode,
        }),
        None,
    );
    reference.check(
        "status.code.kind",
        every!(CodeState {
            Recorded {
                sha: "x".into(),
                complete: true,
                evidence: Evidence::FastForward,
                run_id: "x".into(),
                at_ms: 0,
            },
            Observed {
                sha: "x".into(),
                dirty: None,
                expected: "x".into(),
                run_id: "x".into(),
                outcome: None,
                at_ms: 0,
            },
            NotKnown {
                run_id: None,
                reason: NotKnownReason::NoStepZero,
            },
            NeverDeployed,
            NoCodeChangeInLast { records: 0 },
            TargetChanged {
                run_id: "x".into(),
                target: "x".into(),
            },
        }),
        Some("kind"),
    );
    reference.check(
        "status.code.evidence",
        every!(Evidence {
            FastForward,
            CheckedOutAtStart
        }),
        None,
    );
    reference.check(
        "status.code.reason.kind",
        every!(NotKnownReason {
            StepZero(ReceiptStepStatus::Unknown),
            NoStepZero,
            Unreadable,
        }),
        Some("kind"),
    );
    reference.check(
        "status.code.reason.status",
        every!(ReceiptStepStatus {
            Pending,
            Running,
            Ok,
            Failed,
            Unknown,
            NotStarted,
            Skipped,
        }),
        None,
    );
    reference.check(
        "status.stale_hint.kind",
        every!(StaleHint {
            NewestUnreadable { path: "x".into() },
            UnattributedNewer { path: "x".into() },
        }),
        Some("kind"),
    );
    reference.check(
        "status.problem.kind",
        every!(ProblemKind {
            Trust,
            ProjectDir,
            Receipt
        }),
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
    reference.check(
        "receipt.row.state",
        every!(ReceiptListRow {
            Receipt {
                id: "x".into(),
                path: "x".into(),
                receipt: Box::new(summary),
            },
            Newer {
                id: "x".into(),
                env: "x".into(),
                started_at_ms: None,
                version: 99,
                path: "x".into(),
            },
            Unreadable {
                id: "x".into(),
                env: "x".into(),
                started_at_ms: None,
                path: "x".into(),
                reason: "x".into(),
            },
        }),
        Some("state"),
    );
    reference.assert_all_checked();
}
