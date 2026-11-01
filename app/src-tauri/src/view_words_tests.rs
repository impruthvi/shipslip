use super::*;
use shipslip::receipt::ReceiptStepStatus;
use shipslip::records::{CodeState, Evidence, NotKnownReason, Outcome, OutcomeKind};

const OLD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";

fn recorded(sha: &str, complete: bool, evidence: Evidence, n: u64) -> CodeState {
    CodeState::Recorded {
        sha: sha.into(),
        complete,
        evidence,
        run_id: format!("{n:08x}-0").into(),
        at_ms: 1000 * u128::from(n) + 500,
    }
}

fn env_cell(env: &str, code: CodeState) -> EnvCell {
    EnvCell {
        env: env.into(),
        approved: false,
        approved_target: None,
        code,
        last_attempt: None,
        code_run: None,
        stale_hint: None,
        target: None,
        newer_elsewhere: None,
    }
}

fn every_code_state() -> Vec<EnvCell> {
    let mut approved = env_cell("demo", CodeState::NeverDeployed);
    approved.approved = true;
    vec![
        env_cell("a", recorded(NEW, true, Evidence::FastForward, 1)),
        env_cell("b", recorded(NEW, false, Evidence::CheckedOutAtStart, 1)),
        env_cell(
            "c",
            CodeState::Observed {
                sha: NEW.into(),
                dirty: Some(true),
                expected: OLD.into(),
                run_id: "00000002-0".into(),
                outcome: Some(Outcome {
                    kind: OutcomeKind::FailedAtStep {
                        step: 2,
                        partial_update: false,
                    },
                    unfinished: false,
                }),
                at_ms: 2500,
            },
        ),
        env_cell(
            "d",
            CodeState::NotKnown {
                run_id: Some("00000003-0".into()),
                reason: NotKnownReason::StepZero(ReceiptStepStatus::Failed),
            },
        ),
        env_cell(
            "e",
            CodeState::NotKnown {
                run_id: None,
                reason: NotKnownReason::Unreadable,
            },
        ),
        env_cell("f", CodeState::NeverDeployed),
        approved,
        env_cell("g", CodeState::NoCodeChangeInLast { records: 1 }),
        env_cell("h", CodeState::NoCodeChangeInLast { records: 3 }),
        env_cell(
            "i",
            CodeState::TargetChanged {
                run_id: "00000004-0".into(),
                target: "web2:/srv/app".into(),
            },
        ),
    ]
}

#[test]
fn cell_parts_keep_their_words_and_evidence() {
    let expected = [
        ("", vec![]),
        ("", vec!["deploy incomplete", "checked out at start"]),
        (
            "observed",
            vec![
                "after run 00000002 failed at step 2",
                "expected 111111111111",
                "tree dirty",
            ],
        ),
        (
            "server code not known",
            vec!["after run 00000003", "fast-forward failed"],
        ),
        (
            "server code not known",
            vec!["its receipts could not be read"],
        ),
        ("no deploy receipt on this Mac", vec![]),
        ("no deploy receipt on this Mac", vec!["approved before"]),
        ("no code change in the last run", vec![]),
        ("no code change in the last 3 runs", vec![]),
        (
            "no code recorded for web2:/srv/app",
            vec!["target changed in run 00000004"],
        ),
    ];
    for (cell, (words, evidence)) in every_code_state().iter().zip(expected) {
        let view = cell_view(cell, 2500 + 3 * 60_000, &mut Locators::default());
        assert_eq!(view.words, words);
        assert_eq!(view.evidence, evidence);
        if matches!(
            cell.code,
            CodeState::Recorded { .. } | CodeState::Observed { .. }
        ) {
            assert_eq!(view.sha.as_deref(), Some(NEW));
            assert_eq!(view.short_sha.as_deref(), Some("2222222"));
            assert_eq!(view.age.as_deref(), Some("3m ago"));
        }
    }
}

#[test]
fn cell_words_never_claim_what_is_live_on_the_server() {
    for cell in every_code_state() {
        let view = cell_view(&cell, 0, &mut Locators::default());
        for words in [vec![view.words], view.evidence, view.qualifiers].concat() {
            let words = words.to_lowercase();
            for claim in ["deployed", "is running", "live", "currently", "now on"] {
                assert!(!words.contains(claim), "{words:?} claims {claim:?}");
            }
        }
    }
}

#[test]
fn cell_parts_are_escaped() {
    let cell = env_cell(
        "a",
        CodeState::TargetChanged {
            run_id: "00000004-0".into(),
            target: "web\u{202e}2:/srv\n".into(),
        },
    );
    let view = cell_view(&cell, 0, &mut Locators::default());
    assert_eq!(view.words, "no code recorded for web\\u{202e}2:/srv\\x0a");
}

#[test]
fn age_uses_the_largest_whole_unit() {
    let minute = 60_000;
    let now = 10_000 * minute;
    let ago = |minutes: u128| age(now, now - minutes * minute);
    assert_eq!(age(now, now - 59_999), "just now");
    assert_eq!(ago(1), "1m ago");
    assert_eq!(ago(59), "59m ago");
    assert_eq!(ago(60), "1h ago");
    assert_eq!(ago(47 * 60 + 59), "47h ago");
    assert_eq!(ago(48 * 60), "2d ago");
    // A clock that went backwards is not a negative age.
    assert_eq!(age(now, now + minute), "just now");
}
#[test]
fn the_target_moved_warning_is_escaped() {
    let mut cell = env_cell("staging", CodeState::NeverDeployed);
    cell.target = Some("web1:/srv/app".into());
    cell.approved_target = Some("web\u{202e}1:/srv\n".into());
    let text = target_moved_text(&cell).unwrap();
    assert!(!text.contains('\u{202e}') && !text.contains('\n'));
}

#[test]
fn qualifiers_use_the_cells_facts_and_escape_untrusted_fields() {
    let mut cell = env_cell("staging", CodeState::NeverDeployed);
    assert_eq!(target_moved_text(&cell), None);
    assert_eq!(stale_hint_text(&cell), None);
    assert_eq!(newer_elsewhere_text(&cell, 0), None);
    cell.target = Some("web1:/srv/app".into());
    cell.approved_target = Some("web1:/srv/app-staging".into());
    assert_eq!(
        target_moved_text(&cell).as_deref(),
        Some("approved config now deploys to web1:/srv/app-staging, not this target")
    );
    cell.stale_hint = Some(shipslip::records::StaleHint::NewestUnreadable {
        path: "newer.json".into(),
    });
    assert_eq!(
        stale_hint_text(&cell).as_deref(),
        Some("a newer receipt could not be read; this may be out of date")
    );
    cell.newer_elsewhere = Some(shipslip::records::NewerElsewhere {
        repo_root: "/work/other\n".into(),
        env: "prod\u{1b}".into(),
        run_id: "00000002-0".into(),
        started_at_ms: 0,
    });
    assert_eq!(newer_elsewhere_text(&cell, 180_000).as_deref(), Some("a newer run (00000002, 3m ago) from /work/other\\x0a (prod\\x1b) used this server; this may be out of date"));
}
