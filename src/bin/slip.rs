use std::error::Error;
use std::future::Future;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use shipslip::config::{
    approve_trust, default_trust_path, is_branch_name, is_env_name, is_smoke_url, is_ssh_alias,
    trust_status, InitAnswers, InitPlan, LoadedConfig, TrustSnapshot, TrustStatus,
};
use shipslip::receipt::{default_receipts_root, find_open, ReceiptJournal};
use shipslip::transport::SshTransport;
use shipslip::{
    attach, break_lock, bring_app_up, cancel, execute_recorded, lock_status, prepare_with_plan,
    BreakLockError, BringUpError, Confirmation, DeployEvent, DeployOutcome, DeployTarget,
    ExecuteRejected, ExecutionHandle, MaintenancePhase, PrepareError, RunPlan, SmokeResult,
    StepStatus, WatchResult, WatchStatus, POST_DEPLOY_WATCH,
};
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Exit status after Ctrl-C, as shells report it.
const INTERRUPTED: u8 = 130;
const PROGRESS_EVERY: Duration = Duration::from_secs(30);

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("slip: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode, Box<dyn Error>> {
    let Some(command) = parse_args()? else {
        return Ok(ExitCode::SUCCESS);
    };
    let action = match command.action {
        Action::Init if command.config.is_some() => {
            return Err(invalid_input(
                "init writes .shipslip.toml at the git root; omit --config and unset SHIPSLIP_CONFIG",
            )
            .into());
        }
        Action::Init => return init_command(&std::env::current_dir()?),
        action => action,
    };
    let config = LoadedConfig::load(&std::env::current_dir()?, command.config.as_deref())?;
    if config.uses_default_recipe() {
        eprintln!("Using the default Laravel deploy recipe; add [recipe.deploy] to customize it.");
    }
    let trust_path = default_trust_path()?;
    let receipts_root = default_receipts_root()?;
    let (environment, plan) = match action {
        Action::Init => unreachable!("init runs before the config is loaded"),
        Action::Trust { environment } => {
            return trust_command(&config, &trust_path, environment.as_deref());
        }
        Action::Attach { environment } => {
            return attach_command(&config, &trust_path, &receipts_root, &environment).await;
        }
        Action::BreakLock { environment } => {
            return break_lock_command(&config, &trust_path, &environment).await;
        }
        Action::Up { environment } => {
            return up_command(&config, &trust_path, &environment).await;
        }
        Action::Run { environment, plan } => (environment, plan),
    };
    let target = trusted_target(&config, &trust_path, &environment)?;

    let transport = Arc::new(SshTransport::connect(&target.ssh_alias).await?);
    let mut interrupts = Interrupts::listen();
    let preview = interrupts
        .defer(
            prepare_with_plan(target.clone(), plan, transport.as_ref()),
            "Cancelling once the server check finishes; press Ctrl-C again to quit now.",
        )
        .await
        .map_err(|error| match error {
            PrepareError::LockHeld(info) => format!(
                "deploy lock is {info}; run `slip attach {environment}` to resume an unfinished \
                 run, or check the server and run `slip break-lock {environment}` if it is stale"
            )
            .into(),
            error => Box::<dyn Error>::from(error),
        })?;
    if interrupts.pending {
        interrupts
            .defer(cancel(preview, transport.as_ref()), "")
            .await?;
        println!("Cancelled; no deploy steps were run.");
        exit_interrupted();
    }
    let journal = match ReceiptJournal::create(
        &receipts_root,
        config.project_name(),
        config.repo_root(),
        &preview,
    ) {
        Ok(journal) => Arc::new(journal),
        Err(error) => {
            cancel(preview, transport.as_ref()).await?;
            return Err(error.into());
        }
    };
    show_preview(&preview);

    let Some((preview, confirmation)) = confirm(
        preview,
        &target,
        transport.as_ref(),
        &journal,
        &mut interrupts,
    )
    .await?
    else {
        return Ok(ExitCode::SUCCESS);
    };
    let (events, handle) =
        match execute_recorded(preview, confirmation, transport.clone(), journal.clone()) {
            Ok(run) => run,
            Err(ExecuteRejected { error, preview }) => {
                cancel_prepared(*preview, transport.as_ref(), &journal).await?;
                return Err(error.into());
            }
        };
    follow_events(events, &handle, &mut interrupts, &RunView::new(&target)).await
}

/// Ctrl-C presses, delivered as messages once the listener is installed.
/// Installing it replaces the default of killing the process.
struct Interrupts {
    rx: mpsc::UnboundedReceiver<()>,
    /// A Ctrl-C was pressed and its effect has not finished yet.
    pending: bool,
}

impl Interrupts {
    fn listen() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while tokio::signal::ctrl_c().await.is_ok() {
                if tx.send(()).is_err() {
                    break;
                }
            }
        });
        Self::from_channel(rx)
    }

    fn from_channel(rx: mpsc::UnboundedReceiver<()>) -> Self {
        Self { rx, pending: false }
    }

    async fn recv(&mut self) {
        if self.rx.recv().await.is_none() {
            std::future::pending::<()>().await;
        }
    }

    /// Runs `work` to the end. The first Ctrl-C prints `notice` and is
    /// remembered in `pending`; another one exits at once.
    async fn defer<F: Future>(&mut self, work: F, notice: &str) -> F::Output {
        tokio::pin!(work);
        loop {
            tokio::select! {
                output = &mut work => return output,
                () = self.recv() => {
                    if self.pending {
                        exit_interrupted();
                    }
                    self.pending = true;
                    eprintln!("\n{notice}");
                }
            }
        }
    }
}

/// Exits without waiting for a prompt's reader thread, which cannot be
/// cancelled while it waits for input.
fn exit_interrupted() -> ! {
    let _ = io::stdout().flush();
    std::process::exit(INTERRUPTED.into())
}

/// Reads one answer on a blocking thread. `None` if Ctrl-C came first.
async fn ask(
    read: impl FnOnce() -> io::Result<String> + Send + 'static,
    interrupts: &mut Interrupts,
) -> io::Result<Option<String>> {
    let answer = tokio::task::spawn_blocking(read);
    tokio::select! {
        answer = answer => answer.map_err(io::Error::other)?.map(Some),
        () = interrupts.recv() => Ok(None),
    }
}

/// What the running deploy can be asked to do.
trait RunControls {
    fn detach(&self);
    fn cancel_watch(&self);
}

impl RunControls for ExecutionHandle {
    fn detach(&self) {
        ExecutionHandle::detach(self);
    }

    fn cancel_watch(&self) {
        ExecutionHandle::cancel_watch(self);
    }
}

/// Where a followed run is, for deciding what Ctrl-C does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Running,
    BringingUp,
    Watching,
    Finishing,
}

fn next_stage(stage: Stage, event: &DeployEvent) -> Stage {
    match event {
        DeployEvent::MaintenanceStarted {
            phase: MaintenancePhase::Up,
        } => Stage::BringingUp,
        DeployEvent::MaintenanceFinished {
            phase: MaintenancePhase::Up,
            ..
        }
        | DeployEvent::WatchFinished(_) => Stage::Finishing,
        DeployEvent::WatchStarted { .. } => Stage::Watching,
        _ => stage,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Interrupt {
    /// Start no new step and stop following the running one.
    Detach,
    /// Maintenance up cannot be left half-done; skip the watch after it.
    SkipWatch,
    StopWatch,
    /// Only the lock release and receipt remain.
    Wait,
    Quit,
}

/// A second Ctrl-C in the same stage quits; a new stage starts over.
fn on_interrupt(stage: Stage, again: bool) -> Interrupt {
    if again {
        return Interrupt::Quit;
    }
    match stage {
        Stage::Running => Interrupt::Detach,
        Stage::BringingUp => Interrupt::SkipWatch,
        Stage::Watching => Interrupt::StopWatch,
        Stage::Finishing => Interrupt::Wait,
    }
}

struct RunView {
    env: String,
    log: String,
}

impl RunView {
    fn new(target: &DeployTarget) -> Self {
        Self {
            env: target.env.clone(),
            log: log_display(target),
        }
    }
}

fn log_display(target: &DeployTarget) -> String {
    format!(
        "{}{}",
        target.log_path(),
        if target.log_daily { "-*.log" } else { "" }
    )
}

fn watch_left_text(left: Duration) -> Option<String> {
    let secs = left.as_secs_f64().round() as u64;
    (secs > 0).then(|| format!("Log watch: {secs} s left"))
}

fn watch_summary(result: &WatchResult) -> String {
    let errors = match result.new_errors.len() {
        0 => "no new errors".to_string(),
        1 => "1 new error group".to_string(),
        n => format!("{n} new error groups"),
    };
    let state = match result.status {
        WatchStatus::Complete => "complete",
        WatchStatus::Partial => "partial",
        WatchStatus::Cancelled => "stopped early",
        WatchStatus::Unavailable => "log could not be read",
        WatchStatus::NoLogSeen => "no log file found",
        WatchStatus::NotRun => "not run",
    };
    let checked = matches!(
        result.status,
        WatchStatus::Complete | WatchStatus::Partial | WatchStatus::Cancelled
    );
    if checked || !result.new_errors.is_empty() {
        format!("Log watch: {state}, {errors}")
    } else {
        format!("Log watch: {state}")
    }
}

async fn follow_events(
    mut events: mpsc::UnboundedReceiver<DeployEvent>,
    controls: &impl RunControls,
    interrupts: &mut Interrupts,
    view: &RunView,
) -> Result<ExitCode, Box<dyn Error>> {
    let mut succeeded = false;
    let mut stage = Stage::Running;
    let mut interrupted_in = None;
    let mut progress: Option<(tokio::time::Interval, Instant)> = None;
    loop {
        let event = tokio::select! {
            biased;
            event = events.recv() => match event {
                Some(event) => event,
                None => break,
            },
            () = interrupts.recv() => {
                let action = on_interrupt(stage, interrupted_in == Some(stage));
                interrupted_in = Some(stage);
                match action {
                    Interrupt::Detach => {
                        controls.detach();
                        controls.cancel_watch();
                        eprintln!(
                            "\nStopping: no new step will start; a running command keeps running \
                             on the server. Press Ctrl-C again to quit now."
                        );
                    }
                    Interrupt::SkipWatch => {
                        controls.cancel_watch();
                        eprintln!(
                            "\nBringing the app back up first; the log watch will be skipped. \
                             Press Ctrl-C again to quit now (the app may stay in maintenance mode)."
                        );
                    }
                    Interrupt::StopWatch => {
                        controls.cancel_watch();
                        progress = None;
                        eprintln!("\nStopping the log watch...");
                    }
                    Interrupt::Wait => {
                        eprintln!("\nFinishing the run. Press Ctrl-C again to quit now.");
                    }
                    Interrupt::Quit => {
                        eprintln!(
                            "Quit before the run finished; run `slip attach {}` to check it.",
                            view.env
                        );
                        return Ok(ExitCode::from(INTERRUPTED));
                    }
                }
                continue;
            }
            () = async {
                match progress.as_mut() {
                    Some((ticks, _)) => {
                        ticks.tick().await;
                    }
                    None => std::future::pending().await,
                }
            } => {
                if let Some(text) = progress
                    .as_ref()
                    .and_then(|(_, end)| watch_left_text(end.saturating_duration_since(Instant::now())))
                {
                    println!("{text}");
                }
                continue;
            }
        };
        stage = next_stage(stage, &event);
        match event {
            DeployEvent::StepStarted { index, name } => println!("\nStep {index}: {name}"),
            DeployEvent::Output { line, .. } => println!("  {line}"),
            DeployEvent::StepFinished {
                index,
                status,
                exit_code,
            } => show_step_result(index, status, exit_code),
            DeployEvent::MaintenanceStarted { phase } => {
                println!("\nMaintenance mode: {}", phase_name(phase));
            }
            DeployEvent::MaintenanceOutput { line, .. } => println!("  {line}"),
            DeployEvent::MaintenanceFinished {
                phase,
                status,
                exit_code,
            } => {
                println!(
                    "Maintenance {}: {}{}",
                    phase_name(phase),
                    status_name(&status),
                    exit_code
                        .map(|code| format!(" (exit {code})"))
                        .unwrap_or_default()
                );
            }
            DeployEvent::AppLeftDown => {
                eprintln!("The app may still be in maintenance mode.");
            }
            DeployEvent::ServerState { head, tree_dirty } => {
                println!(
                    "Server state: HEAD={}, working tree {}",
                    head.as_deref().unwrap_or("unknown"),
                    match tree_dirty {
                        Some(true) => "dirty",
                        Some(false) => "clean",
                        None => "unknown",
                    }
                );
            }
            DeployEvent::NewLogError {
                phase,
                message,
                file_line,
            } => {
                println!("New log error ({phase:?}): {message}");
                if let Some(file_line) = file_line {
                    println!("  at {file_line}");
                }
            }
            DeployEvent::WatchStarted { window } => {
                println!(
                    "Watching {} for {} s for new errors (Ctrl-C to stop watching)",
                    view.log,
                    window.as_secs()
                );
                progress = Some((
                    tokio::time::interval_at(Instant::now() + PROGRESS_EVERY, PROGRESS_EVERY),
                    Instant::now() + window,
                ));
            }
            DeployEvent::WatchFinished(result) => {
                progress = None;
                println!("{}", watch_summary(&result));
                for warning in result.warnings {
                    eprintln!("  {warning}");
                }
            }
            DeployEvent::SmokeFinished(result) => match result {
                SmokeResult::Passed { status, latency_ms } => {
                    println!("Smoke check: HTTP {status} in {latency_ms} ms")
                }
                SmokeResult::Failed { status, reason } => eprintln!(
                    "Smoke check failed{}: {reason}",
                    status
                        .map(|code| format!(" (HTTP {code})"))
                        .unwrap_or_default()
                ),
                SmokeResult::Skipped => println!("Smoke check: skipped"),
                SmokeResult::NotConfigured => {}
            },
            DeployEvent::Detached { index } => {
                println!(
                    "Stopped observing step {index}; it continues on the server. \
                     Run `slip attach {}` to follow it.",
                    view.env
                );
            }
            DeployEvent::Interrupted { index, reason } => {
                eprintln!("Lost contact while observing step {index}: {reason}");
            }
            DeployEvent::RunError { reason } => eprintln!("Deploy could not finish: {reason}"),
            DeployEvent::Finished(outcome) => {
                succeeded = matches!(&outcome, DeployOutcome::Succeeded);
                show_outcome(&outcome);
            }
            _ => {}
        }
    }

    Ok(if succeeded {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn parse_args() -> Result<Option<Command>, Box<dyn Error>> {
    parse_args_from(std::env::args().skip(1).collect())
}

fn parse_args_from(args: Vec<String>) -> Result<Option<Command>, Box<dyn Error>> {
    if args.is_empty()
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        print_help();
        return Ok(None);
    }

    let mut index = 0;
    let config = if args.get(index).map(String::as_str) == Some("--config") {
        index += 1;
        let path = args
            .get(index)
            .ok_or_else(|| invalid_input("--config requires a file path"))?;
        index += 1;
        Some(PathBuf::from(path))
    } else {
        std::env::var_os("SHIPSLIP_CONFIG").map(PathBuf::from)
    };

    let action = args
        .get(index)
        .ok_or_else(|| invalid_input("missing command"))?;
    index += 1;
    if action == "init" {
        if index != args.len() {
            return Err(invalid_input("init takes no arguments").into());
        }
        return Ok(Some(Command {
            config,
            action: Action::Init,
        }));
    }
    if action == "trust" {
        let environment = args.get(index).cloned();
        if index + usize::from(environment.is_some()) != args.len() {
            return Err(invalid_input("trust accepts at most one environment name").into());
        }
        return Ok(Some(Command {
            config,
            action: Action::Trust { environment },
        }));
    }
    if matches!(action.as_str(), "attach" | "break-lock" | "up") {
        let environment = args
            .get(index)
            .ok_or_else(|| invalid_input(format!("{action} requires an environment name")))?
            .clone();
        if index + 1 != args.len() {
            return Err(invalid_input(format!("{action} accepts one environment name")).into());
        }
        let action = match action.as_str() {
            "attach" => Action::Attach { environment },
            "break-lock" => Action::BreakLock { environment },
            "up" => Action::Up { environment },
            _ => unreachable!(),
        };
        return Ok(Some(Command { config, action }));
    }
    let environment = args
        .get(index)
        .ok_or_else(|| invalid_input("missing environment name"))?
        .clone();
    index += 1;

    let plan = match action.as_str() {
        "deploy" => RunPlan::Deploy,
        "rerun" => RunPlan::Rerun,
        "from-step" => {
            let step = args
                .get(index)
                .ok_or_else(|| invalid_input("from-step requires a recipe step number"))?
                .parse::<usize>()
                .map_err(|_| invalid_input("step number must be a positive integer"))?;
            index += 1;
            if step == 0 {
                return Err(invalid_input("recipe steps start at 1").into());
            }
            RunPlan::FromStep(step)
        }
        _ => {
            return Err(invalid_input(format!(
                "unknown command `{action}`; run `slip --help` for usage"
            ))
            .into())
        }
    };

    if index != args.len() {
        return Err(invalid_input("unexpected extra argument; run `slip --help` for usage").into());
    }

    Ok(Some(Command {
        config,
        action: Action::Run { environment, plan },
    }))
}

fn print_help() {
    println!(
        "Shipslip deploy runner\n\n\
         Usage:\n\
         \x20 slip [--config FILE] <deploy|rerun|from-step> <ENV> [STEP]\n\
         \x20 slip init\n\
         \x20 slip [--config FILE] trust [ENV]\n\
         \x20 slip [--config FILE] attach ENV\n\
         \x20 slip [--config FILE] break-lock ENV\n\
         \x20 slip [--config FILE] up ENV\n\n\
         Commands:\n\
         \x20 deploy ENV         Fast-forward the checkout and run all recipe steps\n\
         \x20 rerun ENV          Run all recipe steps on the already-deployed commit\n\
         \x20 from-step ENV STEP Run recipe steps starting at STEP (steps start at 1)\n\
         \x20 init               Create .shipslip.toml by answering a few questions\n\
         \x20 trust [ENV]        Review and approve config changes\n\
         \x20 attach ENV         Resume an unfinished run without relaunching its active step\n\
         \x20 break-lock ENV     Clear a stale deploy lock after checking the old run\n\
         \x20 up ENV             Run `php artisan up` under a new deploy lock\n\n\
         Config is discovered from the current directory up to the git root.\n\
         SHIPSLIP_CONFIG can select a different file."
    );
}

fn show_preview(preview: &shipslip::Preview) {
    println!("Environment: {}", preview.target().env);
    match preview.run_plan() {
        RunPlan::Deploy => println!("Plan:        fast-forward, then all recipe steps"),
        RunPlan::Rerun => println!("Plan:        rerun all recipe steps; leave checkout unchanged"),
        RunPlan::FromStep(step) => println!(
            "Plan:        recipe steps {step} through {}; leave checkout unchanged",
            preview.target().steps.len()
        ),
    }
    println!("Branch:      {}", preview.target().branch);
    println!("SSH alias:   {}", preview.target().ssh_alias);
    println!("Path:        {}", preview.target().path);
    println!("Current SHA: {}", preview.from_sha());
    println!("Target SHA:  {}", preview.target_sha());
    if preview.run_plan() == RunPlan::Deploy && !preview.commits().is_empty() {
        println!("Commits to deploy:");
        for commit in preview.commits() {
            println!("  {commit}");
        }
    }
    let first_step = preview.run_plan().first_recipe_step();
    println!("Recipe steps:");
    for (index, step) in preview.target().steps.iter().enumerate() {
        if index + 1 >= first_step {
            println!("  {}. {step}", index + 1);
        }
    }
    if preview.target().maintenance {
        println!("Maintenance: enabled");
    }
    if preview.target().watch_log {
        println!(
            "Log watch:   {} ({} s after steps)",
            log_display(preview.target()),
            POST_DEPLOY_WATCH.as_secs()
        );
    }
    if let Some(url) = &preview.target().smoke_url {
        println!("Smoke URL:   {url}");
    }
    if preview.target().production {
        println!("This is a production environment.");
    }
}

fn init_command(start: &Path) -> Result<ExitCode, Box<dyn Error>> {
    let plan = InitPlan::new(start)?;
    if !io::stdin().is_terminal() {
        return Err(invalid_input("init asks questions; run it in an interactive terminal").into());
    }
    println!("Creating {}", plan.path().display());
    let branch = current_branch(start);
    let answers = ask_init(
        &mut io::stdin().lock(),
        &mut io::stdout(),
        branch.as_deref(),
    )?;
    plan.write(&answers)?;
    println!("\nWrote {}.", plan.path().display());
    println!(
        "Review it, especially the recipe steps, then run `slip trust {}`.",
        answers.env
    );
    Ok(ExitCode::SUCCESS)
}

fn current_branch(dir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(dir)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let branch = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && is_branch_name(&branch)).then_some(branch)
}

fn ask_init(
    input: &mut impl BufRead,
    output: &mut impl Write,
    branch: Option<&str>,
) -> io::Result<InitAnswers> {
    let env = ask_value(input, output, "Environment name", Some("staging"), |v| {
        is_env_name(v)
            .then_some(())
            .ok_or("Use only letters, digits, `-` and `_`.")
    })?;
    let ssh_alias = ask_value(
        input,
        output,
        "SSH host alias from ~/.ssh/config",
        None,
        |v| {
            is_ssh_alias(v)
                .then_some(())
                .ok_or("Use the `Host` name from ~/.ssh/config, not user@host.")
        },
    )?;
    let path = ask_value(input, output, "App path on the server", None, |v| {
        Path::new(v)
            .is_absolute()
            .then_some(())
            .ok_or("Use an absolute path, such as /var/www/app.")
    })?;
    let branch = ask_value(input, output, "Branch to deploy", branch, |v| {
        is_branch_name(v)
            .then_some(())
            .ok_or("That is not a supported branch name.")
    })?;
    let production = ask_yes_no(input, output, "Is this a production environment?", false)?;
    let maintenance = ask_yes_no(
        input,
        output,
        "Use maintenance mode (php artisan down/up) during deploys?",
        true,
    )?;
    let smoke_url = ask_value(
        input,
        output,
        "Smoke check URL (Enter to skip)",
        Some(""),
        |v| {
            (v.is_empty() || is_smoke_url(v))
                .then_some(())
                .ok_or("Use an http:// or https:// URL.")
        },
    )?;
    Ok(InitAnswers {
        env,
        ssh_alias,
        path,
        branch,
        production,
        maintenance,
        smoke_url: (!smoke_url.is_empty()).then_some(smoke_url),
    })
}

/// Asks until `check` accepts the answer. An empty answer takes `default`.
fn ask_value(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &str,
    default: Option<&str>,
    check: impl Fn(&str) -> Result<(), &'static str>,
) -> io::Result<String> {
    loop {
        match default {
            Some(default) if !default.is_empty() => write!(output, "{question} [{default}]: ")?,
            _ => write!(output, "{question}: ")?,
        }
        let answer = match read_init_line(input, output)?.as_str() {
            "" => match default {
                Some(default) => default.to_string(),
                None => {
                    writeln!(output, "  An answer is required.")?;
                    continue;
                }
            },
            answer => answer.to_string(),
        };
        match check(&answer) {
            Ok(()) => return Ok(answer),
            Err(why) => writeln!(output, "  {why}")?,
        }
    }
}

fn ask_yes_no(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &str,
    default: bool,
) -> io::Result<bool> {
    loop {
        write!(
            output,
            "{question} {} ",
            if default { "[Y/n]" } else { "[y/N]" }
        )?;
        match read_init_line(input, output)?.to_ascii_lowercase().as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => writeln!(output, "  Answer y or n.")?,
        }
    }
}

fn read_init_line(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<String> {
    output.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "input ended before init finished; nothing was written",
        ));
    }
    Ok(line.trim().to_string())
}

fn trust_command(
    config: &LoadedConfig,
    trust_path: &Path,
    environment: Option<&str>,
) -> Result<ExitCode, Box<dyn Error>> {
    let names: Vec<String> = match environment {
        Some(name) => {
            if config.trust_snapshot(name).is_none() {
                return Err(
                    invalid_input(format!("environment `{name}` is not in the config")).into(),
                );
            }
            vec![name.to_string()]
        }
        None => config.environment_names().map(str::to_string).collect(),
    };
    println!(
        "Project: {} ({})",
        config.project_name(),
        config.repo_root().display()
    );
    println!("Config:  {}", config.path().display());
    for name in names {
        let snapshot = config
            .trust_snapshot(&name)
            .expect("environment was listed");
        match trust_status(trust_path, config.repo_root(), &name, &snapshot)? {
            TrustStatus::Trusted => println!("{name}: already trusted"),
            TrustStatus::Untrusted { previous } => {
                println!("\n{name}: review these settings before approving:");
                show_trust_changes(previous.as_ref(), &snapshot);
                print!("Type `{name}` to trust this environment (Enter to skip): ");
                io::stdout().flush()?;
                if read_answer()? == name {
                    approve_trust(trust_path, config.repo_root(), &name, snapshot)?;
                    println!("{name}: trusted");
                } else {
                    println!("{name}: remains untrusted");
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn attach_command(
    config: &LoadedConfig,
    trust_path: &Path,
    receipts_root: &Path,
    environment: &str,
) -> Result<ExitCode, Box<dyn Error>> {
    let target = trusted_target(config, trust_path, environment)?;
    let path = find_open(
        receipts_root,
        config.project_name(),
        environment,
        config.repo_root(),
    )?
    .ok_or_else(|| invalid_input(format!("no unfinished run for `{environment}`")))?;
    let journal = Arc::new(ReceiptJournal::load(&path)?);
    journal.claim()?;
    let receipt = journal.snapshot();
    if receipt.project != config.project_name()
        || receipt.repo_root != config.repo_root().to_string_lossy()
        || !saved_target_matches_current(&receipt.target, &target)
    {
        return Err(invalid_input(
            "the unfinished run uses different deploy settings; inspect the receipt before recovery",
        )
        .into());
    }
    println!("Run:        {}", receipt.run_id);
    println!("Environment: {}", receipt.target.env);
    println!("Target SHA:  {}", receipt.target_sha);
    println!("Last phase:  {:?}", receipt.phase);
    println!("Receipt:     {}", path.display());
    if target.production {
        print!("Type `{environment}` to resume this production run: ");
    } else {
        print!("Resume this run? [y/N] ");
    }
    io::stdout().flush()?;
    let answer = read_answer()?;
    let approved = if target.production {
        answer == environment
    } else {
        matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes")
    };
    if !approved {
        println!("Run remains unfinished.");
        return Ok(ExitCode::SUCCESS);
    }
    let transport = Arc::new(SshTransport::connect(&target.ssh_alias).await?);
    let (events, handle) = attach(journal, transport)?;
    // Installed only now: Ctrl-C at the prompt above must leave the run as it is.
    let mut interrupts = Interrupts::listen();
    follow_events(events, &handle, &mut interrupts, &RunView::new(&target)).await
}

fn trusted_target(
    config: &LoadedConfig,
    trust_path: &Path,
    environment: &str,
) -> Result<DeployTarget, Box<dyn Error>> {
    let target = config.target(environment).ok_or_else(|| {
        invalid_input(format!("environment `{environment}` is not in the config"))
    })?;
    let snapshot = config
        .trust_snapshot(environment)
        .expect("target has an environment");
    if !matches!(
        trust_status(trust_path, config.repo_root(), environment, &snapshot)?,
        TrustStatus::Trusted
    ) {
        return Err(invalid_input(format!(
            "config for `{environment}` is untrusted; run `slip trust {environment}` first"
        ))
        .into());
    }
    Ok(target)
}

async fn break_lock_command(
    config: &LoadedConfig,
    trust_path: &Path,
    environment: &str,
) -> Result<ExitCode, Box<dyn Error>> {
    let target = trusted_target(config, trust_path, environment)?;
    let transport = SshTransport::connect(&target.ssh_alias).await?;
    println!("Environment: {environment}");
    println!("SSH alias:   {}", target.ssh_alias);
    println!("Path:        {}", target.path);
    let Some(info) = lock_status(&target, &transport).await? else {
        println!("No deploy lock is held.");
        return Ok(ExitCode::SUCCESS);
    };
    println!("Lock:        {info}");
    if let Some(owner) = &info.owner {
        println!("Run ID:      {}", owner.run_id);
        println!(
            "Old run:     ~/.shipslip/runs/{}/ on the server",
            owner.run_id
        );
    }
    if !info.is_stale() {
        return Err(BreakLockError::Live(info.age_secs).into());
    }
    println!("Check over SSH that no command from the old deploy is still running.");
    print!("Type `{environment}` to break its stale lock (Enter to cancel): ");
    io::stdout().flush()?;
    if read_answer()? != environment {
        println!("Lock was not changed.");
        return Ok(ExitCode::SUCCESS);
    }
    break_lock(&target, &transport, environment).await?;
    println!("Stale deploy lock cleared for `{environment}`.");
    Ok(ExitCode::SUCCESS)
}

async fn up_command(
    config: &LoadedConfig,
    trust_path: &Path,
    environment: &str,
) -> Result<ExitCode, Box<dyn Error>> {
    let target = trusted_target(config, trust_path, environment)?;
    println!("Environment: {environment}");
    println!("SSH alias:   {}", target.ssh_alias);
    println!("Path:        {}", target.path);
    if target.production {
        print!("Type `{environment}` to run `php artisan up`: ");
    } else {
        print!("Run `php artisan up`? [y/N] ");
    }
    io::stdout().flush()?;
    let answer = read_answer()?;
    let approved = if target.production {
        answer == environment
    } else {
        matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes")
    };
    if !approved {
        println!("Maintenance mode was not changed.");
        return Ok(ExitCode::SUCCESS);
    }
    let transport = SshTransport::connect(&target.ssh_alias).await?;
    let lines = bring_app_up(&target, &transport)
        .await
        .map_err(|error| match error {
            BringUpError::LockHeld(info) => format!(
                "deploy lock is {info}; check the server and run \
                 `slip break-lock {environment}` if it is stale"
            )
            .into(),
            error => Box::<dyn Error>::from(error),
        })?;
    for line in lines {
        println!("  {line}");
    }
    println!("Maintenance mode disabled for `{environment}`.");
    Ok(ExitCode::SUCCESS)
}

fn saved_target_matches_current(saved: &DeployTarget, current: &DeployTarget) -> bool {
    if saved == current {
        return true;
    }
    // Receipts from before log observation was added have no observation
    // fields. Keep their exact approved deploy commands attachable.
    let DeployTarget {
        env,
        production,
        ssh_alias,
        path,
        branch,
        steps,
        maintenance,
        watch_log,
        log,
        log_daily,
        smoke_url,
    } = saved;
    !watch_log
        && log.is_none()
        && !log_daily
        && smoke_url.is_none()
        && *env == current.env
        && *production == current.production
        && *ssh_alias == current.ssh_alias
        && *path == current.path
        && *branch == current.branch
        && *steps == current.steps
        && *maintenance == current.maintenance
}

fn show_trust_changes(previous: Option<&TrustSnapshot>, current: &TrustSnapshot) {
    // Destructured without `..` so a new setting must be shown here.
    let TrustSnapshot {
        ssh_alias,
        path,
        branch,
        production,
        maintenance,
        log,
        log_daily,
        smoke_url,
        steps,
    } = current;
    show_change(
        "ssh",
        previous.map(|old| old.ssh_alias.clone()),
        ssh_alias.clone(),
    );
    show_change("path", previous.map(|old| old.path.clone()), path.clone());
    show_change(
        "branch",
        previous.map(|old| old.branch.clone()),
        branch.clone(),
    );
    show_change(
        "production",
        previous.map(|old| old.production.to_string()),
        production.to_string(),
    );
    show_change(
        "maintenance",
        previous.map(|old| old.maintenance.to_string()),
        maintenance.to_string(),
    );
    show_change(
        "log",
        previous.map(|old| format!("{:?}", old.log)),
        format!("{log:?}"),
    );
    show_change(
        "log_daily",
        previous.map(|old| old.log_daily.to_string()),
        log_daily.to_string(),
    );
    show_change(
        "smoke_url",
        previous.map(|old| format!("{:?}", old.smoke_url)),
        format!("{smoke_url:?}"),
    );
    if previous.is_none_or(|old| old.steps != *steps) {
        if let Some(old) = previous {
            println!("  recipe steps before:");
            for (index, step) in old.steps.iter().enumerate() {
                println!("    {}. {step}", index + 1);
            }
        }
        println!("  recipe steps now:");
        for (index, step) in steps.iter().enumerate() {
            println!("    {}. {step}", index + 1);
        }
    }
}

fn show_change(label: &str, previous: Option<String>, current: String) {
    if previous.as_deref() == Some(current.as_str()) {
        return;
    }
    match previous {
        Some(previous) => println!("  {label}: {previous} -> {current}"),
        None => println!("  {label}: {current}"),
    }
}

async fn confirm(
    preview: shipslip::Preview,
    target: &DeployTarget,
    transport: &SshTransport,
    journal: &ReceiptJournal,
    interrupts: &mut Interrupts,
) -> Result<Option<(shipslip::Preview, Confirmation)>, Box<dyn Error>> {
    if target.production {
        print!("Type the environment name `{}` to proceed: ", target.env);
    } else {
        print!("Proceed with this run? [y/N] ");
    }
    let answer = match io::stdout().flush() {
        Ok(()) => ask(read_answer, interrupts).await,
        Err(error) => Err(error),
    };
    let answer = match answer {
        Ok(Some(answer)) => answer,
        Ok(None) => {
            println!();
            interrupts.pending = true;
            interrupts
                .defer(cancel_prepared(preview, transport, journal), "")
                .await?;
            println!("Cancelled; no deploy steps were run.");
            exit_interrupted();
        }
        Err(error) => {
            cancel_prepared(preview, transport, journal).await?;
            return Err(error.into());
        }
    };

    let approved = if target.production {
        answer == target.env
    } else {
        matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes")
    };
    if !approved {
        cancel_prepared(preview, transport, journal).await?;
        if target.production {
            println!("Environment name did not match; no deploy steps were run.");
        } else {
            println!("Cancelled; no deploy steps were run.");
        }
        return Ok(None);
    }

    let typed_env = target.production.then_some(answer.as_str());
    let confirmation = match Confirmation::from(&preview, typed_env) {
        Ok(confirmation) => confirmation,
        Err(error) => {
            cancel_prepared(preview, transport, journal).await?;
            return Err(error.into());
        }
    };
    Ok(Some((preview, confirmation)))
}

async fn cancel_prepared(
    preview: shipslip::Preview,
    transport: &SshTransport,
    journal: &ReceiptJournal,
) -> Result<(), Box<dyn Error>> {
    let save = journal.record_outcome(DeployOutcome::CancelledBeforeChanges);
    let release = cancel(preview, transport).await;
    save?;
    release?;
    journal.finish(DeployOutcome::CancelledBeforeChanges)?;
    Ok(())
}

fn show_step_result(index: usize, status: StepStatus, exit_code: Option<i32>) {
    println!(
        "Step {index}: {}{}",
        status_name(&status),
        exit_code
            .map(|code| format!(" (exit {code})"))
            .unwrap_or_default()
    );
}

fn show_outcome(outcome: &DeployOutcome) {
    match outcome {
        DeployOutcome::Succeeded => println!("Deploy run succeeded."),
        DeployOutcome::FailedAtStep {
            step,
            partial_update,
        } => eprintln!(
            "Deploy failed at step {step}{}.",
            if *partial_update {
                "; the server may have been partially updated"
            } else {
                ""
            }
        ),
        DeployOutcome::StoppedAfterStep { step, reason } => {
            eprintln!("Deploy stopped after step {step}: {reason}.");
        }
        DeployOutcome::CancelledBeforeChanges => eprintln!("Deploy was cancelled before changes."),
        DeployOutcome::AbortedBeforeChanges(reason) => {
            eprintln!("Deploy was aborted before changes: {reason}.");
        }
        DeployOutcome::Unknown { step, reason } => {
            eprintln!("Outcome of step {step} is unknown: {reason}");
        }
    }
}

fn phase_name(phase: MaintenancePhase) -> &'static str {
    match phase {
        MaintenancePhase::Down => "on",
        MaintenancePhase::Up => "off",
    }
}

fn status_name(status: &StepStatus) -> &'static str {
    match status {
        StepStatus::Ok => "succeeded",
        StepStatus::Failed => "failed",
        StepStatus::Unknown => "unknown",
        StepStatus::NotStarted => "not started",
    }
}

fn read_answer() -> io::Result<String> {
    read_line_from(io::stdin().lock())
}

fn read_line_from(mut input: impl BufRead) -> io::Result<String> {
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(answer.trim().to_string())
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

struct Command {
    config: Option<PathBuf>,
    action: Action,
}

enum Action {
    Init,
    Run { environment: String, plan: RunPlan },
    Trust { environment: Option<String> },
    Attach { environment: String },
    BreakLock { environment: String },
    Up { environment: String },
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io::BufReader;

    use super::*;

    #[derive(Default)]
    struct Controls {
        detached: Cell<usize>,
        watch_cancelled: Cell<usize>,
    }

    impl RunControls for Controls {
        fn detach(&self) {
            self.detached.set(self.detached.get() + 1);
        }

        fn cancel_watch(&self) {
            self.watch_cancelled.set(self.watch_cancelled.get() + 1);
        }
    }

    enum Input {
        Event(DeployEvent),
        CtrlC,
    }

    /// Follows a run fed `inputs` one at a time.
    async fn follow(inputs: Vec<Input>) -> (ExitCode, Controls) {
        let controls = Controls::default();
        let (events_tx, events) = mpsc::unbounded_channel();
        let (ctrl_c, rx) = mpsc::unbounded_channel();
        let mut interrupts = Interrupts::from_channel(rx);
        let view = RunView {
            env: "staging".into(),
            log: "storage/logs/laravel.log".into(),
        };
        let feed = async move {
            for input in inputs {
                match input {
                    Input::Event(event) => {
                        let _ = events_tx.send(event);
                    }
                    Input::CtrlC => {
                        let _ = ctrl_c.send(());
                    }
                }
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }
            }
            // Keep Ctrl-C open while the run's events end.
            ctrl_c
        };
        let (code, _ctrl_c) = tokio::join!(
            follow_events(events, &controls, &mut interrupts, &view),
            feed
        );
        (code.unwrap(), controls)
    }

    fn watch_finished(status: WatchStatus) -> DeployEvent {
        let mut result = WatchResult::not_run();
        result.status = status;
        DeployEvent::WatchFinished(result)
    }

    fn step_started() -> DeployEvent {
        DeployEvent::StepStarted {
            index: 1,
            name: "migrate".into(),
        }
    }

    #[test]
    fn ctrl_c_acts_on_the_current_stage_and_quits_when_repeated() {
        assert_eq!(on_interrupt(Stage::Running, false), Interrupt::Detach);
        assert_eq!(on_interrupt(Stage::BringingUp, false), Interrupt::SkipWatch);
        assert_eq!(on_interrupt(Stage::Watching, false), Interrupt::StopWatch);
        assert_eq!(on_interrupt(Stage::Finishing, false), Interrupt::Wait);
        for stage in [
            Stage::Running,
            Stage::BringingUp,
            Stage::Watching,
            Stage::Finishing,
        ] {
            assert_eq!(on_interrupt(stage, true), Interrupt::Quit);
        }
    }

    #[test]
    fn stages_follow_maintenance_up_and_the_log_watch() {
        let up = |phase| DeployEvent::MaintenanceStarted { phase };
        assert_eq!(
            next_stage(Stage::Running, &up(MaintenancePhase::Down)),
            Stage::Running
        );
        assert_eq!(next_stage(Stage::Running, &step_started()), Stage::Running);
        assert_eq!(
            next_stage(Stage::Running, &up(MaintenancePhase::Up)),
            Stage::BringingUp
        );
        assert_eq!(
            next_stage(
                Stage::BringingUp,
                &DeployEvent::MaintenanceFinished {
                    phase: MaintenancePhase::Up,
                    status: StepStatus::Ok,
                    exit_code: Some(0),
                }
            ),
            Stage::Finishing
        );
        assert_eq!(
            next_stage(
                Stage::Finishing,
                &DeployEvent::WatchStarted {
                    window: Duration::from_secs(120)
                }
            ),
            Stage::Watching
        );
        assert_eq!(
            next_stage(Stage::Watching, &watch_finished(WatchStatus::Complete)),
            Stage::Finishing
        );
    }

    #[tokio::test]
    async fn ctrl_c_during_the_watch_stops_only_the_watch() {
        let (code, controls) = follow(vec![
            Input::Event(DeployEvent::WatchStarted {
                window: Duration::from_secs(120),
            }),
            Input::CtrlC,
            Input::Event(watch_finished(WatchStatus::Cancelled)),
            Input::Event(DeployEvent::Finished(DeployOutcome::Succeeded)),
        ])
        .await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(controls.watch_cancelled.get(), 1);
        assert_eq!(controls.detached.get(), 0);
    }

    #[tokio::test]
    async fn ctrl_c_during_a_step_detaches_and_a_second_one_quits() {
        let (code, controls) = follow(vec![
            Input::Event(step_started()),
            Input::CtrlC,
            Input::CtrlC,
        ])
        .await;
        assert_eq!(code, ExitCode::from(INTERRUPTED));
        assert_eq!(controls.detached.get(), 1);
    }

    #[tokio::test]
    async fn ctrl_c_in_a_new_stage_does_not_quit() {
        // A stop between steps still runs the post-deploy watch.
        let (code, controls) = follow(vec![
            Input::Event(step_started()),
            Input::CtrlC,
            Input::Event(DeployEvent::WatchStarted {
                window: Duration::from_secs(120),
            }),
            Input::CtrlC,
            Input::Event(watch_finished(WatchStatus::Cancelled)),
            Input::Event(DeployEvent::Finished(DeployOutcome::StoppedAfterStep {
                step: 1,
                reason: shipslip::StopReason::Requested,
            })),
        ])
        .await;
        assert_eq!(code, ExitCode::FAILURE);
        assert_eq!(controls.detached.get(), 1);
        assert_eq!(controls.watch_cancelled.get(), 2);
    }

    #[tokio::test]
    async fn ctrl_c_while_bringing_the_app_up_skips_the_watch_but_waits() {
        let (code, controls) = follow(vec![
            Input::Event(DeployEvent::MaintenanceStarted {
                phase: MaintenancePhase::Up,
            }),
            Input::CtrlC,
            Input::Event(DeployEvent::MaintenanceFinished {
                phase: MaintenancePhase::Up,
                status: StepStatus::Ok,
                exit_code: Some(0),
            }),
            Input::Event(DeployEvent::Finished(DeployOutcome::Succeeded)),
        ])
        .await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(controls.detached.get(), 0);
        assert_eq!(controls.watch_cancelled.get(), 1);
    }

    #[tokio::test]
    async fn ctrl_c_ends_a_prompt_that_is_still_waiting_for_input() {
        let (reader, writer) = io::pipe().unwrap();
        let (ctrl_c, rx) = mpsc::unbounded_channel();
        let mut interrupts = Interrupts::from_channel(rx);
        ctrl_c.send(()).unwrap();
        let answer = tokio::time::timeout(
            Duration::from_secs(5),
            ask(
                move || read_line_from(BufReader::new(reader)),
                &mut interrupts,
            ),
        )
        .await
        .expect("Ctrl-C must not wait for input")
        .unwrap();
        assert_eq!(answer, None);
        // Lets the blocked reader thread finish so the runtime can shut down.
        drop(writer);
    }

    #[tokio::test]
    async fn prompt_returns_the_typed_answer() {
        let (reader, mut writer) = io::pipe().unwrap();
        let (_ctrl_c, rx) = mpsc::unbounded_channel();
        let mut interrupts = Interrupts::from_channel(rx);
        writer.write_all(b" yes \n").unwrap();
        let answer = ask(
            move || read_line_from(BufReader::new(reader)),
            &mut interrupts,
        )
        .await
        .unwrap();
        assert_eq!(answer.as_deref(), Some("yes"));
    }

    #[tokio::test]
    async fn deferred_work_finishes_after_one_ctrl_c() {
        let (ctrl_c, rx) = mpsc::unbounded_channel();
        let mut interrupts = Interrupts::from_channel(rx);
        let (done, finished) = tokio::sync::oneshot::channel();
        let work = async move { finished.await.unwrap() };
        let feed = async move {
            ctrl_c.send(()).unwrap();
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
            done.send(7).unwrap();
            ctrl_c
        };
        let (output, _ctrl_c) = tokio::join!(interrupts.defer(work, "cancelling"), feed);
        assert_eq!(output, 7);
        assert!(interrupts.pending);
    }

    #[test]
    fn watch_progress_shows_whole_seconds_left() {
        assert_eq!(
            watch_left_text(Duration::from_millis(89_600)).as_deref(),
            Some("Log watch: 90 s left")
        );
        assert_eq!(watch_left_text(Duration::from_millis(400)), None);
        assert_eq!(watch_left_text(Duration::ZERO), None);
    }

    #[test]
    fn watch_summary_uses_plain_words() {
        let summary = |status| {
            let mut result = WatchResult::not_run();
            result.status = status;
            watch_summary(&result)
        };
        assert_eq!(
            summary(WatchStatus::Complete),
            "Log watch: complete, no new errors"
        );
        assert_eq!(
            summary(WatchStatus::Cancelled),
            "Log watch: stopped early, no new errors"
        );
        assert_eq!(summary(WatchStatus::NotRun), "Log watch: not run");
        assert_eq!(
            summary(WatchStatus::NoLogSeen),
            "Log watch: no log file found"
        );
        assert_eq!(
            summary(WatchStatus::Unavailable),
            "Log watch: log could not be read"
        );
    }

    fn run_init(script: &str, branch: Option<&str>) -> (io::Result<InitAnswers>, String) {
        let mut output = Vec::new();
        let answers = ask_init(&mut io::Cursor::new(script), &mut output, branch);
        (answers, String::from_utf8(output).unwrap())
    }

    #[test]
    fn init_takes_defaults_for_empty_answers() {
        let (answers, output) = run_init("\napp-staging\n/var/www/app\n\n\n\n\n", Some("main"));
        assert_eq!(
            answers.unwrap(),
            InitAnswers {
                env: "staging".into(),
                ssh_alias: "app-staging".into(),
                path: "/var/www/app".into(),
                branch: "main".into(),
                production: false,
                maintenance: true,
                smoke_url: None,
            }
        );
        assert!(output.contains("Environment name [staging]: "), "{output}");
        assert!(output.contains("Branch to deploy [main]: "), "{output}");
        assert!(output.contains("Is this a production environment? [y/N] "));
    }

    #[test]
    fn init_asks_again_until_answers_are_valid() {
        let (answers, output) = run_init(
            "prod env\nproduction\n\nubuntu@1.2.3.4\napp-prod\nvar/www\n/var/www/app\n\
             -main\nmain\nmaybe\nyes\nn\nexample.com\nhttps://example.com/health\n",
            None,
        );
        let answers = answers.unwrap();
        assert_eq!(answers.env, "production");
        assert_eq!(answers.ssh_alias, "app-prod");
        assert_eq!(answers.path, "/var/www/app");
        assert_eq!(answers.branch, "main");
        assert!(answers.production);
        assert!(!answers.maintenance);
        assert_eq!(
            answers.smoke_url.as_deref(),
            Some("https://example.com/health")
        );
        for hint in [
            "Use only letters, digits",
            "An answer is required.",
            "not user@host",
            "Use an absolute path",
            "not a supported branch name",
            "Answer y or n.",
            "Use an http:// or https:// URL.",
        ] {
            assert!(output.contains(hint), "missing {hint:?} in {output}");
        }
    }

    #[test]
    fn init_stops_when_input_ends() {
        let (answers, _) = run_init("staging\napp-staging\n", Some("main"));
        assert_eq!(answers.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn init_takes_no_arguments() {
        let parsed = parse_args_from(vec!["init".into()]).unwrap().unwrap();
        assert!(matches!(parsed.action, Action::Init));
        assert!(parse_args_from(vec!["init".into(), "staging".into()]).is_err());
    }

    #[test]
    fn recovery_commands_require_exactly_one_environment() {
        for name in ["break-lock", "up"] {
            let parsed = parse_args_from(vec![name.into(), "staging".into()])
                .unwrap()
                .unwrap();
            match parsed.action {
                Action::BreakLock { environment } | Action::Up { environment } => {
                    assert_eq!(environment, "staging")
                }
                _ => panic!("{name} parsed as another action"),
            }
            assert!(parse_args_from(vec![name.into()]).is_err());
            assert!(parse_args_from(vec![name.into(), "staging".into(), "extra".into()]).is_err());
        }
    }
}
