//! `slip pull ENV`: read a Cloud target's deployments into receipts.

use std::error::Error;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::ExitCode;

use shipslip::cloud::{self, CloudToken, CurlCloudApi, PullEvent};
use shipslip::config::{CloudTargetError, LoadedConfig};
use shipslip::logs::escape_field;

use super::{exit_interrupted, github_token::hidden_secret, invalid_input, Interrupts, DECLINED};

pub(super) fn parse(args: &[String]) -> io::Result<String> {
    match args {
        [environment] if !environment.starts_with('-') => Ok(environment.clone()),
        [] => Err(invalid_input("pull requires an environment name")),
        _ => Err(invalid_input("pull accepts one environment name")),
    }
}

pub(super) async fn run(
    config: &LoadedConfig,
    trust_path: &Path,
    receipts_root: &Path,
    environment: &str,
) -> Result<ExitCode, Box<dyn Error>> {
    let target = config
        .approved_cloud_target(environment, trust_path)
        .map_err(|error| -> Box<dyn Error> {
            match error {
                CloudTargetError::Store(error) => error.into(),
                error => invalid_input(error.to_string()).into(),
            }
        })?;
    if !io::stdin().is_terminal() {
        return Err(invalid_input(
            "the Laravel Cloud token is entered at a hidden prompt, which needs an interactive terminal",
        )
        .into());
    }
    let mut interrupts = Interrupts::listen();
    let Some(answer) = hidden_secret("Laravel Cloud API token", &mut interrupts).await? else {
        println!("Cancelled; nothing was pulled.");
        if interrupts.pending {
            exit_interrupted();
        }
        return Ok(DECLINED);
    };
    let token =
        CloudToken::new(answer.to_string()).map_err(|error| invalid_input(error.to_string()))?;
    drop(answer);
    let api = CurlCloudApi::new(token);

    let pulling = cloud::pull(&target, &api, receipts_root, show_event);
    // Pulls only read Laravel Cloud and write each receipt whole, so
    // stopping at once loses nothing already written.
    let report = tokio::select! {
        report = pulling => report?,
        () = interrupts.recv() => {
            println!("\nStopped; receipts already written are kept.");
            exit_interrupted();
        }
    };
    if report.written == 0 {
        println!("No new Cloud deployments to pull.");
    } else {
        println!(
            "Pulled {} Cloud deployment{}. List them: slip receipts {}",
            report.written,
            if report.written == 1 { "" } else { "s" },
            escape_field(environment)
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn show_event(event: PullEvent) {
    match event {
        PullEvent::Listing { cloud_environment } => println!(
            "Reading Laravel Cloud deployments of {}…",
            escape_field(&cloud_environment)
        ),
        PullEvent::Written {
            deployment_id,
            cloud_status,
        } => println!(
            "  pulled    {}  {}",
            escape_field(&deployment_id),
            escape_field(&cloud_status)
        ),
        PullEvent::AlreadyHeld { deployment_id } => {
            println!("  held      {}", escape_field(&deployment_id))
        }
        PullEvent::Skipped {
            deployment_id,
            cloud_status,
        } => println!(
            "  skipped   {}  {} (only succeeded deployments get a receipt for now)",
            escape_field(&deployment_id),
            escape_field(&cloud_status)
        ),
    }
}
