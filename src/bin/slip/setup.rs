use std::error::Error;
use std::io;
use std::process::ExitCode;

use shipslip::logs::escape;
use shipslip::setup::{
    self, DetectionContext, DetectionOptions, DetectionReport, FindingState, Purpose,
};

use super::invalid_input;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Args {
    pub(super) purpose: Option<Purpose>,
}
impl Args {
    pub(super) fn parse(args: &[String]) -> io::Result<Self> {
        let mut result = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
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

pub(super) fn shell_line(dir: &std::path::Path) -> String {
    format!(
        "export PATH='{}':\"$PATH\"",
        dir.to_string_lossy().replace('\'', "'\\''")
    )
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
            let status = match &finding.state {
                FindingState::Ok { version, .. } => format!("ok ({version})"),
                FindingState::Missing => "missing".into(),
                FindingState::TooOld { found, need } => format!("too old ({found}; need {need})"),
                FindingState::Broken { error, .. } => format!("broken: {error}"),
                FindingState::OffPath { .. } => {
                    "warning: installed outside PATH; usable by slip".into()
                }
                FindingState::Unverified { reason } => format!("warning: unverified: {reason}"),
            };
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
    println!("{}", render(&report, &purposes));
    Ok(report_exit_code(&report))
}

pub(super) fn report_exit_code(report: &DetectionReport) -> ExitCode {
    if report.ready() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(3)
    }
}
