use std::collections::BTreeMap;
use std::error::Error;
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use serde::Deserialize;
use shipslip::transport::SshTransport;
use shipslip::{
    cancel, execute, prepare_with_plan, Confirmation, DeployEvent, DeployOutcome, DeployTarget,
    MaintenancePhase, RunPlan, StepStatus,
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

    let config_file = File::open(&command.config)?;
    let config: Config = serde_json::from_reader(config_file)?;
    let environment = config
        .environments
        .get(&command.environment)
        .ok_or_else(|| {
            invalid_input(format!(
                "environment `{}` is not in the config",
                command.environment
            ))
        })?;
    let target = environment.to_target(&command.environment);

    let transport = Arc::new(SshTransport::connect(&target.ssh_alias).await?);
    let preview = prepare_with_plan(target.clone(), command.plan, transport.as_ref()).await?;
    show_preview(&preview);

    let Some((preview, confirmation)) = confirm(preview, &target, transport.as_ref()).await? else {
        return Ok(ExitCode::SUCCESS);
    };

    let (mut events, _handle) = execute(preview, confirmation, transport)?;
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
            DeployEvent::Detached { index } => {
                println!("Stopped observing step {index}; it continues on the server.");
            }
            DeployEvent::Interrupted { index, reason } => {
                eprintln!("Lost contact while observing step {index}: {reason}");
            }
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
    let args: Vec<String> = std::env::args().skip(1).collect();
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
        PathBuf::from(path)
    } else {
        std::env::var_os("SHIPSLIP_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("shipslip.json"))
    };

    let action = args
        .get(index)
        .ok_or_else(|| invalid_input("missing command"))?;
    index += 1;
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
        environment,
        plan,
    }))
}

fn print_help() {
    println!(
        "Shipslip deploy runner\n\n\
         Usage:\n\
         \x20 slip [--config FILE] <deploy|rerun|from-step> <ENV> [STEP]\n\n\
         Commands:\n\
         \x20 deploy ENV         Fast-forward the checkout and run all recipe steps\n\
         \x20 rerun ENV          Run all recipe steps on the already-deployed commit\n\
         \x20 from-step ENV STEP Run recipe steps starting at STEP (steps start at 1)\n\n\
         Config defaults to ./shipslip.json, or SHIPSLIP_CONFIG."
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
    println!("Path:        {}", preview.target().path);
    println!("Current SHA: {}", preview.from_sha());
    println!("Target SHA:  {}", preview.target_sha());
    if preview.run_plan() == RunPlan::Deploy && !preview.commits().is_empty() {
        println!("Commits to deploy:");
        for commit in preview.commits() {
            println!("  {commit}");
        }
    }
    if preview.target().maintenance {
        println!("Maintenance: enabled");
    }
    if preview.target().production {
        println!("This is a production environment.");
    }
}

async fn confirm(
    preview: shipslip::Preview,
    target: &DeployTarget,
    transport: &SshTransport,
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
            cancel(preview, transport).await?;
            return Err(error.into());
        }
    };

    let approved = if target.production {
        answer == target.env
    } else {
        matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes")
    };
    if !approved {
        cancel(preview, transport).await?;
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
            cancel(preview, transport).await?;
            return Err(error.into());
        }
    };
    Ok(Some((preview, confirmation)))
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
    config: PathBuf,
    environment: String,
    plan: RunPlan,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    environments: BTreeMap<String, Environment>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Environment {
    production: bool,
    ssh_alias: String,
    path: String,
    branch: String,
    #[serde(default)]
    steps: Vec<String>,
    #[serde(default)]
    maintenance: bool,
}

impl Environment {
    fn to_target(&self, name: &str) -> DeployTarget {
        DeployTarget {
            env: name.to_string(),
            production: self.production,
            ssh_alias: self.ssh_alias.clone(),
            path: self.path.clone(),
            branch: self.branch.clone(),
            steps: self.steps.clone(),
            maintenance: self.maintenance,
        }
    }
}
