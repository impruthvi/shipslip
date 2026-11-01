use base64::{engine::general_purpose::STANDARD, Engine as _};
use shipslip::logs::{snapshot, Anchor, Request, Since};
use shipslip::logs::{
    AnchorFallback, CheckoutFallbackReason, MarkerFallbackReason, RunCheck, RunFallbackReason,
};
use shipslip::records::Untrusted;
use shipslip::transport::{Transport, TransportError};
use shipslip::DeployTarget;
use tokio::sync::mpsc::Sender;

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TIP: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa HEAD@{90000} merge: fast-forward";

fn encoded(text: &str) -> String {
    STANDARD.encode(text)
}

fn marker() -> String {
    format!("@marker 1\n@run {}\n@plan deploy\n@target {SHA}\n@started 90000\n@end\n@finished 95000 succeeded\n@reflog {}\n", encoded("r-one"), encoded(TIP))
}

fn run_probe() -> String {
    format!("@marker ok {}\n@git-head ok {SHA}\n@reflog ok {}\n@anchor-time marker 90000 2026-10-01 21:00:00 +0000\n@anchor-time reflog 90000 2026-10-01 21:00:00 +0000", encoded(&marker()), encoded(TIP))
}

struct ProbeReply(String);

impl Transport for ProbeReply {
    async fn run(
        &self,
        _script: &str,
        output: Sender<String>,
        _line_limit: usize,
    ) -> Result<i32, TransportError> {
        for line in self.0.lines() {
            output.send(line.to_owned()).await.unwrap();
        }
        Ok(0)
    }

    async fn reconnect(&self) -> Result<(), TransportError> {
        panic!("a snapshot must not reconnect")
    }
}

async fn read(probe: &str, request: Request) -> shipslip::logs::Snapshot {
    let target = DeployTarget {
        env: "production".into(),
        production: true,
        ssh_alias: "app".into(),
        path: "/srv/app".into(),
        branch: "main".into(),
        steps: Vec::new(),
        maintenance: false,
        watch_log: false,
        log: None,
        log_daily: false,
        smoke_url: None,
        timezone: None,
        logs: Default::default(),
    };
    snapshot(
        &ProbeReply(format!(
            "@time 100000 13600 2026-10-01 00:00:00 2026-10-01 +0000 +0000\n{probe}\n@end\n"
        )),
        &target,
        &request,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn default_anchor_exposes_the_24_hour_window() {
    let snapshot = read("@marker missing\n@reflog missing", Request::default()).await;
    assert_eq!(snapshot.anchor, Anchor::Default { seconds: 86400 });
}

#[tokio::test]
async fn explicit_since_keeps_the_users_relative_or_local_time() {
    for since in [Since::Ago(7200), Since::At("2026-10-01 14:00".into())] {
        let snapshot = read(
            "@marker missing",
            Request {
                since: Some(since.clone()),
                ..Request::default()
            },
        )
        .await;
        assert_eq!(snapshot.anchor, Anchor::Since { since });
    }
}

#[tokio::test]
async fn verified_run_keeps_marker_outcome_as_server_text() {
    let snapshot = read(&run_probe(), Request::default()).await;
    assert_eq!(
        snapshot.anchor,
        Anchor::Run {
            run_id: "r-one".into(),
            plan: "deploy".into(),
            target_commit: SHA.into(),
            check: RunCheck::Verified,
            server_marker_outcome: Some(Untrusted::new("succeeded")),
            reflog_recorded: true,
        }
    );
}

#[tokio::test]
async fn an_active_run_exposes_its_id_and_optional_step() {
    for step in [Some(2), None] {
        let probe = format!(
            "{}\n@active {} {}",
            run_probe(),
            encoded("r-one"),
            step.map(|step| step.to_string()).unwrap_or("-".into())
        );
        let snapshot = read(&probe, Request::default()).await;
        assert_eq!(
            snapshot.anchor,
            Anchor::RunInProgress {
                run_id: "r-one".into(),
                step
            }
        );
    }
}

#[tokio::test]
async fn checkout_change_exposes_the_full_commit() {
    let probe = format!(
        "@marker missing\n@reflog ok {}\n@anchor-time reflog 90000 2026-10-01 21:00:00 +0000",
        encoded(TIP)
    );
    let snapshot = read(&probe, Request::default()).await;
    assert_eq!(
        snapshot.anchor,
        Anchor::CheckoutChange { commit: SHA.into() }
    );
}

#[tokio::test]
async fn a_run_with_no_reflog_falls_back_with_its_run_id_and_reason() {
    let probe = run_probe()
        .lines()
        .filter(|line| !line.starts_with("@reflog "))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n@reflog missing";
    let snapshot = read(&probe, Request::default()).await;
    assert_eq!(snapshot.anchor, Anchor::Default { seconds: 86400 });
    assert!(snapshot.anchor_fallbacks.contains(&AnchorFallback::Run {
        run_id: "r-one".into(),
        reason: RunFallbackReason::ReflogMissing,
    }));
    assert!(snapshot
        .warnings
        .iter()
        .any(|warning| warning == "run r-one could not be verified: reflog is missing"));
}

#[tokio::test]
async fn rejected_runs_expose_why_their_anchor_could_not_be_used() {
    let changed = run_probe().replace(
        &encoded(TIP),
        &encoded(&TIP.replace("fast-forward", "checkout")),
    );
    let unreached = run_probe()
        .replace(
            &format!("@git-head ok {SHA}"),
            &format!("@git-head ok {}", "b".repeat(40)),
        )
        .replace(
            &encoded(&marker()),
            &encoded(&marker().replace(&format!("@reflog {}\n", encoded(TIP)), "")),
        );
    for (probe, reason, warning) in [
        (
            changed,
            RunFallbackReason::CheckoutChanged,
            "checkout changed after run r-one",
        ),
        (
            unreached,
            RunFallbackReason::TargetNotReached {
                target_commit: SHA.into(),
            },
            "run r-one did not reach its target aaaaaaa",
        ),
        (
            run_probe().replace("@anchor-time marker 90000", "@anchor-time marker 90001"),
            RunFallbackReason::InvalidStartTime,
            "last-run marker has an invalid start time",
        ),
    ] {
        let snapshot = read(&probe, Request::default()).await;
        assert_eq!(
            snapshot.anchor,
            Anchor::CheckoutChange { commit: SHA.into() }
        );
        assert_eq!(
            snapshot.anchor_fallbacks,
            vec![AnchorFallback::Run {
                run_id: "r-one".into(),
                reason
            }]
        );
        assert!(snapshot.warnings.iter().any(|value| value == warning));
    }
}

#[tokio::test]
async fn missing_corrupt_and_unreadable_markers_remain_distinct_fallbacks() {
    for (marker, reason, warning) in [
        (
            "@marker missing".into(),
            MarkerFallbackReason::Missing,
            None,
        ),
        (
            format!("@marker ok {}", encoded("bad marker")),
            MarkerFallbackReason::Corrupt,
            Some("anchor: last-run marker is corrupt"),
        ),
        (
            "@marker ok %%%".into(),
            MarkerFallbackReason::Corrupt,
            Some("anchor: last-run marker is corrupt"),
        ),
        (
            format!("@marker error {}", encoded("permission denied")),
            MarkerFallbackReason::Unreadable {
                message: "permission denied".into(),
            },
            Some("anchor: permission denied"),
        ),
        (
            "@marker error %%%".into(),
            MarkerFallbackReason::Unreadable {
                message: "could not read last-run marker".into(),
            },
            Some("anchor: could not read last-run marker"),
        ),
    ] {
        let probe = format!(
            "{marker}\n@reflog ok {}\n@anchor-time reflog 90000 2026-10-01 21:00:00 +0000",
            encoded(TIP)
        );
        let snapshot = read(&probe, Request::default()).await;
        assert_eq!(
            snapshot.anchor,
            Anchor::CheckoutChange { commit: SHA.into() }
        );
        assert_eq!(
            snapshot.anchor_fallbacks,
            vec![AnchorFallback::LastRunMarker { reason }]
        );
        assert_eq!(
            snapshot
                .warnings
                .iter()
                .find(|warning| warning.starts_with("anchor: "))
                .map(String::as_str),
            warning
        );
    }
}

#[tokio::test]
async fn checkout_fallbacks_distinguish_missing_failed_and_invalid_reflogs() {
    for (probe, reason, warning) in [
        (
            "@reflog missing".into(),
            CheckoutFallbackReason::ReflogMissing,
            None,
        ),
        (
            format!("@reflog error {}", encoded("git refused")),
            CheckoutFallbackReason::ReflogReadFailed {
                message: "git refused".into(),
            },
            Some("anchor: reflog failed (git refused) — using 24h"),
        ),
        (
            format!("@git-head error {}\n@reflog missing", encoded("bad HEAD")),
            CheckoutFallbackReason::HeadReadFailed {
                message: "bad HEAD".into(),
            },
            Some("anchor: reflog failed (bad HEAD) — using 24h"),
        ),
        (
            format!(
                "@reflog ok {}\n@anchor-time reflog 90001 2026-10-01 21:00:00 +0000",
                encoded(TIP)
            ),
            CheckoutFallbackReason::InvalidStartTime { commit: SHA.into() },
            None,
        ),
        (
            "@reflog ok %%%".into(),
            CheckoutFallbackReason::ReflogReadFailed {
                message: "invalid reflog output".into(),
            },
            Some("anchor: reflog failed (invalid reflog output) — using 24h"),
        ),
        (
            "@reflog error %%%".into(),
            CheckoutFallbackReason::ReflogReadFailed {
                message: "reflog failed".into(),
            },
            Some("anchor: reflog failed (reflog failed) — using 24h"),
        ),
    ] {
        let snapshot = read(&format!("@marker missing\n{probe}"), Request::default()).await;
        assert_eq!(snapshot.anchor, Anchor::Default { seconds: 86400 });
        assert_eq!(
            snapshot.anchor_fallbacks,
            vec![
                AnchorFallback::LastRunMarker {
                    reason: MarkerFallbackReason::Missing
                },
                AnchorFallback::CheckoutChange { reason },
            ]
        );
        assert_eq!(
            snapshot
                .warnings
                .iter()
                .find(|warning| warning.starts_with("anchor: "))
                .map(String::as_str),
            warning
        );
    }
}

#[tokio::test]
async fn git_read_errors_keep_the_run_but_mark_its_check_unverified() {
    for (probe, reason) in [
        (
            run_probe().replace(
                &format!("@git-head ok {SHA}"),
                &format!("@git-head error {}", encoded("dubious ownership")),
            ),
            "dubious ownership",
        ),
        (
            run_probe().replace(
                &format!("@reflog ok {}", encoded(TIP)),
                &format!("@reflog error {}", encoded("git refused")),
            ),
            "git refused",
        ),
        (
            run_probe().replace(&format!("@git-head ok {SHA}"), "@git-head ok invalid"),
            "invalid HEAD",
        ),
    ] {
        let snapshot = read(&probe, Request::default()).await;
        let Anchor::Run {
            check,
            server_marker_outcome,
            ..
        } = snapshot.anchor
        else {
            panic!("the run should remain the anchor")
        };
        assert_eq!(
            check,
            RunCheck::Unverified {
                reason: reason.into()
            }
        );
        assert_eq!(server_marker_outcome, Some(Untrusted::new("succeeded")));
        assert!(snapshot.anchor_fallbacks.is_empty());
    }
}

#[tokio::test]
async fn an_outcome_without_recorded_reflog_stays_server_text_but_the_label_is_unknown() {
    let old_marker = marker().replace(&format!("@reflog {}\n", encoded(TIP)), "");
    let probe = run_probe().replace(&encoded(&marker()), &encoded(&old_marker));
    let snapshot = read(&probe, Request::default()).await;
    let Anchor::Run {
        check,
        server_marker_outcome,
        reflog_recorded,
        ..
    } = snapshot.anchor
    else {
        panic!("expected a run anchor")
    };
    assert_eq!(check, RunCheck::Verified);
    assert_eq!(server_marker_outcome, Some(Untrusted::new("succeeded")));
    assert!(!reflog_recorded);
}

#[tokio::test]
async fn an_unfinished_marker_has_no_server_outcome() {
    let unfinished = marker().replace("@finished 95000 succeeded\n", "");
    let probe = run_probe().replace(&encoded(&marker()), &encoded(&unfinished));
    let snapshot = read(&probe, Request::default()).await;
    assert!(matches!(
        snapshot.anchor,
        Anchor::Run {
            server_marker_outcome: None,
            ..
        }
    ));
}

#[tokio::test]
async fn explicit_since_bypasses_anchor_failures() {
    let snapshot = read(
        "@marker ok %%%\n@reflog error %%%",
        Request {
            since: Some(Since::Ago(86400)),
            ..Request::default()
        },
    )
    .await;
    assert_eq!(
        snapshot.anchor,
        Anchor::Since {
            since: Since::Ago(86400)
        }
    );
    assert!(snapshot.anchor_fallbacks.is_empty());
    assert!(!snapshot
        .warnings
        .iter()
        .any(|warning| warning.starts_with("anchor: ")));
}
