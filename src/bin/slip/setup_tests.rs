use super::*;
use std::fs;

use super::super::setup_fixture as fixture;
use fixture::Fixture;

#[derive(Default)]
struct Prompts {
    asked: Vec<&'static str>,
    confirmations: usize,
    approve: Vec<bool>,
    changed_home: Option<std::path::PathBuf>,
    lock_root: Option<std::path::PathBuf>,
}
impl SetupPrompts for Prompts {
    async fn identity(
        &mut self,
        field: &'static str,
        _interrupts: &mut Interrupts,
    ) -> io::Result<Option<String>> {
        self.asked.push(field);
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
        plan: &setup::SetupPlan,
        _question: &str,
        _interrupts: &mut Interrupts,
    ) -> io::Result<Option<bool>> {
        assert!(plan.inputs_needed.is_empty());
        self.confirmations += 1;
        if self.confirmations == 1 {
            if let Some(home) = &self.changed_home {
                fs::create_dir_all(home.join(".nvm"))?;
            }
        } else if let Some(root) = &self.lock_root {
            assert!(matches!(
                setup::SetupSession::acquire(root),
                Err(setup::SetupError::Busy)
            ));
        }
        Ok(Some(self.approve.remove(0)))
    }
}

async fn run(fixture: &Fixture, prompts: &mut Prompts) -> ExitCode {
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    run_setup_flow(
        RepairArgs {
            purpose: Some(Purpose::Create),
        },
        &fixture.context,
        fixture.root.clone(),
        fixture.root.join("ops"),
        &mut Interrupts::from_channel(receiver),
        prompts,
    )
    .await
    .unwrap()
}

fn records(fixture: &Fixture) -> Vec<serde_json::Value> {
    fs::read_dir(fixture.root.join("ops/setup"))
        .unwrap()
        .map(|entry| serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap())
        .collect()
}

#[test]
fn setup_parser_accepts_only_create_and_publish() {
    assert_eq!(RepairArgs::parse(&[]).unwrap().purpose, None);
    for purpose in ["create", "publish"] {
        assert!(RepairArgs::parse(&[format!("--for={purpose}")])
            .unwrap()
            .purpose
            .is_some());
        assert!(RepairArgs::parse(&["--for".into(), purpose.into()]).is_ok());
    }
    for args in [
        vec!["--json"],
        vec!["--for=deploy"],
        vec!["--for"],
        vec!["--for=create", "--for=publish"],
        vec!["--yes"],
    ] {
        assert!(
            RepairArgs::parse(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err()
        );
    }
}

#[tokio::test]
async fn declined_preview_writes_only_audit_record_and_never_runs_tools_or_config() {
    let fixture = Fixture::new();
    let mut prompts = Prompts {
        approve: vec![false],
        ..Prompts::default()
    };
    assert_eq!(run(&fixture, &mut prompts).await, ExitCode::from(3));
    assert_eq!(prompts.asked, ["user.name", "user.email"]);
    assert_eq!(prompts.confirmations, 1);
    assert!(!fixture.root.join("actions").exists());
    assert!(!fixture.root.join("name").exists());
    assert!(!fixture.root.join("email").exists());
    assert!(!fixture.root.join("brew/bin/php").exists());
    let record = records(&fixture).remove(0);
    assert_eq!(record["exit_code"], 3);
    assert!(record["actions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|action| action["state"] == "not_started"));
}

#[tokio::test]
async fn guidance_only_and_off_path_ready_reports_never_ask_confirmation() {
    let mut fixture = Fixture::new();
    fixture.complete();
    let report = fixture.detect().await;
    assert!(report
        .findings
        .iter()
        .any(|finding| matches!(finding.state, FindingState::OffPath { .. })));
    let mut prompts = Prompts::default();
    assert_eq!(run(&fixture, &mut prompts).await, ExitCode::SUCCESS);
    assert_eq!(prompts.confirmations, 0);
    assert!(prompts.asked.is_empty());
    fixture.context.macos = false;
    for name in ["node", "npm"] {
        fs::remove_file(fixture.root.join("brew/bin").join(name)).unwrap();
    }
    assert_eq!(run(&fixture, &mut prompts).await, ExitCode::from(3));
    assert_eq!(prompts.confirmations, 0);
}

#[tokio::test]
async fn changed_machine_gets_new_preview_and_confirmation_without_reasking_inputs() {
    let fixture = Fixture::new();
    fixture.complete();
    for name in ["node", "npm"] {
        fs::remove_file(fixture.root.join("brew/bin").join(name)).unwrap();
    }
    fs::remove_file(fixture.root.join("home/.composer/vendor/bin/laravel")).unwrap();
    fs::remove_file(fixture.root.join("name")).unwrap();
    fs::remove_file(fixture.root.join("email")).unwrap();
    let mut prompts = Prompts {
        approve: vec![true, false],
        changed_home: Some(fixture.root.join("home")),
        lock_root: Some(fixture.root.join("ops")),
        ..Prompts::default()
    };
    assert_eq!(run(&fixture, &mut prompts).await, ExitCode::from(3));
    assert_eq!(prompts.confirmations, 2);
    assert_eq!(prompts.asked, ["user.name", "user.email"]);
    assert!(!fixture.root.join("actions").exists());
    assert_eq!(records(&fixture).len(), 2);
    assert!(records(&fixture)
        .iter()
        .all(|record| record["exit_code"] == 3));
}

#[tokio::test]
async fn held_setup_lock_exits_one_and_records_reason_without_changes() {
    let fixture = Fixture::new();
    let _lock = setup::SetupSession::acquire(&fixture.root.join("ops")).unwrap();
    let mut prompts = Prompts {
        approve: vec![true],
        ..Prompts::default()
    };
    assert_eq!(run(&fixture, &mut prompts).await, ExitCode::from(1));
    assert!(!fixture.root.join("actions").exists());
    let record = records(&fixture).remove(0);
    assert_eq!(record["exit_code"], 1);
    assert!(record["error"]
        .as_str()
        .unwrap()
        .contains("another slip setup is running"));
}

#[tokio::test]
async fn approved_setup_rechecks_and_returns_ready() {
    let fixture = Fixture::new();
    let mut prompts = Prompts {
        approve: vec![true],
        ..Prompts::default()
    };
    assert_eq!(run(&fixture, &mut prompts).await, ExitCode::SUCCESS);
    assert_eq!(prompts.confirmations, 1);
    assert_eq!(records(&fixture)[0]["exit_code"], 0);
    assert!(fixture.detect().await.ready());
    assert!(!fixture.inputs().name.unwrap().is_empty());
}

#[tokio::test]
async fn preview_shows_exact_commands_environment_privilege_and_path_additions() {
    let fixture = Fixture::new();
    let plan =
        setup::plan_setup(&fixture.context, &fixture.detect().await, &fixture.inputs()).unwrap();
    let text = render_plan(&plan);
    for key in [
        "HOMEBREW_NO_AUTO_UPDATE",
        "HOMEBREW_NO_INSTALL_UPGRADE",
        "HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK",
        "HOMEBREW_NO_INSTALL_CLEANUP",
    ] {
        assert!(text.contains(&format!("{key}='1'")));
    }
    assert!(text.contains("own dependencies"));
    assert!(text.contains("Runs as you; output shown below"));
    assert!(text.contains("Requires executable:"));
    assert!(text.contains("PATH additions:"));
    assert!(text.contains(
        fixture
            .root
            .join("home/.composer/vendor/bin")
            .to_str()
            .unwrap()
    ));
    assert!(text.contains("'global' 'require' 'laravel/installer'"));
}

#[tokio::test]
async fn prompt_failures_preserve_exit_codes_and_do_not_run_actions() {
    let fixture = Fixture::new();
    let report = fixture.detect().await;
    let plan = setup::plan_setup(&fixture.context, &report, &fixture.inputs()).unwrap();
    for (kind, expected) in [
        (io::ErrorKind::UnexpectedEof, 1),
        (io::ErrorKind::InvalidInput, 2),
    ] {
        let code = finish_prompt_error(
            "slip setup",
            &fixture.root.join("ops"),
            &plan,
            &report,
            io::Error::new(kind, "prompt failed"),
        )
        .unwrap();
        assert_eq!(code.exit_code(), ExitCode::from(expected));
    }
    let records = records(&fixture);
    assert!(records.iter().any(|record| record["exit_code"] == 1));
    assert!(records.iter().any(|record| record["exit_code"] == 2));
    assert!(!fixture.root.join("actions").exists());
}

#[tokio::test]
async fn frontend_first_interrupt_records_child_that_traps_and_exits_zero_as_130() {
    let fixture = Fixture::new();
    let root = fixture::quote(&fixture.root.to_string_lossy());
    fixture.brew(&format!(
        r#"
trap 'exit 0' INT
echo $$ > {root}/pid
while :; do /bin/sleep 0.02; done
"#
    ));
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut interrupts = Interrupts::from_channel(receiver);
    let mut prompts = Prompts {
        approve: vec![true],
        ..Prompts::default()
    };
    let work = run_setup_flow(
        RepairArgs {
            purpose: Some(Purpose::Create),
        },
        &fixture.context,
        fixture.root.clone(),
        fixture.root.join("ops"),
        &mut interrupts,
        &mut prompts,
    );
    let send = async {
        while !fixture.root.join("pid").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let pid: i32 = fs::read_to_string(fixture.root.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        sender.send(()).unwrap();
        // SAFETY: this is the running fake Homebrew child created by this test.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(work, send)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap(), ExitCode::from(130));
    let record = records(&fixture).remove(0);
    assert_eq!(record["exit_code"], 130);
    assert_eq!(record["actions"][0]["state"], "interrupted");
    assert_eq!(record["actions"][0]["exit_code"], 0);
    assert!(record["actions"].as_array().unwrap()[1..]
        .iter()
        .all(|action| action["state"] == "not_started"));
}

#[tokio::test]
async fn second_interrupt_exits_130_after_engine_observes_the_first() {
    const CASE: &str = "SHIPSLIP_SETUP_SECOND_INTERRUPT";
    if let Some(marker) = std::env::var_os(CASE) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender.send(()).unwrap();
        sender.send(()).unwrap();
        let mut interrupts = Interrupts::from_channel(receiver);
        let (signal, mut state) = tokio::sync::watch::channel(false);
        let work = async {
            state.changed().await.unwrap();
            fs::write(marker, "interrupted").unwrap();
            std::future::pending::<()>().await;
        };
        interrupts
            .defer_with(work, "", || {
                signal.send(true).unwrap();
            })
            .await;
        panic!("the second interrupt must exit");
    }
    let fixture = Fixture::new();
    let marker = fixture.root.join("first-interrupt");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "setup::tests::second_interrupt_exits_130_after_engine_observes_the_first",
            "--nocapture",
        ])
        .env(CASE, &marker)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert_eq!(fs::read_to_string(marker).unwrap(), "interrupted");
}
