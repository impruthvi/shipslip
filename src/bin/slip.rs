use std::error::Error;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use shipslip::config::{
    approve_trust, default_trust_path, trust_status, LoadedConfig, TrustSnapshot, TrustStatus,
};
use shipslip::receipt::{default_receipts_root, find_open, ReceiptJournal};
use shipslip::transport::SshTransport;
use shipslip::{
    attach, break_lock, bring_app_up, cancel, execute_recorded, lock_status, prepare_with_plan,
    BreakLockError, BringUpError, Confirmation, DeployEvent, DeployOutcome, DeployTarget,
    MaintenancePhase, PrepareError, RunPlan, SmokeResult, StepStatus, WatchStatus,
};

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
    let config = LoadedConfig::load(&std::env::current_dir()?, command.config.as_deref())?;
    if config.uses_default_recipe() {
        eprintln!("Using the default Laravel deploy recipe; add [recipe.deploy] to customize it.");
    }
    let trust_path = default_trust_path()?;
    let receipts_root = default_receipts_root()?;
    let (environment, plan) = match command.action {
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
    let preview = prepare_with_plan(target.clone(), plan, transport.as_ref())
        .await
        .map_err(|error| match error {
            PrepareError::LockHeld(info) => format!(
                "deploy lock is {info}; run `slip attach {environment}` to resume an unfinished \
                 run, or check the server and run `slip break-lock {environment}` if it is stale"
            )
            .into(),
            error => Box::<dyn Error>::from(error),
        })?;
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

    let Some((preview, confirmation)) =
        confirm(preview, &target, transport.as_ref(), &journal).await?
    else {
        return Ok(ExitCode::SUCCESS);
    };
    let cancel_preview = preview.clone();
    let (events, _handle) =
        match execute_recorded(preview, confirmation, transport.clone(), journal.clone()) {
            Ok(run) => run,
            Err(error) => {
                cancel_prepared(cancel_preview, transport.as_ref(), &journal).await?;
                return Err(error.into());
            }
        };
    follow_events(events).await
}

async fn follow_events(
    mut events: tokio::sync::mpsc::UnboundedReceiver<DeployEvent>,
) -> Result<ExitCode, Box<dyn Error>> {
    let mut succeeded = false;
    while let Some(event) = events.recv().await {
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
            DeployEvent::WatchFinished(result) => {
                println!(
                    "Log watch: {:?} ({} new groups)",
                    result.status,
                    result.new_errors.len()
                );
                if result.status == WatchStatus::NoLogSeen {
                    eprintln!("No log file appeared; check LOG_CHANNEL.");
                }
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
                println!("Stopped observing step {index}; it continues on the server.");
            }
            DeployEvent::Interrupted { index, reason } => {
                eprintln!("Lost contact while observing step {index}: {reason}");
            }
            DeployEvent::RunError { reason } => eprintln!("Deploy could not finish: {reason}"),
            DeployEvent::Finished(outcome) => {
                succeeded = matches!(&outcome, DeployOutcome::Succeeded);
                show_outcome(&outcome);
            }
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
         \x20 slip [--config FILE] trust [ENV]\n\
         \x20 slip [--config FILE] attach ENV\n\
         \x20 slip [--config FILE] break-lock ENV\n\
         \x20 slip [--config FILE] up ENV\n\n\
         Commands:\n\
         \x20 deploy ENV         Fast-forward the checkout and run all recipe steps\n\
         \x20 rerun ENV          Run all recipe steps on the already-deployed commit\n\
         \x20 from-step ENV STEP Run recipe steps starting at STEP (steps start at 1)\n\
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
    let first_step = match preview.run_plan() {
        RunPlan::Deploy | RunPlan::Rerun => 1,
        RunPlan::FromStep(step) => step,
    };
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
        let default = if preview.target().log_daily {
            "storage/logs/laravel"
        } else {
            "storage/logs/laravel.log"
        };
        println!(
            "Log watch:   {}{} (120 s after steps)",
            preview.target().log.as_deref().unwrap_or(default),
            if preview.target().log_daily {
                "-*.log"
            } else {
                ""
            }
        );
    }
    if let Some(url) = &preview.target().smoke_url {
        println!("Smoke URL:   {url}");
    }
    if preview.target().production {
        println!("This is a production environment.");
    }
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
    let (events, _handle) = attach(journal, transport)?;
    follow_events(events).await
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
) -> Result<Option<(shipslip::Preview, Confirmation)>, Box<dyn Error>> {
    let answer = (|| {
        if target.production {
            print!("Type the environment name `{}` to proceed: ", target.env);
        } else {
            print!("Proceed with this run? [y/N] ");
        }
        io::stdout().flush()?;
        read_answer()
    })();
    let answer = match answer {
        Ok(answer) => answer,
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
            eprintln!("Deploy stopped after step {step}: {reason:?}.");
        }
        DeployOutcome::CancelledBeforeChanges => eprintln!("Deploy was cancelled before changes."),
        DeployOutcome::AbortedBeforeChanges(reason) => {
            eprintln!("Deploy was aborted before changes: {reason:?}.");
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
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
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
    Run { environment: String, plan: RunPlan },
    Trust { environment: Option<String> },
    Attach { environment: String },
    BreakLock { environment: String },
    Up { environment: String },
}

#[cfg(test)]
mod tests {
    use super::*;

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
