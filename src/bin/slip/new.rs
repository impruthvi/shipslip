use std::error::Error;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use shipslip::create::{self, CreateError, CreateEvent, CreateRequest};
use shipslip::git::Git;
use shipslip::laravel::{Auth, Database, InstallerOptions, StarterKit, Testing};
use shipslip::logs::escape;
use shipslip::setup::{DetectionContext, DetectionOptions, DetectionReport, FindingState, Purpose};
use tokio::sync::watch;

use super::{ask_typed, ask_value, ask_yes_no, invalid_input, Interrupts, INTERRUPTED};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Args {
    pub name: String,
    pub starter_kit: Option<StarterKit>,
    pub auth: Option<Auth>,
    pub database: Option<Database>,
    pub testing: Option<Testing>,
    pub branch: String,
    pub boost: Option<bool>,
}

impl Args {
    pub(super) fn parse(args: &[String]) -> Result<Self, Box<dyn Error>> {
        let mut result = Self {
            name: String::new(),
            starter_kit: None,
            auth: None,
            database: None,
            testing: None,
            branch: "main".into(),
            boost: None,
        };
        let mut branch_seen = false;
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if matches!(arg.as_str(), "--boost" | "--no-boost") {
                if result.boost.is_some() {
                    return Err(
                        invalid_input("pass only one of --boost or --no-boost, once").into(),
                    );
                }
                result.boost = Some(arg == "--boost");
                index += 1;
                continue;
            }
            if arg.starts_with("--") {
                let (flag, inline) = arg
                    .split_once('=')
                    .map_or((arg.as_str(), None), |(flag, value)| (flag, Some(value)));
                if !matches!(
                    flag,
                    "--starter-kit" | "--auth" | "--database" | "--testing" | "--branch"
                ) {
                    return Err(invalid_input(format!("unknown new option {flag}")).into());
                }
                let value = match inline {
                    Some(value) => value,
                    None => {
                        index += 1;
                        args.get(index)
                            .filter(|value| !value.starts_with("--"))
                            .ok_or_else(|| invalid_input(format!("{flag} requires a value")))?
                    }
                };
                match flag {
                    "--starter-kit" => set(&mut result.starter_kit, value.parse()?, flag)?,
                    "--auth" => set(&mut result.auth, value.parse()?, flag)?,
                    "--database" => set(&mut result.database, value.parse()?, flag)?,
                    "--testing" => set(&mut result.testing, value.parse()?, flag)?,
                    "--branch" => {
                        if branch_seen {
                            return Err(invalid_input("--branch was provided twice").into());
                        }
                        result.branch = value.into();
                        branch_seen = true;
                    }
                    _ => unreachable!(),
                }
            } else if result.name.is_empty() && !arg.starts_with('-') {
                result.name = arg.into();
            } else {
                return Err(invalid_input("new accepts exactly one project name or .").into());
            }
            index += 1;
        }
        if result.name.is_empty() {
            return Err(invalid_input(
                "usage: slip new <name|.> [--starter-kit none|react|vue|svelte|livewire]",
            )
            .into());
        }
        if result.starter_kit == Some(StarterKit::None) && result.auth == Some(Auth::Laravel) {
            return Err(invalid_input("--starter-kit none requires --auth none").into());
        }
        if !shipslip::config::is_branch_name(&result.branch) {
            return Err(invalid_input("invalid --branch").into());
        }
        Ok(result)
    }
    fn missing(&self) -> Option<&'static str> {
        if self.starter_kit.is_none() {
            return Some("--starter-kit");
        }
        if self.starter_kit != Some(StarterKit::None) && self.auth.is_none() {
            return Some("--auth");
        }
        if self.database.is_none() {
            return Some("--database");
        }
        if self.testing.is_none() {
            return Some("--testing");
        }
        if self.boost.is_none() {
            return Some("--boost or --no-boost");
        }
        None
    }
}

fn set<T>(slot: &mut Option<T>, value: T, flag: &str) -> io::Result<()> {
    if slot.is_some() {
        return Err(invalid_input(format!("{flag} was provided twice")));
    }
    *slot = Some(value);
    Ok(())
}

pub(super) async fn yes_no(
    question: &str,
    default: bool,
    interrupts: &mut Interrupts,
) -> io::Result<Option<bool>> {
    let question = question.to_string();
    let answer = ask_typed(
        move || {
            ask_yes_no(
                &mut io::stdin().lock(),
                &mut io::stdout(),
                &question,
                default,
            )
        },
        interrupts,
    )
    .await?;
    Ok(Some(answer.unwrap_or_else(|| super::exit_interrupted())))
}

async fn choice<T>(
    value: Option<T>,
    question: &str,
    default: &str,
    interrupts: &mut Interrupts,
) -> Result<Option<T>, Box<dyn Error>>
where
    T: FromStr + Send + 'static,
    T::Err: Error + 'static,
{
    if let Some(value) = value {
        return Ok(Some(value));
    }
    let question = question.to_string();
    let default = default.to_string();
    let value = ask_typed(
        move || {
            ask_value(
                &mut io::stdin().lock(),
                &mut io::stdout(),
                &question,
                Some(&default),
                |value| {
                    T::from_str(value)
                        .map(|_| ())
                        .map_err(|_| "Choose one of the listed values.")
                },
            )
        },
        interrupts,
    )
    .await?;
    Ok(Some(
        value
            .unwrap_or_else(|| super::exit_interrupted())
            .parse()
            .map_err(Box::<dyn Error>::from)?,
    ))
}

pub(super) async fn run(args: Args) -> Result<ExitCode, Box<dyn Error>> {
    if !io::stdin().is_terminal() {
        if let Some(flag) = args.missing() {
            return Err(
                invalid_input(format!("stdin is not a terminal; pass {flag} explicitly")).into(),
            );
        }
        return Err(invalid_input(
            "new needs an interactive terminal to review and confirm creation",
        )
        .into());
    }
    let mut interrupts = Interrupts::listen();
    let Some(kit) = choice(
        args.starter_kit,
        "Starter kit (none/react/vue/svelte/livewire)",
        "none",
        &mut interrupts,
    )
    .await?
    else {
        return Ok(ExitCode::from(INTERRUPTED));
    };
    if kit == StarterKit::None && args.auth == Some(Auth::Laravel) {
        return Err(invalid_input("--starter-kit none requires --auth none").into());
    }
    let auth = if kit == StarterKit::None {
        Auth::None
    } else {
        let Some(auth) = choice(
            args.auth,
            "Authentication (laravel/none)",
            "laravel",
            &mut interrupts,
        )
        .await?
        else {
            return Ok(ExitCode::from(INTERRUPTED));
        };
        auth
    };
    let Some(database) = choice(
        args.database,
        "Database (sqlite/mysql/mariadb/pgsql/sqlsrv)",
        "sqlite",
        &mut interrupts,
    )
    .await?
    else {
        return Ok(ExitCode::from(INTERRUPTED));
    };
    let Some(testing) = choice(
        args.testing,
        "Testing (pest/phpunit)",
        "pest",
        &mut interrupts,
    )
    .await?
    else {
        return Ok(ExitCode::from(INTERRUPTED));
    };
    let boost = match args.boost {
        Some(boost) => boost,
        None => yes_no(
            "Install Laravel Boost? (recommended)",
            true,
            &mut interrupts,
        )
        .await?
        .unwrap_or_else(|| super::exit_interrupted()),
    };
    let request = CreateRequest {
        name: args.name,
        options: InstallerOptions {
            starter_kit: kit,
            auth,
            database,
            testing,
            boost,
        },
        branch: args.branch,
    };
    let rerun = rerun_command(&request);
    let preview = match prepare_creation(
        request,
        &std::env::current_dir()?,
        &DetectionContext::from_environment(),
        create::default_operations_root()?,
        &mut interrupts,
        &mut super::setup::TerminalPrompts,
    )
    .await
    {
        Ok(CreationPreparation::Ready(preview)) => preview,
        Ok(CreationPreparation::Stopped(outcome)) => {
            if outcome == super::setup::RepairOutcome::Interrupted {
                super::exit_interrupted();
            }
            return Ok(outcome.exit_code());
        }
        Err(error) => {
            println!("Run `{rerun}` again to resume.");
            return Err(error);
        }
    };
    println!("\nCreate Laravel project");
    println!(
        "Destination: {}",
        escape(&preview.destination.path.display().to_string())
    );
    println!("Starter kit: {kit}; auth: {auth}; database: {database}; testing: {testing}");
    println!(
        "Branch: {}; Boost: {}",
        escape(&preview.request.branch),
        preview.request.options.boost
    );
    for (tool, version) in &preview.versions {
        println!("  {tool}: {}", escape(version));
    }
    let command = std::iter::once(preview.tools_path())
        .chain(preview.installer_args.iter().map(|value| value.as_os_str()))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");
    println!("Installer: {command}");
    println!("ShipSlip will verify the scaffold and review files before the initial Git commit.");
    if yes_no("Create this local project?", false, &mut interrupts).await? != Some(true) {
        println!("Creation cancelled; nothing written.");
        return Ok(ExitCode::SUCCESS);
    }
    let confirmation = preview.confirm();
    let preview = *preview;
    let branch = preview.request.branch.clone();
    let (cancel, receiver) = watch::channel(false);
    let creating = create::execute_create(preview, confirmation, receiver, |event| match event {
        CreateEvent::Started(stage) => println!("\n{}…", escape(&stage)),
        CreateEvent::Output(line) => println!("  {}", escape(&line)),
        CreateEvent::Verified(version) => {
            println!("Laravel {}: scaffold and tests verified.", escape(&version))
        }
        CreateEvent::Moved(path) => {
            println!("Project created at {}", escape(&path.display().to_string()))
        }
        CreateEvent::Journal(path) => {
            println!("Operation record: {}", escape(&path.display().to_string()))
        }
    });
    tokio::pin!(creating);
    let mut progress = tokio::time::interval(Duration::from_secs(30));
    progress.tick().await;
    let result = loop {
        tokio::select! {
            result = &mut creating => break result,
            () = interrupts.recv() => {
                interrupts.pending = true;
                let _ = cancel.send(true);
                eprintln!("\nCancelling creation and stopping installer processes…");
            }
            _ = progress.tick() => println!("Still working; dependency downloads can take a few minutes…"),
        }
    };
    let project = match result {
        Ok(project) => project,
        Err(error) => {
            eprintln!("slip: {}", escape(&error.to_string()));
            println!("Run `{rerun}` again to resume creation.");
            let cancelled = matches!(error, CreateError::Cancelled(_));
            let staging = match &error {
                CreateError::Cancelled(path) => Some(path),
                CreateError::ScaffoldFailed { stage, staging, .. } if stage != "move" => {
                    Some(staging)
                }
                _ => None,
            };
            if let Some(staging) = staging {
                if yes_no(
                    "Delete the failed operation's staging folder?",
                    false,
                    &mut interrupts,
                )
                .await?
                    == Some(true)
                {
                    create::remove_owned_staging(staging)?;
                    println!("Removed staging folder; the operation record is retained.");
                }
            }
            return Ok(if cancelled {
                ExitCode::from(INTERRUPTED)
            } else {
                ExitCode::FAILURE
            });
        }
    };
    finish_created_project(project, &branch, &mut interrupts).await
}

fn rerun_command(request: &CreateRequest) -> String {
    format!(
        "slip new {} --starter-kit {} --auth {} --database {} --testing {} --branch {} {}",
        shell_quote(request.name.as_ref()),
        request.options.starter_kit,
        request.options.auth,
        request.options.database,
        request.options.testing,
        shell_quote(request.branch.as_ref()),
        if request.options.boost {
            "--boost"
        } else {
            "--no-boost"
        }
    )
}

fn publish_warnings(report: &DetectionReport) -> String {
    let mut lines = vec!["Publishing later will need:".into()];
    for finding in report.findings.iter().filter(|finding| {
        finding.requirement.purposes.contains(&Purpose::Publish)
            && !matches!(finding.state, FindingState::Ok { .. })
    }) {
        lines.push(format!(
            "  {}: {}",
            finding.requirement.id,
            escape(&super::setup::render_state(&finding.state))
        ));
        if let Some(detail) = &finding.detail {
            lines.push(format!("    {}", escape(detail)));
        }
    }
    if lines.len() == 1 {
        String::new()
    } else {
        lines.join("\n")
    }
}

enum CreationPreparation {
    Ready(Box<create::CreatePreview>),
    Stopped(super::setup::RepairOutcome),
}

async fn prepare_creation(
    request: CreateRequest,
    start: &Path,
    context: &DetectionContext,
    operations_root: PathBuf,
    interrupts: &mut Interrupts,
    prompts: &mut impl super::setup::SetupPrompts,
) -> Result<CreationPreparation, Box<dyn Error>> {
    request.check(start)?;
    let options = DetectionOptions {
        purposes: vec![Purpose::Create, Purpose::Publish],
        root: start.into(),
        installer_options: Some(request.options.clone()),
    };
    let notice = format!("Creation checks were interrupted; waiting for the current check. Press Ctrl-C again to exit immediately. Resume with `{}`.", rerun_command(&request));
    let report = interrupts
        .defer(shipslip::setup::detect(context, &options), &notice)
        .await?;
    let warnings = publish_warnings(&report);
    let mut creation = report;
    creation
        .findings
        .retain(|finding| finding.requirement.purposes.contains(&Purpose::Create));
    for finding in &mut creation.findings {
        finding.requirement.purposes = vec![Purpose::Create];
    }
    let repair = super::setup::RepairRequest {
        options: DetectionOptions {
            purposes: vec![Purpose::Create],
            ..options
        },
        operations_root: operations_root.clone(),
        report: Some(creation),
        heading: Some("Missing for project creation"),
        warnings,
        confirmation: format!(
            "Apply setup and continue creating {}?",
            escape(&request.name)
        ),
        rerun: rerun_command(&request),
    };
    let outcome = super::setup::repair_with_prompts(repair, context, interrupts, prompts).await?;
    if outcome != super::setup::RepairOutcome::Ready {
        return Ok(CreationPreparation::Stopped(outcome));
    }
    let tools = shipslip::setup::detect_tools(
        context,
        &["php", "composer", "laravel", "git", "node", "npm"],
    )
    .creation_tools()?;
    Ok(CreationPreparation::Ready(Box::new(
        create::preview(request, start, tools, operations_root).await?,
    )))
}

pub(super) async fn finish_created_project(
    project: create::CreatedProject,
    branch: &str,
    interrupts: &mut Interrupts,
) -> Result<ExitCode, Box<dyn Error>> {
    let root = project.root.clone();
    let result = finish_created_project_inner(project, branch, interrupts).await;
    if result.is_err() {
        eprintln!("{}", retained_project_guidance(&root));
    }
    result
}

async fn finish_created_project_inner(
    mut project: create::CreatedProject,
    branch: &str,
    interrupts: &mut Interrupts,
) -> Result<ExitCode, Box<dyn Error>> {
    let git = Git::new(project.git.clone());
    git.init(&project.root, branch)?;
    let identity = git.identity(&project.root)?;
    let files = git.stage_initial(&project.root)?;
    let tree = git.run(&project.root, &["write-tree"])?;
    println!(
        "\nInitial commit on {} as {} <{}>:",
        escape(branch),
        escape(&identity.name),
        escape(&identity.email)
    );
    for file in files {
        println!("  {}", escape(&file));
    }
    let committed = yes_no("Create this initial commit?", false, interrupts).await? == Some(true);
    if committed {
        let head = git.commit_initial_reviewed(&project.root, branch, tree.trim(), &identity)?;
        project.mark_committed(head.clone())?;
        println!("Initial commit: {}", escape(&head));
    } else {
        println!("Initial commit skipped; files remain staged. Review and commit them before publishing.");
    }
    let root = project.root.clone();
    drop(project);
    finish_local_project(&root, committed, interrupts, &mut TerminalProjectPrompts).await
}

trait ProjectPrompts {
    async fn confirm(
        &mut self,
        question: &str,
        interrupts: &mut Interrupts,
    ) -> io::Result<Option<bool>>;
    async fn publish(
        &mut self,
        root: &Path,
        interrupts: &mut Interrupts,
    ) -> Result<super::publish::PublishOutcome, Box<dyn Error>>;
    async fn server(
        &mut self,
        root: &Path,
        interrupts: &mut Interrupts,
    ) -> Result<Option<PathBuf>, Box<dyn Error>>;
}
struct TerminalProjectPrompts;
impl ProjectPrompts for TerminalProjectPrompts {
    async fn confirm(
        &mut self,
        question: &str,
        interrupts: &mut Interrupts,
    ) -> io::Result<Option<bool>> {
        yes_no(question, false, interrupts).await
    }
    async fn publish(
        &mut self,
        root: &Path,
        interrupts: &mut Interrupts,
    ) -> Result<super::publish::PublishOutcome, Box<dyn Error>> {
        super::publish::run_at(super::publish::Args::default(), root, interrupts).await
    }
    async fn server(
        &mut self,
        root: &Path,
        interrupts: &mut Interrupts,
    ) -> Result<Option<PathBuf>, Box<dyn Error>> {
        configure_server(root, interrupts).await
    }
}

async fn finish_local_project(
    root: &Path,
    committed: bool,
    interrupts: &mut Interrupts,
    prompts: &mut impl ProjectPrompts,
) -> Result<ExitCode, Box<dyn Error>> {
    println!(
        "\nLocal project verified: {}",
        escape(&root.display().to_string())
    );
    println!(
        "Run locally:\n  cd {}\n  composer run dev",
        shell_quote(root.as_os_str())
    );
    let mut published = None;
    if committed
        && prompts
            .confirm("Publish to GitHub now?", interrupts)
            .await?
            == Some(true)
    {
        let outcome = prompts.publish(root, interrupts).await?;
        published = match chained_publication(root, outcome, interrupts) {
            Ok(publication) => publication,
            Err(code) => return Ok(code),
        };
    }
    if interrupts.pending {
        return Ok(ExitCode::from(INTERRUPTED));
    }
    if prompts
        .confirm("Configure a server now?", interrupts)
        .await?
        == Some(true)
    {
        if let Some(path) = prompts.server(root, interrupts).await? {
            if let Some(publication) = published {
                super::publish::offer_config_commit(root, &path, &publication, interrupts).await?;
            } else {
                println!("Deployment config remains local. Review and commit it when ready.");
            }
        }
    }
    Ok(if interrupts.pending {
        ExitCode::from(INTERRUPTED)
    } else {
        ExitCode::SUCCESS
    })
}

fn chained_publication(
    root: &Path,
    outcome: super::publish::PublishOutcome,
    interrupts: &mut Interrupts,
) -> Result<Option<super::publish::Publication>, ExitCode> {
    match outcome {
        super::publish::PublishOutcome::Published(publication) => Ok(Some(publication)),
        outcome => {
            if matches!(outcome, super::publish::PublishOutcome::Interrupted) {
                interrupts.pending = true;
            }
            println!("The verified local project is retained. Run `cd {} && slip publish github` to publish later.", shell_quote(root.as_os_str()));
            match outcome {
                super::publish::PublishOutcome::NotReady
                | super::publish::PublishOutcome::Declined => Ok(None),
                _ => Err(outcome.exit_code()),
            }
        }
    }
}

fn retained_project_guidance(root: &Path) -> String {
    format!(
        "Local project retained at {}.\nFrom that project, review git status and finish any pending commit.\nCompleted commits and any completed GitHub publication are retained. Run slip publish github to review or resume publication.\nRun slip init if no .shipslip.toml exists; otherwise review the existing deployment config.",
        escape(&root.display().to_string())
    )
}

fn shell_quote(value: &std::ffi::OsStr) -> String {
    format!(
        "'{}'",
        escape(&value.to_string_lossy()).replace('\'', "'\\''")
    )
}

async fn configure_server(
    root: &Path,
    interrupts: &mut Interrupts,
) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let plan = shipslip::config::InitPlan::new(root)?;
    for warning in plan.warnings() {
        eprintln!("Warning: {}", escape(warning));
    }
    println!("Creating {}", escape(&plan.path().display().to_string()));
    let branch = super::current_branch(root);
    let answers = ask_typed(
        move || {
            super::ask_init(
                &mut io::stdin().lock(),
                &mut io::stdout(),
                branch.as_deref(),
            )
        },
        interrupts,
    )
    .await?;
    let answers = answers.unwrap_or_else(|| super::exit_interrupted());
    plan.write(&answers)?;
    println!(
        "Wrote {}. Review the recipe, then run slip trust {}.",
        escape(&plan.path().display().to_string()),
        escape(&answers.env)
    );
    Ok(Some(plan.path().into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Args, Box<dyn Error>> {
        Args::parse(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn parsing_and_missing_choice_contract() {
        assert_eq!(parse(&["demo"]).unwrap().missing(), Some("--starter-kit"));
        assert_eq!(
            parse(&["demo", "--starter-kit", "react"])
                .unwrap()
                .missing(),
            Some("--auth")
        );
        assert_eq!(
            parse(&[".", "--starter-kit=none"]).unwrap().missing(),
            Some("--database")
        );
        let complete = parse(&[
            "demo",
            "--starter-kit",
            "react",
            "--auth=none",
            "--database=mysql",
            "--testing=phpunit",
            "--branch",
            "custom",
            "--boost",
        ])
        .unwrap();
        assert!(complete.missing().is_none());
        assert_eq!(complete.boost, Some(true));
        assert_eq!(complete.branch, "custom");
        for args in [
            vec![],
            vec!["a", "b"],
            vec!["demo", "--force"],
            vec!["demo", "--starter-kit", "bogus"],
            vec!["demo", "--auth", "workos"],
            vec!["demo", "--starter-kit", "none", "--auth", "laravel"],
            vec!["demo", "--boost", "--boost"],
            vec!["demo", "--database"],
            vec!["demo", "--auth=none", "--auth=none"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
    #[test]
    fn boost_choice_is_prompted_unless_explicit() {
        assert_eq!(parse(&["demo"]).unwrap().boost, None);
        assert_eq!(parse(&["demo", "--boost"]).unwrap().boost, Some(true));
        assert_eq!(parse(&["demo", "--no-boost"]).unwrap().boost, Some(false));
        for flags in [
            ["--boost", "--boost"],
            ["--no-boost", "--no-boost"],
            ["--boost", "--no-boost"],
            ["--no-boost", "--boost"],
        ] {
            assert!(parse(&["demo", flags[0], flags[1]]).is_err());
        }
    }
    #[tokio::test]
    async fn non_tty_missing_choice_fails_before_tool_resolution() {
        if io::stdin().is_terminal() {
            return;
        }
        let error = run(parse(&["demo", "--starter-kit", "react"]).unwrap())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("pass --auth"));
    }
}

#[cfg(all(test, unix))]
#[path = "new_setup_tests.rs"]
mod setup_tests;
