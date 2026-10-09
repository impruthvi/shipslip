use super::*;
use shipslip::receipt::{AttemptSummary, Badge, Flag, NewerElsewhere, Problem};
use shipslip::{DeployOutcome, RunPlan};

const NOW: u128 = 10 * 86_400_000;
const A: &str = "a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0";
const B: &str = "b1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0";

fn args(values: &[&str]) -> Result<Args, String> {
    Args::parse(
        &values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())
}

fn cell(env: &str, code: CodeState) -> EnvCell {
    EnvCell {
        env: env.into(),
        approved: false,
        code,
        last_attempt: None,
        stale_hint: None,
        target: None,
        newer_elsewhere: None,
    }
}

fn row(label: &str, envs: Vec<EnvCell>) -> RepoRow {
    RepoRow {
        repo_root: format!("/work/{label}"),
        project: Some(label.into()),
        label: label.into(),
        path_missing: false,
        envs,
    }
}

fn recorded(complete: bool, evidence: Evidence) -> CodeState {
    CodeState::Recorded {
        sha: A.into(),
        complete,
        evidence,
        run_id: "aaaa0001-0".into(),
        at_ms: NOW - 3 * 86_400_000,
    }
}

fn no_compare(_: &RepoRow, _: &str, _: &str) -> EnvComparison {
    unreachable!("no comparison requested")
}

#[test]
fn parses_an_optional_pair_to_compare() {
    assert_eq!(args(&[]), Ok(Args { compare: None }));
    assert_eq!(
        args(&["--compare", "staging", "production"]),
        Ok(Args {
            compare: Some(("staging".into(), "production".into()))
        })
    );
    for bad in [
        &["staging"][..],
        &["--compare", "staging"],
        &["--compare", "staging", "staging"],
        &["--compare", "a", "b", "c"],
    ] {
        assert_eq!(
            args(bad),
            Err("usage: slip status [--compare ENV ENV]".into()),
            "{bad:?}"
        );
    }
}

#[test]
fn every_code_state_has_its_own_words() {
    let lines = |code: CodeState, approved: bool| {
        let mut cell = cell("staging", code);
        cell.approved = approved;
        code_line(&cell, NOW)
    };
    assert_eq!(
        lines(recorded(true, Evidence::FastForward), false),
        "code at a1b2c3d4e5f6 (recorded 3d ago)"
    );
    assert_eq!(
        lines(recorded(false, Evidence::FastForward), false),
        "code at a1b2c3d4e5f6, deploy incomplete (recorded 3d ago)"
    );
    assert_eq!(
        lines(recorded(true, Evidence::CheckedOutAtStart), false),
        "code at a1b2c3d4e5f6, checked out at start (recorded 3d ago)"
    );
    let observed = |sha: &str, dirty, outcome| CodeState::Observed {
        sha: sha.into(),
        dirty,
        expected: A.into(),
        run_id: "bbbb0002-0".into(),
        outcome,
        at_ms: NOW - 2 * 3_600_000,
    };
    assert_eq!(
        lines(observed(A, Some(true), None), false),
        "observed a1b2c3d4e5f6 after run bbbb0002, tree dirty (2h ago)"
    );
    assert_eq!(
        lines(
            observed(
                B,
                Some(false),
                Some(DeployOutcome::FailedAtStep {
                    step: 2,
                    partial_update: false
                })
            ),
            false
        ),
        "observed b1b2c3d4e5f6 after run bbbb0002 failed at step 2, expected a1b2c3d4e5f6 (2h ago)"
    );
    assert_eq!(
        lines(
            observed(
                A,
                None,
                Some(DeployOutcome::Unknown {
                    step: 1,
                    reason: "connection lost".into()
                })
            ),
            false
        ),
        "observed a1b2c3d4e5f6 after run bbbb0002 lost track of step 1 (2h ago)"
    );
    assert_eq!(
        lines(
            CodeState::NotKnown {
                run_id: Some("cccc0003-0".into()),
                reason: NotKnownReason::StepZero(ReceiptStepStatus::Unknown),
            },
            false
        ),
        "server code not known after run cccc0003 (fast-forward result unknown)"
    );
    assert_eq!(
        lines(
            CodeState::NotKnown {
                run_id: None,
                reason: NotKnownReason::Unreadable,
            },
            false
        ),
        "server code not known (its receipts could not be read)"
    );
    assert_eq!(
        lines(CodeState::NeverDeployed, true),
        "approved before, never deployed"
    );
    assert_eq!(lines(CodeState::NeverDeployed, false), "never deployed");
    assert_eq!(
        lines(CodeState::NoCodeChangeInLast { records: 20 }, false),
        "no code change in the last 20 runs"
    );
    assert_eq!(
        lines(
            CodeState::TargetChanged {
                run_id: "dddd0004-0".into(),
                target: "web2:/srv/app".into(),
            },
            false
        ),
        "target changed in run dddd0004; no code recorded for web2:/srv/app"
    );
}

#[test]
fn map_shows_last_runs_targets_hints_and_problems() {
    let mut production = cell("production", recorded(true, Evidence::FastForward));
    production.target = Some("web1:/srv/shop".into());
    production.last_attempt = Some(AttemptSummary {
        run_id: "eeee0005-0".into(),
        started_at_ms: NOW - 20 * 60_000,
        run_plan: RunPlan::Deploy,
        badge: Badge {
            unfinished: true,
            outcome: Some(DeployOutcome::Succeeded),
            flags: vec![Flag::NewErrors(2)],
        },
        next_steps: vec![
            "The run did not finish. If no deploy is running, resume it from /work/shop with `slip attach production`.".into(),
        ],
    });
    let mut staging = cell("staging", recorded(true, Evidence::FastForward));
    staging.stale_hint = Some(StaleHint::UnattributedNewer {
        path: "/r/shop/staging/9-x.json".into(),
    });
    let mut gone = row("blog", vec![cell("staging", CodeState::NeverDeployed)]);
    gone.path_missing = true;
    let overview = Overview {
        repos: vec![row("shop", vec![production, staging]), gone],
        problems: vec![Problem {
            path: "/r/shop/staging/9-x.json".into(),
            message: "invalid receipt `/r/shop/staging/9-x.json`: expected value".into(),
        }],
    };

    let text = render(&overview, NOW, None, no_compare);
    assert_eq!(
        text,
        "Last recorded code per environment, from local receipts. Nothing here is live.\n\
         \n\
         shop  /work/shop\n\
         \x20 production  code at a1b2c3d4e5f6 (recorded 3d ago)\n\
         \x20             last run eeee0005 (20m ago): Unfinished (last recorded: Succeeded)  ⚠ 2 new error groups\n\
         \x20               → The run did not finish. If no deploy is running, resume it from /work/shop with `slip attach production`.\n\
         \x20             on web1:/srv/shop\n\
         \x20 staging     code at a1b2c3d4e5f6 (recorded 3d ago)\n\
         \x20             ⚠ a newer receipt could not be read; this may be out of date: /r/shop/staging/9-x.json\n\
         \n\
         blog  /work/blog  (checkout not found)\n\
         \x20 staging  never deployed\n\
         \n\
         Could not read:\n\
         \x20 invalid receipt `/r/shop/staging/9-x.json`: expected value\n"
    );
    assert!(!text.contains("deployed to") && !text.contains("running now"));
}

#[test]
fn receipt_text_cannot_add_lines_or_escape_sequences() {
    let mut forged = cell("stag\ning", recorded(true, Evidence::FastForward));
    forged.target = Some("web1:/srv/app\u{1b}[2J".into());
    if let CodeState::Recorded { sha, .. } = &mut forged.code {
        *sha = "evil\ncode at 000000".into();
    }
    let mut row = row("shop\nfake  /x", vec![forged]);
    row.repo_root = "/work/\u{1b}]0;title\u{7}".into();
    let text = render(
        &Overview {
            repos: vec![row],
            problems: vec![],
        },
        NOW,
        None,
        no_compare,
    );
    assert!(
        !text.contains('\u{1b}') && !text.contains('\u{7}'),
        "{text}"
    );
    assert_eq!(text.lines().count(), 5, "{text}");
}

#[test]
fn comparisons_print_per_checkout_with_notes() {
    let overview = Overview {
        repos: vec![
            row(
                "shop",
                vec![
                    cell("production", recorded(true, Evidence::FastForward)),
                    cell("staging", recorded(false, Evidence::FastForward)),
                ],
            ),
            row("blog", vec![cell("staging", CodeState::NeverDeployed)]),
        ],
        problems: vec![],
    };
    let text = render(
        &overview,
        NOW,
        Some(("staging", "production")),
        |row, a, b| {
            assert_eq!(
                (row.label.as_str(), a, b),
                ("shop", "staging", "production")
            );
            EnvComparison {
                result: Comparison::Ahead(4),
                notes: vec![("staging".into(), Qualifier::DeployIncomplete)],
            }
        },
    );
    assert!(
        text.ends_with(
            "\nstaging vs production:\n  shop  staging is 4 commits ahead of production (staging deploy incomplete)\n"
        ),
        "{text}"
    );

    let line = |result| {
        comparison_line(
            "staging",
            "production",
            &EnvComparison {
                result,
                notes: vec![],
            },
        )
    };
    assert_eq!(
        line(Comparison::Same),
        "staging and production are on the same commit"
    );
    assert_eq!(
        line(Comparison::Behind(1)),
        "staging is 1 commit behind production"
    );
    assert_eq!(
        line(Comparison::Diverged { a: 2, b: 1 }),
        "staging and production have diverged: 2 only on staging, 1 only on production"
    );
    assert_eq!(
        line(Comparison::Unavailable("checkout not found".into())),
        "cannot compare: checkout not found"
    );

    let none = render(&overview, NOW, Some(("qa", "demo")), no_compare);
    assert!(
        none.ends_with("\nqa vs demo:\n  No checkout has both qa and demo.\n"),
        "{none}"
    );
}

#[test]
fn empty_map_says_how_to_start() {
    let text = render(
        &Overview {
            repos: vec![],
            problems: vec![],
        },
        NOW,
        None,
        no_compare,
    );
    assert_eq!(
        text,
        "No deploys or approvals recorded yet. Run `slip trust ENV` and `slip deploy ENV` in a project.\n"
    );
}

#[test]
fn a_newer_run_from_another_checkout_is_called_out() {
    let mut staging = cell("staging", recorded(true, Evidence::FastForward));
    staging.newer_elsewhere = Some(NewerElsewhere {
        repo_root: "/private/tmp/app".into(),
        env: "staging".into(),
        run_id: "ffff0006-0".into(),
        started_at_ms: NOW - 3_600_000,
    });
    let text = render(
        &Overview {
            repos: vec![row("app", vec![staging])],
            problems: vec![],
        },
        NOW,
        None,
        no_compare,
    );
    assert!(
        text.contains(
            "\n           ⚠ a newer run (ffff0006, 1h ago) from /private/tmp/app (staging) used this server; this may be out of date\n"
        ),
        "{text}"
    );
}
