use super::*;
use crate::setup::{RepairOutcome, SetupPrompts};
use crate::setup_fixture::{quote, Fixture, SCRUBBED};
use std::fs;

#[derive(Default)]
struct Prompts {
    identities: Vec<&'static str>,
    confirmations: Vec<String>,
    answer: Option<bool>,
}
impl SetupPrompts for Prompts {
    async fn identity(
        &mut self,
        field: &'static str,
        _interrupts: &mut Interrupts,
    ) -> io::Result<Option<String>> {
        self.identities.push(field);
        Ok(Some(
            if field == "user.name" {
                "Tester"
            } else {
                "tester@example.com"
            }
            .into(),
        ))
    }
    async fn confirm(
        &mut self,
        _plan: &shipslip::setup::SetupPlan,
        question: &str,
        _interrupts: &mut Interrupts,
    ) -> io::Result<Option<bool>> {
        self.confirmations.push(question.into());
        Ok(self.answer)
    }
}

fn request() -> CreateRequest {
    CreateRequest {
        name: "shop".into(),
        options: InstallerOptions {
            starter_kit: StarterKit::None,
            auth: Auth::None,
            database: Database::Sqlite,
            testing: Testing::Pest,
            boost: false,
        },
        branch: "feature/shop".into(),
    }
}

fn fixture() -> Fixture {
    let fixture = Fixture::new();
    let git = fixture.root.join("sources/git");
    let body = fs::read_to_string(&git).unwrap().replace(
        "  *) exit 99 ;;",
        "  'check-ref-format --branch '*) echo branch ;;\n  *) exit 99 ;;",
    );
    fixture.script(&git, &body);
    fixture.complete();
    fixture
}

fn interrupts() -> Interrupts {
    Interrupts::from_channel(tokio::sync::mpsc::unbounded_channel().1)
}

async fn prepare(
    fixture: &Fixture,
    request: CreateRequest,
    prompts: &mut Prompts,
) -> Result<CreationPreparation, Box<dyn Error>> {
    prepare_creation(
        request,
        &fixture.root,
        &fixture.context,
        fixture.root.join("ops"),
        &mut interrupts(),
        prompts,
    )
    .await
}

#[tokio::test]
async fn invalid_destinations_and_options_fail_before_any_detection_or_setup() {
    let fixture = fixture();
    fs::create_dir(fixture.root.join("existing")).unwrap();
    std::os::unix::fs::symlink(fixture.root.join("missing"), fixture.root.join("link")).unwrap();
    fixture.script(
        &fixture.root.join("brew/bin/php"),
        &format!(
            "echo probe > {}/probe; exit 1",
            quote(&fixture.root.to_string_lossy())
        ),
    );
    for name in ["existing", "link", "nested/shop"] {
        let mut request = request();
        request.name = name.into();
        let mut prompts = Prompts::default();
        assert!(prepare(&fixture, request, &mut prompts).await.is_err());
        assert!(prompts.confirmations.is_empty());
        assert!(!fixture.root.join("probe").exists());
        assert!(!fixture.root.join("ops").exists());
    }
    for bad_options in [true, false] {
        let mut request = request();
        if bad_options {
            request.options.auth = Auth::Laravel;
        } else {
            request.branch = "bad branch".into();
        }
        assert!(prepare(&fixture, request, &mut Prompts::default())
            .await
            .is_err());
        assert!(!fixture.root.join("probe").exists());
    }
}

#[tokio::test]
async fn publish_failures_are_warnings_and_never_enter_the_creation_repair_plan() {
    let fixture = fixture();
    fs::remove_file(fixture.root.join("brew/bin/gh")).unwrap();
    let combined = shipslip::setup::detect(
        &fixture.context,
        &DetectionOptions {
            purposes: vec![Purpose::Create, Purpose::Publish],
            root: fixture.root.clone(),
            installer_options: Some(request().options),
        },
    )
    .await
    .unwrap();
    let warnings = publish_warnings(&combined);
    assert!(warnings.contains("Publishing later will need:"));
    assert!(warnings.contains("gh: missing"));
    assert!(!warnings.contains("Not ready:"));
    let mut prompts = Prompts::default();
    let CreationPreparation::Ready(preview) =
        prepare(&fixture, request(), &mut prompts).await.unwrap()
    else {
        panic!("publishing must not block creation")
    };
    assert_eq!(preview.request.name, "shop");
    assert!(prompts.confirmations.is_empty());
    assert!(!fixture.root.join("actions").exists());
}

#[tokio::test]
async fn missing_node_gets_one_setup_approval_and_retains_all_creation_answers() {
    let fixture = fixture();
    for tool in ["node", "npm", "gh"] {
        fs::remove_file(fixture.root.join("brew/bin").join(tool)).unwrap();
    }
    let mut request = request();
    request.options = InstallerOptions {
        starter_kit: StarterKit::Livewire,
        auth: Auth::Laravel,
        database: Database::Mariadb,
        testing: Testing::Phpunit,
        boost: true,
    };
    let expected = request.clone();
    let mut prompts = Prompts {
        answer: Some(true),
        ..Prompts::default()
    };
    let CreationPreparation::Ready(preview) =
        prepare(&fixture, request, &mut prompts).await.unwrap()
    else {
        panic!("repair must resume creation")
    };
    assert_eq!(
        prompts.confirmations,
        ["Apply setup and continue creating shop?"]
    );
    assert!(prompts.identities.is_empty());
    assert_eq!(preview.request.options, expected.options);
    assert_eq!(preview.request.branch, expected.branch);
    assert_eq!(preview.request.name, expected.name);
    assert_eq!(
        fs::read_to_string(fixture.root.join("actions")).unwrap(),
        "install node\n"
    );
    assert!(!fixture.root.join("brew/bin/gh").exists());
    assert!(preview
        .installer_args
        .iter()
        .any(|arg| arg == "--database=mariadb"));
}

#[tokio::test]
async fn declined_guidance_failed_and_interrupted_repairs_stop_before_creation_preview() {
    for mode in ["decline", "guidance", "failed", "interrupted"] {
        let mut fixture = fixture();
        for tool in ["node", "npm"] {
            fs::remove_file(fixture.root.join("brew/bin").join(tool)).unwrap();
        }
        let mut prompts = Prompts {
            answer: Some(false),
            ..Prompts::default()
        };
        let expected = match mode {
            "guidance" => {
                fixture.context.macos = false;
                RepairOutcome::NotReady
            }
            "failed" => {
                fixture.brew("exit 11");
                prompts.answer = Some(true);
                RepairOutcome::Failed
            }
            "interrupted" => {
                prompts.answer = None;
                RepairOutcome::Interrupted
            }
            _ => RepairOutcome::Declined,
        };
        let CreationPreparation::Stopped(outcome) =
            prepare(&fixture, request(), &mut prompts).await.unwrap()
        else {
            panic!("{mode}: must stop")
        };
        assert_eq!(outcome, expected);
        assert!(!fixture.root.join("shop").exists());
        assert_eq!(prompts.confirmations.len(), usize::from(mode != "guidance"));
    }
}

#[tokio::test]
async fn identity_is_kept_when_in_flow_install_fails() {
    let fixture = fixture();
    for path in ["name", "email", "brew/bin/node", "brew/bin/npm"] {
        fs::remove_file(fixture.root.join(path)).unwrap();
    }
    fixture.brew("exit 11");
    let mut prompts = Prompts {
        answer: Some(true),
        ..Prompts::default()
    };
    assert!(matches!(
        prepare(&fixture, request(), &mut prompts).await.unwrap(),
        CreationPreparation::Stopped(RepairOutcome::Failed)
    ));
    assert_eq!(prompts.identities, ["user.name", "user.email"]);
    assert_eq!(
        fs::read_to_string(fixture.root.join("actions")).unwrap(),
        "name\nemail\n"
    );
    let report = fixture.detect().await;
    assert!(shipslip::setup::plan_setup(
        &fixture.context,
        &report,
        &shipslip::setup::SetupInputs::default()
    )
    .unwrap()
    .inputs_needed
    .is_empty());
}

#[test]
fn creation_rerun_round_trips_every_answer_and_shell_metacharacters() {
    let mut request = request();
    request.name = "my 'app $()".into();
    for boost in [true, false] {
        request.options.boost = boost;
        let command = rerun_command(&request);
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("set -- {command}; printf '%s\\n' \"$@\"")])
            .output()
            .unwrap();
        assert!(output.status.success());
        let words = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .skip(2)
            .map(String::from)
            .collect::<Vec<_>>();
        let parsed = Args::parse(&words).unwrap();
        assert_eq!(parsed.name, request.name);
        assert_eq!(parsed.branch, request.branch);
        assert_eq!(parsed.starter_kit, Some(request.options.starter_kit));
        assert_eq!(parsed.auth, Some(request.options.auth));
        assert_eq!(parsed.database, Some(request.options.database));
        assert_eq!(parsed.testing, Some(request.options.testing));
        assert_eq!(parsed.boost, Some(boost));
    }
}

#[tokio::test]
async fn missing_node_demo_repairs_resumes_and_verifies_the_created_app() {
    let fixture = fixture();
    for tool in ["node", "npm"] {
        fs::remove_file(fixture.root.join("brew/bin").join(tool)).unwrap();
    }
    let laravel = fixture.root.join("home/.composer/vendor/bin/laravel");
    let body = fs::read_to_string(&laravel).unwrap();
    let version_and_help = body.replace("else printf", "elif [ \"$2\" = --help ]; then printf");
    let version_and_help = version_and_help.trim_end().strip_suffix("fi").unwrap();
    fixture.script(&laravel, &format!(r#"
{version_and_help}
else
{SCRUBBED}
project=$2
/bin/mkdir -p "$project/vendor" "$project/public/build/assets"
printf artisan > "$project/artisan"
printf autoload > "$project/vendor/autoload.php"
printf '%s' '{{"name":"laravel/laravel","require":{{"laravel/framework":"^13.0"}}}}' > "$project/composer.json"
printf '%s' '{{"packages":[{{"name":"laravel/framework","version":"v13.34.0"}}],"packages-dev":[{{"name":"pestphp/pest","version":"v4.0.0"}}]}}' > "$project/composer.lock"
printf '%s' '{{"scripts":{{"build":"vite build"}}}}' > "$project/package.json"
printf '{{}}' > "$project/package-lock.json"
printf '%s' '{{"app":{{"file":"assets/app.js"}}}}' > "$project/public/build/manifest.json"
printf built > "$project/public/build/assets/app.js"
fi
"#));
    let php = fixture.root.join("brew/bin/php");
    let body = fs::read_to_string(&php).unwrap().replace(
        "  *) exit 99 ;;",
        &format!("  artisan) {SCRUBBED}\n    printf tested > .test-ran ;;\n  *) exit 99 ;;"),
    );
    fixture.script(&php, &body);
    let mut prompts = Prompts {
        answer: Some(true),
        ..Prompts::default()
    };
    let CreationPreparation::Ready(preview) =
        prepare(&fixture, request(), &mut prompts).await.unwrap()
    else {
        panic!()
    };
    let confirmation = preview.confirm();
    let (_signal, receiver) = watch::channel(false);
    let project = create::execute_create(*preview, confirmation, receiver, |_| {})
        .await
        .unwrap();
    assert!(project.root.join("artisan").exists());
    assert_eq!(
        fs::read_to_string(project.root.join(".test-ran")).unwrap(),
        "tested"
    );
    assert_eq!(prompts.confirmations.len(), 1);
}

struct ProjectQuestions {
    outcome: Option<super::super::publish::PublishOutcome>,
    questions: Vec<String>,
}
impl ProjectPrompts for ProjectQuestions {
    async fn confirm(
        &mut self,
        question: &str,
        _interrupts: &mut Interrupts,
    ) -> io::Result<Option<bool>> {
        self.questions.push(question.into());
        Ok(Some(question == "Publish to GitHub now?"))
    }
    async fn publish(
        &mut self,
        _root: &Path,
        _interrupts: &mut Interrupts,
    ) -> Result<super::super::publish::PublishOutcome, Box<dyn Error>> {
        Ok(self.outcome.take().unwrap())
    }
    async fn server(
        &mut self,
        _root: &Path,
        _interrupts: &mut Interrupts,
    ) -> Result<Option<PathBuf>, Box<dyn Error>> {
        panic!("server was declined")
    }
}

#[tokio::test]
async fn chained_not_ready_and_declined_publish_continue_to_server_and_exit_zero() {
    use super::super::publish::PublishOutcome;
    let fixture = fixture();
    for outcome in [PublishOutcome::NotReady, PublishOutcome::Declined] {
        let mut prompts = ProjectQuestions {
            outcome: Some(outcome),
            questions: vec![],
        };
        let mut interrupts = interrupts();
        assert_eq!(
            finish_local_project(&fixture.root, true, &mut interrupts, &mut prompts)
                .await
                .unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            prompts.questions,
            ["Publish to GitHub now?", "Configure a server now?"]
        );
        assert!(!interrupts.pending);
    }
}

#[tokio::test]
async fn chained_interrupt_stops_before_server_and_exits_130() {
    let fixture = fixture();
    let mut prompts = ProjectQuestions {
        outcome: Some(super::super::publish::PublishOutcome::Interrupted),
        questions: vec![],
    };
    assert_eq!(
        finish_local_project(&fixture.root, true, &mut interrupts(), &mut prompts)
            .await
            .unwrap(),
        ExitCode::from(INTERRUPTED)
    );
    assert_eq!(prompts.questions, ["Publish to GitHub now?"]);
}

#[tokio::test]
async fn failed_chained_repair_preserves_failure_and_stops_before_server() {
    use super::super::publish::PublishOutcome;
    let fixture = fixture();
    for (outcome, code) in [(PublishOutcome::Failed, 1), (PublishOutcome::Invalid, 2)] {
        let mut prompts = ProjectQuestions {
            outcome: Some(outcome),
            questions: vec![],
        };
        assert_eq!(
            finish_local_project(&fixture.root, true, &mut interrupts(), &mut prompts)
                .await
                .unwrap(),
            ExitCode::from(code)
        );
        assert_eq!(prompts.questions, ["Publish to GitHub now?"]);
    }
}
