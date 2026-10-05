use super::*;
use crate::setup as setup_api;
use std::os::unix::fs::PermissionsExt;
use tokio::sync::watch;

#[path = "fixtures/setup_fixture.rs"]
mod fixture;
use fixture::{quote, Fixture, SCRUBBED};

fn node_missing(fixture: &Fixture) {
    for name in ["node", "npm"] {
        fs::remove_file(fixture.root.join("brew/bin").join(name)).unwrap();
    }
}

fn change(report: &mut DetectionReport, id: RequirementId) -> &mut Finding {
    report
        .findings
        .iter_mut()
        .find(|finding| finding.requirement.id == id)
        .unwrap()
}

fn record(outcome: &SetupOutcome) -> serde_json::Value {
    serde_json::from_slice(&fs::read(&outcome.record_path).unwrap()).unwrap()
}

async fn apply(fixture: &Fixture, plan: &SetupPlan) -> SetupOutcome {
    let session = SetupSession::acquire(&fixture.root.join("ops")).unwrap();
    let (_signal, receiver) = watch::channel(false);
    let options = fixture.options();
    let result = session
        .apply(
            plan,
            plan.confirm(),
            SetupEnvironment {
                context: &fixture.context,
                options: &options,
            },
            receiver,
            |_| {},
        )
        .await
        .unwrap();
    let SetupApplyResult::Finished(outcome) = result else {
        panic!("unexpected machine change")
    };
    outcome
}

#[tokio::test]
async fn groups_only_missing_formulae_with_previewed_environment_and_dependencies() {
    let fixture = Fixture::new();
    let report = fixture.detect().await;
    let needs_inputs = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
    assert_eq!(needs_inputs.inputs_needed, ["user.name", "user.email"]);
    let plan = plan_setup(&fixture.context, &report, &fixture.inputs()).unwrap();
    assert_eq!(
        plan.actions[0].args,
        ["install", "php", "composer", "node", "git"]
    );
    assert_eq!(plan.actions[0].environment.len(), 4);
    assert!(plan.actions[0]
        .environment
        .values()
        .all(|value| value == "1"));
    assert_eq!(plan.actions[1].args[2], "user.name");
    assert_eq!(plan.actions[2].args[2], "user.email");
    assert_eq!(
        plan.actions[3].binary,
        fixture.root.join("brew/bin/composer")
    );
    assert!(plan.actions[3].preconditions[0].canonical.is_none());
    assert!(plan
        .actions
        .iter()
        .all(|action| action.kind == ActionKind::Captured && action.runs_as == RunAs::CurrentUser));
}

#[tokio::test]
async fn php_new_does_not_own_node_and_homebrew_node_action_is_planned() {
    let fixture = Fixture::new();
    fixture.complete();
    node_missing(&fixture);
    fs::create_dir_all(fixture.root.join("home/.config/herd-lite/bin")).unwrap();
    let report = fixture.detect().await;
    assert!(report.facts.managers.contains(&Manager::HerdLite));
    assert!(!report
        .facts
        .providers
        .get("node")
        .is_some_and(|providers| providers.contains(&Manager::HerdLite)));
    let plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(plan.actions[0].args, ["install", "node"]);
}

#[tokio::test]
async fn owned_missing_managed_shadowed_and_broken_tools_get_guidance_only() {
    let fixture = Fixture::new();
    fixture.complete();
    let original = fixture.detect().await;
    for owner in [
        Owner::Herd,
        Owner::HerdLite,
        Owner::Asdf,
        Owner::Mise,
        Owner::Nvm,
        Owner::Fnm,
        Owner::Homebrew,
        Owner::System,
        Owner::Unknown,
    ] {
        let mut report = original.clone();
        let node = change(&mut report, RequirementId::Tool("node"));
        node.owner = owner;
        node.state = FindingState::TooOld {
            found: "1".into(),
            need: "2".into(),
        };
        let plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
        assert!(plan.actions.is_empty(), "{owner:?}");
        assert!(!plan.guidance[0].message.is_empty());
    }
    for manager in [Manager::Nvm, Manager::Fnm, Manager::Asdf, Manager::Mise] {
        let mut report = original.clone();
        let node = change(&mut report, RequirementId::Tool("node"));
        node.state = FindingState::Missing;
        node.managers = vec![manager];
        let plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
        assert!(plan.actions.is_empty());
        assert!(!plan.guidance[0].commands.is_empty());
    }
    let mut report = original;
    let node = change(&mut report, RequirementId::Tool("node"));
    node.state = FindingState::Broken {
        path: "shadow/node".into(),
        error: "broken".into(),
    };
    node.shadowing = Some(fixture.root.join("bin/node"));
    let plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
    assert!(plan.actions.is_empty());
    assert!(plan.guidance[0].message.contains("shadows"));
    assert!(!fixture.root.join("actions").exists());
}

#[tokio::test]
async fn broken_nvm_owned_node_is_left_untouched_during_setup() {
    let mut fixture = Fixture::new();
    fixture.complete();
    let node = fixture.root.join("home/.nvm/versions/node/old/bin/node");
    fixture.script(&node, "exit 7");
    let before = fs::read(&node).unwrap();
    fixture
        .context
        .environment
        .insert("PATH".into(), node.parent().unwrap().as_os_str().into());
    let report = fixture.detect().await;
    assert_eq!(
        change(&mut report.clone(), RequirementId::Tool("node")).owner,
        Owner::Nvm
    );
    let plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
    assert!(plan.actions.is_empty());
    assert_eq!(apply(&fixture, &plan).await.exit_code, 3);
    assert_eq!(fs::read(node).unwrap(), before);
    assert!(!fixture.root.join("actions").exists());
}

#[tokio::test]
async fn absent_or_unwritable_brew_and_missing_clt_never_plan_brew_actions() {
    let fixture = Fixture::new();
    fixture.complete();
    node_missing(&fixture);
    let original = fixture.detect().await;
    for mode in ["missing", "unwritable", "clt"] {
        let mut report = original.clone();
        match mode {
            "missing" => report.facts.homebrew = None,
            "unwritable" => report.facts.homebrew.as_mut().unwrap().writable = false,
            _ => report.facts.command_line_tools = Some(false),
        }
        let plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
        assert!(plan.actions.is_empty(), "{mode}");
        let text = serde_json::to_string(&plan.guidance).unwrap();
        assert!(text.contains(match mode {
            "missing" => "Homebrew/install",
            "unwritable" => "ownership",
            _ => "xcode-select --install",
        }));
    }
}

#[tokio::test]
async fn read_only_prefix_is_writable_when_cellar_and_bin_are() {
    let fixture = Fixture::new();
    fixture.complete();
    let prefix = fixture.root.join("brew");
    fs::create_dir(prefix.join("Cellar")).unwrap();
    let mode =
        |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    mode(&prefix, 0o555);
    let intel_layout = fixture.detect().await.facts.homebrew.unwrap().writable;
    mode(&prefix.join("Cellar"), 0o555);
    let cellar_locked = fixture.detect().await.facts.homebrew.unwrap().writable;
    mode(&prefix.join("Cellar"), 0o755);
    mode(&prefix, 0o755);
    assert!(intel_layout);
    assert!(!cellar_locked);
}

#[tokio::test]
async fn linux_setup_remains_guidance_only() {
    let mut fixture = Fixture::new();
    fixture.context.macos = false;
    let report = fixture.detect().await;
    let plan = plan_setup(&fixture.context, &report, &fixture.inputs()).unwrap();
    assert!(plan.actions.is_empty());
    assert!(plan.inputs_needed.is_empty());
    assert!(!plan.guidance.is_empty());

    fixture.complete();
    fs::remove_file(fixture.root.join("email")).unwrap();
    let report = fixture.detect().await;
    let plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
    assert!(plan.actions.is_empty());
    assert!(plan.inputs_needed.is_empty());
    let email = plan
        .guidance
        .iter()
        .find(|guidance| guidance.requirement == Some(RequirementId::Identity("user.email")))
        .unwrap();
    assert_eq!(email.commands, ["git config --global user.email <value>"]);
}

#[tokio::test]
async fn eligibility_hash_ignores_probe_text_and_auth_but_tracks_action_permissions() {
    let fixture = Fixture::new();
    fixture.complete();
    let mut options = fixture.options();
    options.purposes.push(Purpose::Publish);
    let original = detect(&fixture.context, &options).await.unwrap();
    let hash = findings_hash(&fixture.context, &original);
    let mut report = original.clone();
    if let FindingState::Ok { version, .. } =
        &mut change(&mut report, RequirementId::Identity("user.name")).state
    {
        *version = "changed identity text".into();
    }
    change(&mut report, RequirementId::GithubAuth).state = FindingState::Missing;
    assert_eq!(findings_hash(&fixture.context, &report), hash);
    for mode in [
        "providers",
        "writable",
        "shadowing",
        "clt",
        "owner",
        "state",
        "path",
    ] {
        let mut report = original.clone();
        match mode {
            "providers" => {
                report
                    .facts
                    .providers
                    .insert("node".into(), vec![Manager::Nvm]);
            }
            "writable" => report.facts.homebrew.as_mut().unwrap().writable = false,
            "shadowing" => {
                change(&mut report, RequirementId::Tool("node")).shadowing =
                    Some("other/node".into())
            }
            "clt" => report.facts.command_line_tools = Some(false),
            "owner" => change(&mut report, RequirementId::Tool("node")).owner = Owner::Nvm,
            "state" => {
                change(&mut report, RequirementId::Tool("node")).state = FindingState::Missing
            }
            _ => {
                change(&mut report, RequirementId::Tool("node"))
                    .location
                    .as_mut()
                    .unwrap()
                    .path = "other/node".into()
            }
        }
        assert_ne!(findings_hash(&fixture.context, &report), hash, "{mode}");
    }
    let mut broken = original.clone();
    change(&mut broken, RequirementId::Tool("php")).state = FindingState::Broken {
        path: "php".into(),
        error: "one".into(),
    };
    let hash = findings_hash(&fixture.context, &broken);
    change(&mut broken, RequirementId::Tool("php")).state = FindingState::Broken {
        path: "php".into(),
        error: "two".into(),
    };
    assert_eq!(findings_hash(&fixture.context, &broken), hash);
}

#[tokio::test]
async fn homebrew_only_chain_finds_new_php_and_composer_bin_and_rerun_converges() {
    for bin in ["vendor/bin", "custom/bin"] {
        let mut fixture = Fixture::new();
        fixture.sources(bin);
        if bin != "vendor/bin" {
            fs::write(
                fixture.root.join("home/.composer/config.json"),
                r#"{"config":{"bin-dir":"custom/bin"}}"#,
            )
            .unwrap();
        }
        for (index, key) in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "SHIPSLIP_GITHUB_TOKEN",
        ]
        .into_iter()
        .enumerate()
        {
            fixture
                .context
                .environment
                .insert(key.into(), format!("secret-{index}").into());
        }
        let plan =
            plan_setup(&fixture.context, &fixture.detect().await, &fixture.inputs()).unwrap();
        let outcome = apply(&fixture, &plan).await;
        assert_eq!(outcome.exit_code, 0, "{:?}", record(&outcome));
        assert!(fixture
            .root
            .join(format!("home/.composer/{bin}/laravel"))
            .exists());
        assert_eq!(
            fs::read_to_string(fixture.root.join("interpreter"))
                .unwrap()
                .trim(),
            fixture.root.join("brew/bin/php").to_str().unwrap()
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("actions"))
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            ["install php composer node git", "name", "email", "composer"]
        );
        let fresh = fixture.detect().await;
        let rerun = plan_setup(&fixture.context, &fresh, &SetupInputs::default()).unwrap();
        assert!(rerun.actions.is_empty());
        assert!(rerun.inputs_needed.is_empty());
        assert_eq!(apply(&fixture, &rerun).await.exit_code, 0);
        assert_eq!(
            fs::metadata(&outcome.record_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(outcome.record_path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[tokio::test]
async fn composer_action_pins_managed_php_ahead_of_homebrew_php() {
    let mut fixture = Fixture::new();
    fixture.complete();
    fs::remove_file(fixture.root.join("home/.composer/vendor/bin/laravel")).unwrap();
    fs::create_dir_all(fixture.root.join("home/.asdf/plugins/php")).unwrap();
    let managed = fixture.root.join("home/.asdf/installs/php/8/bin/php");
    fixture.copy(&fixture.root.join("sources/php"), &managed);
    fixture.context.environment.insert(
        "PATH".into(),
        std::env::join_paths([managed.parent().unwrap(), &fixture.root.join("bin")]).unwrap(),
    );
    let report = fixture.detect().await;
    assert_eq!(
        change(&mut report.clone(), RequirementId::Tool("php")).owner,
        Owner::Asdf
    );
    let plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(apply(&fixture, &plan).await.exit_code, 0);
    assert_eq!(
        fs::read_to_string(fixture.root.join("interpreter"))
            .unwrap()
            .trim(),
        managed.to_str().unwrap()
    );
}

#[tokio::test]
async fn identity_writes_precede_install_and_survive_later_failure() {
    let fixture = Fixture::new();
    fixture.complete();
    node_missing(&fixture);
    fs::remove_file(fixture.root.join("name")).unwrap();
    fs::remove_file(fixture.root.join("email")).unwrap();
    fixture.brew("echo failed >&2; exit 17");
    let plan = plan_setup(&fixture.context, &fixture.detect().await, &fixture.inputs()).unwrap();
    assert_eq!(plan.actions[0].args[2], "user.name");
    assert_eq!(plan.actions[1].args[2], "user.email");
    let outcome = apply(&fixture, &plan).await;
    assert_eq!(outcome.exit_code, 1);
    assert_eq!(record(&outcome)["actions"][2]["exit_code"], 17);
    let rerun = plan_setup(
        &fixture.context,
        &fixture.detect().await,
        &SetupInputs::default(),
    )
    .unwrap();
    assert!(rerun.inputs_needed.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.root.join("actions")).unwrap(),
        "name\nemail\n"
    );
}

#[tokio::test]
async fn failed_action_stops_dependents_and_records_output_and_exit() {
    let fixture = Fixture::new();
    fixture.brew("echo brew-failed; echo detail >&2; exit 19");
    let plan = plan_setup(&fixture.context, &fixture.detect().await, &fixture.inputs()).unwrap();
    let outcome = apply(&fixture, &plan).await;
    assert_eq!(outcome.exit_code, 1);
    let record = record(&outcome);
    assert_eq!(record["exit_code"], 1);
    assert_eq!(record["actions"][0]["state"], "failed");
    assert_eq!(record["actions"][0]["exit_code"], 19);
    assert!(record["actions"][0]["output"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("brew-failed")));
    assert!(record["actions"].as_array().unwrap()[1..]
        .iter()
        .all(|action| action["state"] == "not_started"));
    assert!(record["final_report"].is_object());
}

#[tokio::test]
async fn failed_precondition_stops_before_mutations_and_returns_one() {
    let fixture = Fixture::new();
    let mut plan =
        plan_setup(&fixture.context, &fixture.detect().await, &fixture.inputs()).unwrap();
    plan.actions[0].preconditions.push(ActionPrecondition {
        path: fixture.root.join("absent"),
        canonical: None,
    });
    let outcome = apply(&fixture, &plan).await;
    assert_eq!(outcome.exit_code, 1);
    assert!(outcome.error.unwrap().contains("failed precondition"));
    assert!(!fixture.root.join("actions").exists());
}

#[test]
fn setup_target_lock_is_shared_and_released_on_drop() {
    let fixture = Fixture::new();
    let root = fixture.root.join("ops");
    let session = SetupSession::acquire(&root).unwrap();
    assert!(matches!(
        SetupSession::acquire(&root),
        Err(SetupError::Busy)
    ));
    drop(session);
    assert!(SetupSession::acquire(&root).is_ok());
}

#[tokio::test]
async fn manager_appearing_after_preview_requests_replan_without_actions() {
    let fixture = Fixture::new();
    fixture.complete();
    node_missing(&fixture);
    let plan = plan_setup(
        &fixture.context,
        &fixture.detect().await,
        &SetupInputs::default(),
    )
    .unwrap();
    fs::create_dir_all(fixture.root.join("home/.nvm")).unwrap();
    let session = SetupSession::acquire(&fixture.root.join("ops")).unwrap();
    let (_signal, receiver) = watch::channel(false);
    let options = fixture.options();
    let result = session
        .apply(
            &plan,
            plan.confirm(),
            SetupEnvironment {
                context: &fixture.context,
                options: &options,
            },
            receiver,
            |_| panic!("no action events before replan"),
        )
        .await
        .unwrap();
    let SetupApplyResult::Changed {
        report,
        record_path,
    } = result
    else {
        panic!("expected replan")
    };
    let new_plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
    assert!(new_plan.actions.is_empty());
    assert!(new_plan
        .guidance
        .iter()
        .any(|guidance| guidance.commands.contains(&"nvm install --lts".into())));
    assert!(!fixture.root.join("actions").exists());
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(record_path).unwrap()).unwrap();
    assert_eq!(record["exit_code"], 3);
}

#[tokio::test]
async fn confirmation_cannot_be_reused_after_action_changes() {
    let fixture = Fixture::new();
    let mut plan =
        plan_setup(&fixture.context, &fixture.detect().await, &fixture.inputs()).unwrap();
    let confirmation = plan.confirm();
    plan.actions[0].args.push("unexpected".into());
    let session = SetupSession::acquire(&fixture.root.join("ops")).unwrap();
    let (_signal, receiver) = watch::channel(false);
    let options = fixture.options();
    assert!(matches!(
        session
            .apply(
                &plan,
                confirmation,
                SetupEnvironment {
                    context: &fixture.context,
                    options: &options
                },
                receiver,
                |_| {}
            )
            .await,
        Err(SetupError::Invalid(_))
    ));
    assert!(!fixture.root.join("actions").exists());
}

#[tokio::test]
async fn interrupt_between_installed_git_identity_writes_asks_only_missing_field() {
    let fixture = Fixture::new();
    let plan = plan_setup(&fixture.context, &fixture.detect().await, &fixture.inputs()).unwrap();
    let session = SetupSession::acquire(&fixture.root.join("ops")).unwrap();
    let (signal, receiver) = watch::channel(false);
    let options = fixture.options();
    let result = session
        .apply(
            &plan,
            plan.confirm(),
            SetupEnvironment {
                context: &fixture.context,
                options: &options,
            },
            receiver,
            |event| {
                if matches!(
                    event,
                    SetupEvent::ActionFinished {
                        index: 1,
                        state: ActionState::Ok,
                        ..
                    }
                ) {
                    signal.send(true).unwrap();
                }
            },
        )
        .await
        .unwrap();
    let SetupApplyResult::Finished(outcome) = result else {
        panic!()
    };
    assert_eq!(outcome.exit_code, 130);
    assert_eq!(record(&outcome)["actions"][2]["state"], "not_started");
    assert!(fixture.root.join("name").exists());
    assert!(!fixture.root.join("email").exists());
    let rerun = plan_setup(
        &fixture.context,
        &fixture.detect().await,
        &SetupInputs::default(),
    )
    .unwrap();
    assert_eq!(rerun.inputs_needed, ["user.email"]);
}

#[tokio::test]
async fn interrupt_after_identity_preserves_both_fields_on_rerun() {
    let fixture = Fixture::new();
    fixture.complete();
    node_missing(&fixture);
    fs::remove_file(fixture.root.join("name")).unwrap();
    fs::remove_file(fixture.root.join("email")).unwrap();
    let plan = plan_setup(&fixture.context, &fixture.detect().await, &fixture.inputs()).unwrap();
    let session = SetupSession::acquire(&fixture.root.join("ops")).unwrap();
    let (signal, receiver) = watch::channel(false);
    let options = fixture.options();
    let result = session
        .apply(
            &plan,
            plan.confirm(),
            SetupEnvironment {
                context: &fixture.context,
                options: &options,
            },
            receiver,
            |event| {
                if matches!(event, SetupEvent::ActionFinished { index: 1, .. }) {
                    signal.send(true).unwrap();
                }
            },
        )
        .await
        .unwrap();
    let SetupApplyResult::Finished(outcome) = result else {
        panic!()
    };
    assert_eq!(outcome.exit_code, 130);
    assert_eq!(record(&outcome)["actions"][2]["state"], "not_started");
    assert!(plan_setup(
        &fixture.context,
        &fixture.detect().await,
        &SetupInputs::default()
    )
    .unwrap()
    .inputs_needed
    .is_empty());
}

#[tokio::test]
async fn trapping_brew_records_interrupt_before_exit_and_returns_130_even_for_zero() {
    let fixture = Fixture::new();
    let root = quote(&fixture.root.to_string_lossy());
    fixture.brew(&format!(r#"
{SCRUBBED}
trap 'echo trapped > {root}/trapped; while [ ! -f {root}/release ]; do /bin/sleep 0.02; done; exit 0' INT
echo $$ > {root}/pid.tmp && /bin/mv {root}/pid.tmp {root}/pid
while :; do /bin/sleep 0.02; done
"#));
    let plan = plan_setup(&fixture.context, &fixture.detect().await, &fixture.inputs()).unwrap();
    let session = SetupSession::acquire(&fixture.root.join("ops")).unwrap();
    let (signal, receiver) = watch::channel(false);
    let send = async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !fixture.root.join("pid").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let pid: i32 = fs::read_to_string(fixture.root.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        signal.send(true).unwrap();
        // SAFETY: this PID belongs to the test's live fake Homebrew child.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    };
    let options = fixture.options();
    let mut path = None;
    let work = session.apply(
        &plan,
        plan.confirm(),
        SetupEnvironment {
            context: &fixture.context,
            options: &options,
        },
        receiver,
        |event| match event {
            SetupEvent::Record { path: record } => path = Some(record),
            SetupEvent::Interrupted { index: 0 } => {
                let record: serde_json::Value =
                    serde_json::from_slice(&fs::read(path.as_ref().unwrap()).unwrap()).unwrap();
                assert_eq!(record["actions"][0]["state"], "interrupted");
                assert_eq!(record["exit_code"], 130);
                fs::write(fixture.root.join("release"), "go").unwrap();
            }
            _ => {}
        },
    );
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(work, send)
    })
    .await
    .unwrap();
    let SetupApplyResult::Finished(outcome) = result.unwrap() else {
        panic!()
    };
    assert_eq!(outcome.exit_code, 130);
    assert!(fixture.root.join("trapped").exists());
    let record = record(&outcome);
    assert_eq!(record["actions"][0]["exit_code"], 0);
    assert!(record["actions"].as_array().unwrap()[1..]
        .iter()
        .all(|action| action["state"] == "not_started"));
}

#[tokio::test]
async fn captured_output_is_bounded_and_all_context_tokens_are_redacted() {
    let mut fixture = Fixture::new();
    fixture.complete();
    node_missing(&fixture);
    fixture
        .context
        .environment
        .insert("GH_TOKEN".into(), "test-secret".into());
    fixture.brew(&format!("{SCRUBBED}\ni=0; while [ $i -lt 205 ]; do echo test-secret-line-$i; i=$((i+1)); done; exit 9"));
    let plan = plan_setup(
        &fixture.context,
        &fixture.detect().await,
        &SetupInputs::default(),
    )
    .unwrap();
    let outcome = apply(&fixture, &plan).await;
    let record = record(&outcome);
    assert_eq!(
        record["actions"][0]["output"].as_array().unwrap().len(),
        200
    );
    assert!(!fs::read_to_string(outcome.record_path)
        .unwrap()
        .contains("test-secret"));
}

#[tokio::test]
async fn attached_action_inherits_streams_and_emits_no_output_events() {
    let fixture = Fixture::new();
    fixture.complete();
    let mut plan = plan_setup(
        &fixture.context,
        &fixture.detect().await,
        &SetupInputs::default(),
    )
    .unwrap();
    let binary = fixture.root.join("bin/attached");
    fixture.script(&binary, "echo attached-output");
    let mut attached = action(binary, vec![], vec![]);
    attached.kind = ActionKind::Attached;
    plan.actions.push(attached);
    let session = SetupSession::acquire(&fixture.root.join("ops")).unwrap();
    let (_signal, receiver) = watch::channel(false);
    let options = fixture.options();
    let mut events = Vec::new();
    let result = session
        .apply(
            &plan,
            plan.confirm(),
            SetupEnvironment {
                context: &fixture.context,
                options: &options,
            },
            receiver,
            |event| events.push(event),
        )
        .await
        .unwrap();
    let SetupApplyResult::Finished(outcome) = result else {
        panic!()
    };
    assert_eq!(outcome.exit_code, 0);
    assert!(!events
        .iter()
        .any(|event| matches!(event, SetupEvent::Output { .. })));
    assert_eq!(
        record(&outcome)["actions"][0]["output"],
        serde_json::json!([])
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SetupEvent::ActionFinished {
            exit_code: Some(0),
            ..
        }
    )));
}

#[test]
fn native_path_bytes_are_preserved_in_hashes_and_confirmation() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let mut report = DetectionReport {
        findings: vec![],
        facts: MachineFacts::default(),
        warnings: vec![],
    };
    let first = PathBuf::from(OsString::from_vec(b"/tmp/\xff".to_vec()));
    let second = PathBuf::from(OsString::from_vec(b"/tmp/\xfe".to_vec()));
    assert_eq!(first.to_string_lossy(), second.to_string_lossy());
    report.facts.composer_bin = Some(first.clone());
    let hash = findings_hash(&fixture.context, &report);
    report.facts.composer_bin = Some(second.clone());
    assert_ne!(findings_hash(&fixture.context, &report), hash);
    let mut plan = plan_setup(&fixture.context, &report, &SetupInputs::default()).unwrap();
    plan.path_additions = vec![first];
    let hash = plan.fingerprint();
    plan.path_additions = vec![second];
    assert_ne!(plan.fingerprint(), hash);
}

#[test]
fn setup_events_match_version_one_golden_contract() {
    let events = [
        SetupEvent::Record {
            path: "/fixture/ops/setup/run.json".into(),
        },
        SetupEvent::ActionStarted { index: 0 },
        SetupEvent::Output {
            index: 0,
            line: "brew output".into(),
        },
        SetupEvent::Interrupted { index: 0 },
        SetupEvent::ActionFinished {
            index: 0,
            exit_code: Some(0),
            state: ActionState::Interrupted,
        },
        SetupEvent::ActionFinished {
            index: 1,
            exit_code: None,
            state: ActionState::Failed,
        },
        SetupEvent::Finished { exit_code: 130 },
    ];
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/setup-events-v1.json")).unwrap();
    assert_eq!(serde_json::to_value(events).unwrap(), golden);
}
