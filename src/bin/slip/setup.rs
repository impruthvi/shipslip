use std::error::Error;
use std::io::{self, IsTerminal};
use std::process::ExitCode;

use shipslip::logs::escape;
use shipslip::setup::{
    self, DetectionContext, DetectionOptions, DetectionReport, DoctorReport, FindingState, Purpose,
};

use super::{ask_typed, ask_value, ask_yes_no, invalid_input, Interrupts, INTERRUPTED};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Args {
    pub(super) purpose: Option<Purpose>,
    pub(super) json: bool,
}
impl Args {
    pub(super) fn parse(args: &[String]) -> io::Result<Self> {
        let mut result = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if arg == "--json" {
                if result.json {
                    return Err(invalid_input("--json was provided twice"));
                }
                result.json = true;
                continue;
            }
            let value = if arg == "--for" {
                args.next()
                    .ok_or_else(|| invalid_input("--for requires create or publish"))?
                    .as_str()
            } else if let Some(value) = arg.strip_prefix("--for=") {
                value
            } else {
                return Err(invalid_input(format!("unknown doctor option {arg}")));
            };
            if result.purpose.is_some() {
                return Err(invalid_input("--for was provided twice"));
            }
            result.purpose = Some(match value {
                "create" => Purpose::Create,
                "publish" => Purpose::Publish,
                _ => return Err(invalid_input("--for must be create or publish")),
            });
        }
        Ok(result)
    }
}

pub(super) fn json_requested(args: &[String]) -> bool {
    let index = if args.first().map(String::as_str) == Some("--config") {
        2
    } else {
        0
    };
    args.get(index).map(String::as_str) == Some("doctor")
        && args[index + 1..].iter().any(|arg| arg == "--json")
}

pub(super) fn render_json(
    report: &DetectionReport,
    purposes: &[Purpose],
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&DoctorReport::from_detection(report, purposes))
}

pub(super) fn render_error_json(error: &str, invalid: bool) -> String {
    let report = if invalid {
        DoctorReport::invalid_input(error)
    } else {
        DoctorReport::failed(error)
    };
    serde_json::to_string(&report).expect("doctor error output contains only JSON-safe fields")
}

pub(super) fn shell_line(dir: &std::path::Path) -> String {
    format!(
        "export PATH='{}':\"$PATH\"",
        dir.to_string_lossy().replace('\'', "'\\''")
    )
}

pub(super) fn render_state(state: &FindingState) -> String {
    match state {
        FindingState::Ok { version, .. } => format!("ok ({version})"),
        FindingState::Missing => "missing".into(),
        FindingState::TooOld { found, need } => format!("too old ({found}; need {need})"),
        FindingState::Broken { error, .. } => format!("broken: {error}"),
        FindingState::OffPath { .. } => "warning: installed outside PATH; usable by slip".into(),
        FindingState::Unverified { reason } => format!("warning: unverified: {reason}"),
    }
}

pub(super) fn render(report: &DetectionReport, purposes: &[Purpose]) -> String {
    let mut lines = Vec::new();
    for purpose in purposes {
        lines.push(match purpose {
            Purpose::Create => "Project creation:".into(),
            Purpose::Publish => "GitHub publishing:".into(),
        });
        for finding in report
            .findings
            .iter()
            .filter(|finding| finding.requirement.purposes.contains(purpose))
        {
            let status = render_state(&finding.state);
            lines.push(format!("  {}: {}", finding.requirement.id, escape(&status)));
            if let Some(location) = &finding.location {
                lines.push(format!(
                    "    {} ({:?})",
                    escape(&location.path.display().to_string()),
                    finding.owner
                ));
            }
            if let FindingState::OffPath { dir, .. } = &finding.state {
                lines.push(format!(
                    "    Add to your shell startup file: {}",
                    escape(&shell_line(dir))
                ));
            }
            if let Some(detail) = &finding.detail {
                lines.push(format!("    {}", escape(detail)));
            }
            if !finding.managers.is_empty() {
                lines.push(format!("    Installed providers: {:?}", finding.managers));
            }
            if let Some(path) = &finding.shadowing {
                lines.push(format!(
                    "    Shadowing path: {}",
                    escape(&path.display().to_string())
                ));
            }
        }
    }
    if let Some(brew) = &report.facts.homebrew {
        lines.push(format!(
            "Homebrew: {} (prefix {}; {})",
            escape(&brew.binary.display().to_string()),
            escape(&brew.prefix.display().to_string()),
            if brew.writable {
                "writable"
            } else {
                "not writable by this user"
            }
        ));
    }
    if report.facts.command_line_tools == Some(false) {
        lines.push("Command Line Tools not installed; run xcode-select --install.".into());
    }
    for warning in &report.warnings {
        lines.push(format!(
            "Warning: {}: {}",
            escape(&warning.path.display().to_string()),
            escape(&warning.message)
        ));
    }
    lines.push(
        if report.ready() {
            "Ready (warnings may remain)."
        } else {
            "Not ready: resolve the blocking findings above."
        }
        .into(),
    );
    lines.join("\n")
}

pub(super) async fn run(args: Args) -> Result<ExitCode, Box<dyn Error>> {
    let context = DetectionContext::from_environment();
    run_with_context(args, &context, std::env::current_dir()?).await
}

pub(super) async fn run_with_context(
    args: Args,
    context: &DetectionContext,
    root: std::path::PathBuf,
) -> Result<ExitCode, Box<dyn Error>> {
    let purposes = args.purpose.map_or_else(
        || vec![Purpose::Create, Purpose::Publish],
        |purpose| vec![purpose],
    );
    let report = setup::detect(
        context,
        &DetectionOptions {
            purposes: purposes.clone(),
            root,
            installer_options: None,
        },
    )
    .await?;
    let output = if args.json {
        render_json(&report, &purposes)?
    } else {
        render(&report, &purposes)
    };
    println!("{output}");
    Ok(report_exit_code(&report))
}

pub(super) fn report_exit_code(report: &DetectionReport) -> ExitCode {
    if report.ready() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(3)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RepairArgs {
    pub(super) purpose: Option<Purpose>,
}
impl RepairArgs {
    pub(super) fn parse(args: &[String]) -> io::Result<Self> {
        let parsed = Args::parse(args)
            .map_err(|error| invalid_input(error.to_string().replace("doctor", "setup")))?;
        if parsed.json {
            return Err(invalid_input("unknown setup option --json"));
        }
        Ok(Self {
            purpose: parsed.purpose,
        })
    }
}

pub(super) fn render_plan(plan: &setup::SetupPlan) -> String {
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let mut lines = vec!["Setup plan:".into()];
    for (index, action) in plan.actions.iter().enumerate() {
        let environment = action
            .environment
            .iter()
            .map(|(key, value)| format!("{key}={}", quote(value)))
            .collect::<Vec<_>>()
            .join(" ");
        let command = std::iter::once(quote(&action.binary.to_string_lossy()))
            .chain(action.args.iter().map(|arg| quote(arg)))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(format!(
            "  {}. {}{}{}",
            index + 1,
            environment,
            if environment.is_empty() { "" } else { " " },
            escape(&command)
        ));
        lines.push(format!(
            "     Runs as {:?}; {:?}",
            action.runs_as, action.kind
        ));
        for precondition in &action.preconditions {
            lines.push(format!(
                "     Requires executable: {}",
                escape(&precondition.path.display().to_string())
            ));
        }
        if !action.environment.is_empty() {
            lines.push(
                "     Homebrew may install or upgrade the new formula's own dependencies.".into(),
            );
        }
    }
    if !plan.actions.is_empty() {
        lines
            .push("Approved tools take precedence in each command; your PATH follows them.".into());
        lines.push("PATH additions:".into());
        for path in &plan.path_additions {
            lines.push(format!("  {}", escape(&path.display().to_string())));
        }
    }
    for guidance in &plan.guidance {
        lines.push(format!(
            "Guidance{}: {}",
            guidance
                .requirement
                .map(|id| format!(" for {id}"))
                .unwrap_or_default(),
            escape(&guidance.message)
        ));
        for command in &guidance.commands {
            lines.push(format!("  {}", escape(command)));
        }
    }
    lines.join("\n")
}

pub(super) trait SetupPrompts {
    async fn identity(
        &mut self,
        field: &'static str,
        interrupts: &mut Interrupts,
    ) -> io::Result<Option<String>>;
    async fn confirm(
        &mut self,
        plan: &setup::SetupPlan,
        question: &str,
        interrupts: &mut Interrupts,
    ) -> io::Result<Option<bool>>;
}
pub(super) struct TerminalPrompts;
impl SetupPrompts for TerminalPrompts {
    async fn identity(
        &mut self,
        field: &'static str,
        interrupts: &mut Interrupts,
    ) -> io::Result<Option<String>> {
        ask_typed(
            move || {
                ask_value(
                    &mut io::stdin().lock(),
                    &mut io::stdout(),
                    &format!("Git {field}"),
                    None,
                    setup::validate_identity,
                )
            },
            interrupts,
        )
        .await
    }
    async fn confirm(
        &mut self,
        _plan: &setup::SetupPlan,
        question: &str,
        interrupts: &mut Interrupts,
    ) -> io::Result<Option<bool>> {
        let question = question.to_owned();
        ask_typed(
            move || ask_yes_no(&mut io::stdin().lock(), &mut io::stdout(), &question, false),
            interrupts,
        )
        .await
    }
}

pub(super) async fn run_setup(args: RepairArgs) -> Result<ExitCode, Box<dyn Error>> {
    if !io::stdin().is_terminal() {
        return Err(invalid_input(
            "setup asks for confirmation; run it in an interactive terminal",
        )
        .into());
    }
    let context = DetectionContext::from_environment();
    let mut interrupts = Interrupts::listen();
    let code = run_setup_flow(
        args,
        &context,
        std::env::current_dir()?,
        shipslip::create::default_operations_root()?,
        &mut interrupts,
        &mut TerminalPrompts,
    )
    .await?;
    if code == ExitCode::from(INTERRUPTED) {
        // A cancelled terminal prompt still owns a blocking stdin reader.
        super::exit_interrupted();
    }
    Ok(code)
}

fn rerun(args: &RepairArgs) -> String {
    format!(
        "slip setup{}",
        match args.purpose {
            None => "",
            Some(Purpose::Create) => " --for create",
            Some(Purpose::Publish) => " --for publish",
        }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairOutcome {
    Ready,
    NotReady,
    Declined,
    Failed,
    Invalid,
    Interrupted,
}
impl RepairOutcome {
    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Ready,
            3 => Self::NotReady,
            2 => Self::Invalid,
            INTERRUPTED => Self::Interrupted,
            _ => Self::Failed,
        }
    }
    pub(super) fn exit_code(self) -> ExitCode {
        ExitCode::from(match self {
            Self::Ready => 0,
            Self::NotReady | Self::Declined => 3,
            Self::Failed => 1,
            Self::Invalid => 2,
            Self::Interrupted => INTERRUPTED,
        })
    }
}

pub(super) struct RepairRequest {
    pub options: DetectionOptions,
    pub operations_root: std::path::PathBuf,
    pub report: Option<DetectionReport>,
    pub heading: Option<&'static str>,
    pub warnings: String,
    pub confirmation: String,
    pub rerun: String,
}

fn finish_repair(
    code: u8,
    rerun: &str,
    path: &std::path::Path,
    error: Option<&str>,
) -> RepairOutcome {
    println!("Operation record: {}", escape(&path.display().to_string()));
    if let Some(error) = error {
        eprintln!("{}", escape(error));
    }
    if code == INTERRUPTED {
        println!("Setup was interrupted; run `{}` again", rerun);
    } else if code != 0 {
        println!(
            "Run `{}` again after resolving the remaining requirements.",
            rerun
        );
    }
    RepairOutcome::from_code(code)
}

fn finish_prompt_error(
    rerun: &str,
    operations_root: &std::path::Path,
    plan: &setup::SetupPlan,
    report: &DetectionReport,
    error: io::Error,
) -> Result<RepairOutcome, setup::SetupError> {
    let code = if error.kind() == io::ErrorKind::InvalidInput {
        2
    } else {
        1
    };
    let path =
        setup::record_setup_outcome(operations_root, plan, report, code, Some(error.to_string()))?;
    Ok(finish_repair(code, rerun, &path, Some(&error.to_string())))
}

async fn run_setup_flow(
    args: RepairArgs,
    context: &DetectionContext,
    root: std::path::PathBuf,
    operations_root: std::path::PathBuf,
    interrupts: &mut Interrupts,
    prompts: &mut impl SetupPrompts,
) -> Result<ExitCode, Box<dyn Error>> {
    let request = RepairRequest {
        options: DetectionOptions {
            purposes: args.purpose.map_or_else(
                || vec![Purpose::Create, Purpose::Publish],
                |purpose| vec![purpose],
            ),
            root,
            installer_options: None,
        },
        operations_root,
        report: None,
        heading: None,
        warnings: String::new(),
        confirmation: "Apply this setup plan?".into(),
        rerun: rerun(&args),
    };
    Ok(repair_with_prompts(request, context, interrupts, prompts)
        .await?
        .exit_code())
}

pub(super) async fn repair_with_prompts(
    request: RepairRequest,
    context: &DetectionContext,
    interrupts: &mut Interrupts,
    prompts: &mut impl SetupPrompts,
) -> Result<RepairOutcome, Box<dyn Error>> {
    let RepairRequest {
        options,
        operations_root,
        report,
        heading,
        warnings,
        confirmation,
        rerun,
    } = request;
    let notice = format!("Setup was interrupted; waiting for the current check. Press Ctrl-C again to exit immediately. Resume with `{rerun}`.");
    let mut report = match report {
        Some(report) => report,
        None => {
            interrupts
                .defer(setup::detect(context, &options), &notice)
                .await?
        }
    };
    let mut inputs = setup::SetupInputs::default();
    let mut session = None;
    loop {
        if !report.ready() {
            if let Some(heading) = heading {
                println!("\n{heading}");
            }
        }
        println!("{}", render(&report, &options.purposes));
        if !warnings.is_empty() {
            println!("{warnings}");
        }
        if heading.is_some() && report.ready() && !interrupts.pending {
            return Ok(RepairOutcome::Ready);
        }
        let mut plan = setup::plan_setup(context, &report, &inputs)?;
        if interrupts.pending {
            let path =
                setup::record_setup_outcome(&operations_root, &plan, &report, INTERRUPTED, None)?;
            return Ok(finish_repair(INTERRUPTED, &rerun, &path, None));
        }
        for field in plan.inputs_needed.clone() {
            let answer = match prompts.identity(field, interrupts).await {
                Ok(answer) => answer,
                Err(error) => {
                    return Ok(finish_prompt_error(
                        &rerun,
                        &operations_root,
                        &plan,
                        &report,
                        error,
                    )?)
                }
            };
            let Some(value) = answer else {
                let path = setup::record_setup_outcome(
                    &operations_root,
                    &plan,
                    &report,
                    INTERRUPTED,
                    None,
                )?;
                return Ok(finish_repair(INTERRUPTED, &rerun, &path, None));
            };
            match field {
                "user.name" => inputs.name = Some(value),
                "user.email" => inputs.email = Some(value),
                _ => unreachable!(),
            }
        }
        plan = setup::plan_setup(context, &report, &inputs)?;
        println!("{}", render_plan(&plan));
        let code = if plan.actions.is_empty() {
            Some(if report.ready() { 0 } else { 3 })
        } else {
            let answer = match prompts.confirm(&plan, &confirmation, interrupts).await {
                Ok(answer) => answer,
                Err(error) => {
                    return Ok(finish_prompt_error(
                        &rerun,
                        &operations_root,
                        &plan,
                        &report,
                        error,
                    )?)
                }
            };
            match answer {
                Some(true) => None,
                Some(false) => Some(3),
                None => Some(INTERRUPTED),
            }
        };
        if let Some(code) = code {
            let path = setup::record_setup_outcome(&operations_root, &plan, &report, code, None)?;
            let outcome = finish_repair(code, &rerun, &path, None);
            return Ok(if code == 3 && !plan.actions.is_empty() {
                RepairOutcome::Declined
            } else {
                outcome
            });
        }
        if session.is_none() {
            match setup::SetupSession::acquire(&operations_root) {
                Ok(acquired) => session = Some(acquired),
                Err(error) => {
                    let path = setup::record_setup_outcome(
                        &operations_root,
                        &plan,
                        &report,
                        1,
                        Some(error.to_string()),
                    )?;
                    return Ok(finish_repair(1, &rerun, &path, Some(&error.to_string())));
                }
            }
        }
        let (signal, receiver) = tokio::sync::watch::channel(false);
        let applying = session.as_ref().unwrap().apply(
            &plan,
            plan.confirm(),
            setup::SetupEnvironment {
                context,
                options: &options,
            },
            receiver,
            |event| match event {
                setup::SetupEvent::ActionStarted { index } => {
                    println!("\nRunning setup action {}…", index + 1)
                }
                setup::SetupEvent::Output { line, .. } => println!("  {}", escape(&line)),
                setup::SetupEvent::Interrupted { .. } => eprintln!("\nSetup was interrupted; waiting for the current command. Press Ctrl-C again to exit immediately. Resume with `{}`.", escape(&rerun)),
                _ => {}
            },
        );
        let result = interrupts
            .defer_with(applying, "", || {
                let _ = signal.send(true);
            })
            .await;
        match result {
            Ok(setup::SetupApplyResult::Changed {
                report: changed,
                record_path,
            }) => {
                println!("Your machine changed since this plan was shown.");
                println!(
                    "Operation record: {}",
                    escape(&record_path.display().to_string())
                );
                report = *changed;
            }
            Ok(setup::SetupApplyResult::Finished(outcome)) => {
                if let Some(report) = &outcome.report {
                    println!("{}", render(report, &options.purposes));
                }
                return Ok(finish_repair(
                    outcome.exit_code,
                    &rerun,
                    &outcome.record_path,
                    outcome.error.as_deref(),
                ));
            }
            Err(error) => {
                let code = if interrupts.pending { INTERRUPTED } else { 1 };
                let path = setup::record_setup_outcome(
                    &operations_root,
                    &plan,
                    &report,
                    code,
                    Some(error.to_string()),
                )?;
                return Ok(finish_repair(code, &rerun, &path, Some(&error.to_string())));
            }
        }
    }
}

#[cfg(all(test, unix))]
#[path = "setup_tests.rs"]
mod tests;
