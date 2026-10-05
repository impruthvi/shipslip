use super::*;
use crate::setup::{RepairOutcome, SetupPrompts};
use crate::setup_fixture::{quote, Fixture};

#[derive(Default)]
struct Prompts {
    setup_answer: Option<bool>,
    setup_questions: Vec<String>,
    publication_questions: Vec<String>,
    auth_log: Option<std::path::PathBuf>,
}
impl SetupPrompts for Prompts {
    async fn identity(
        &mut self,
        _field: &'static str,
        _interrupts: &mut Interrupts,
    ) -> io::Result<Option<String>> {
        panic!("publish does not configure identity")
    }
    async fn confirm(
        &mut self,
        _plan: &shipslip::setup::SetupPlan,
        question: &str,
        _interrupts: &mut Interrupts,
    ) -> io::Result<Option<bool>> {
        self.setup_questions.push(question.into());
        Ok(self.setup_answer)
    }
}
impl PublishPrompts for Prompts {
    async fn value(
        &mut self,
        question: &str,
        _default: &str,
        _check: fn(&str) -> Result<(), &'static str>,
        _interrupts: &mut Interrupts,
    ) -> io::Result<String> {
        if let Some(path) = &self.auth_log {
            assert!(
                fs::read_to_string(path)?.contains("auth status --hostname github.com --active")
            );
        }
        self.publication_questions.push(question.into());
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "stop after ordered detection",
        ))
    }
    async fn confirm(
        &mut self,
        _question: &str,
        _interrupts: &mut Interrupts,
    ) -> io::Result<Option<bool>> {
        panic!("no publication preview expected")
    }
}

async fn run(
    fixture: &Fixture,
    args: &mut Args,
    prompts: &mut Prompts,
) -> Result<PublishOutcome, Box<dyn Error>> {
    let mut interrupts = Interrupts::from_channel(tokio::sync::mpsc::unbounded_channel().1);
    run_at_inner(
        args,
        &fixture.root,
        &mut interrupts,
        &mut false,
        &fixture.context,
        fixture.root.join("ops"),
        prompts,
    )
    .await
}

#[tokio::test]
async fn standalone_guidance_and_declined_setup_exit_three_before_publication_prompts() {
    for guidance in [true, false] {
        let mut fixture = Fixture::new();
        fixture.context.macos = !guidance;
        let mut prompts = Prompts {
            setup_answer: Some(false),
            ..Prompts::default()
        };
        let outcome = run(&fixture, &mut Args::default(), &mut prompts)
            .await
            .unwrap();
        assert_eq!(outcome.exit_code(), ExitCode::from(3));
        assert!(matches!(outcome, PublishOutcome::NotReady) == guidance);
        assert!(prompts.publication_questions.is_empty());
        assert_eq!(prompts.setup_questions.len(), usize::from(!guidance));
        assert!(!fixture.root.join("actions").exists());
    }
}

#[tokio::test]
async fn standalone_failed_and_interrupted_repairs_keep_their_exit_codes() {
    for interrupt in [true, false] {
        let fixture = Fixture::new();
        fixture.brew("exit 14");
        let mut prompts = Prompts {
            setup_answer: if interrupt { None } else { Some(true) },
            ..Prompts::default()
        };
        let outcome = run(&fixture, &mut Args::default(), &mut prompts)
            .await
            .unwrap();
        assert_eq!(
            outcome.exit_code(),
            ExitCode::from(if interrupt { 130 } else { 1 })
        );
        assert!(prompts.publication_questions.is_empty());
    }
}

#[tokio::test]
async fn missing_github_auth_gets_manual_guidance_without_publication_prompts() {
    let fixture = Fixture::new();
    fixture.complete();
    fixture.script(
        &fixture.root.join("brew/bin/gh"),
        "if [ \"$1\" = --version ]; then echo 'gh 2'; else echo 'not logged in' >&2; exit 1; fi",
    );
    let mut prompts = Prompts::default();
    assert!(matches!(
        run(&fixture, &mut Args::default(), &mut prompts)
            .await
            .unwrap(),
        PublishOutcome::NotReady
    ));
    assert!(prompts.setup_questions.is_empty());
    assert!(prompts.publication_questions.is_empty());
}

#[tokio::test]
async fn ready_publish_detects_authentication_before_repository_and_visibility_questions() {
    let fixture = Fixture::new();
    fixture.complete();
    let log = fixture.root.join("gh-calls");
    fixture.script(
        &fixture.root.join("brew/bin/gh"),
        &format!(
            "echo \"$*\" >> {}; echo gh-ready",
            quote(&log.to_string_lossy())
        ),
    );
    let root = quote(&fixture.root.to_string_lossy());
    fixture.script(&fixture.root.join("brew/bin/git"), &format!("if [ \"$1\" = -c ]; then shift 2; fi; case \"$*\" in --version) echo 'git 2';; 'rev-parse --show-toplevel') echo {root};; *) exit 99;; esac"));
    for with_name in [false, true] {
        let mut args = Args {
            name: with_name.then(|| "shop".into()),
            ..Args::default()
        };
        let mut prompts = Prompts {
            auth_log: Some(log.clone()),
            ..Prompts::default()
        };
        let error = run(&fixture, &mut args, &mut prompts).await.unwrap_err();
        assert!(error.to_string().contains("stop after ordered detection"));
        assert_eq!(
            prompts.publication_questions,
            [if with_name {
                "Visibility (private/public)"
            } else {
                "GitHub repository name"
            }]
        );
        assert!(prompts.setup_questions.is_empty());
    }
}

#[tokio::test]
async fn repaired_publish_rechecks_tools_before_asking_repository_name() {
    let fixture = Fixture::new();
    let root = quote(&fixture.root.to_string_lossy());
    fixture.script(&fixture.root.join("sources/git"), &format!("if [ \"$1\" = -c ]; then shift 2; fi; case \"$*\" in --version) echo 'git 2';; 'rev-parse --show-toplevel') echo {root};; *) exit 99;; esac"));
    let log = fixture.root.join("gh-calls");
    fixture.script(
        &fixture.root.join("sources/gh"),
        &format!(
            "echo \"$*\" >> {}; echo ready",
            quote(&log.to_string_lossy())
        ),
    );
    let mut prompts = Prompts {
        setup_answer: Some(true),
        auth_log: Some(log),
        ..Prompts::default()
    };
    assert!(run(&fixture, &mut Args::default(), &mut prompts)
        .await
        .unwrap_err()
        .to_string()
        .contains("stop after ordered detection"));
    assert_eq!(
        prompts.setup_questions,
        ["Apply setup and continue publishing?"]
    );
    assert_eq!(prompts.publication_questions, ["GitHub repository name"]);
    assert_eq!(
        fs::read_to_string(fixture.root.join("actions")).unwrap(),
        "install gh git\n"
    );
}

#[test]
fn publish_outcomes_map_to_standalone_exit_codes() {
    for (repair, code) in [
        (RepairOutcome::NotReady, 3),
        (RepairOutcome::Declined, 3),
        (RepairOutcome::Failed, 1),
        (RepairOutcome::Invalid, 2),
        (RepairOutcome::Interrupted, 130),
    ] {
        assert_eq!(
            PublishOutcome::from_repair(repair).unwrap().exit_code(),
            ExitCode::from(code)
        );
    }
    assert!(PublishOutcome::from_repair(RepairOutcome::Ready).is_none());
}

#[test]
fn publication_rerun_retains_passed_and_collected_flags_and_project_directory() {
    let fixture = Fixture::new();
    let args = Args {
        owner: Some("team's $value".into()),
        name: Some("shop".into()),
        visibility: Some(Visibility::Private),
    };
    let command = rerun_command(&args);
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("set -- {command}; printf '%s\\n' \"$@\"")])
        .output()
        .unwrap();
    assert!(output.status.success());
    let words = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .skip(3)
        .map(String::from)
        .collect::<Vec<_>>();
    assert_eq!(Args::parse(&words).unwrap(), args);
    assert!(rerun_at(&args, &fixture.root).starts_with("cd '"));
    assert!(rerun_at(&args, &fixture.root).contains(" && slip publish github"));
}
