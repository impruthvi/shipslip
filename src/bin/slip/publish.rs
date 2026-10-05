use std::error::Error;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::ExitCode;

use shipslip::create::default_operations_root;
use shipslip::logs::escape;
use shipslip::publish::{
    self, PublishError, PublishEvent, PublishRequest, PublishResult, PublishTools, Visibility,
};

use super::{ask_typed, ask_value, invalid_input, new::yes_no, Interrupts, INTERRUPTED};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Args {
    owner: Option<String>,
    name: Option<String>,
    visibility: Option<Visibility>,
}
impl Args {
    pub(super) fn parse(args: &[String]) -> Result<Self, Box<dyn Error>> {
        let mut result = Self::default();
        let mut index = 0;
        while index < args.len() {
            let (flag, inline) = args[index]
                .split_once('=')
                .map_or((args[index].as_str(), None), |(flag, value)| {
                    (flag, Some(value))
                });
            if !matches!(flag, "--owner" | "--repo" | "--visibility") {
                return Err(invalid_input(format!("unknown publish option {flag}")).into());
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
            if value.is_empty() {
                return Err(invalid_input(format!("{flag} requires a value")).into());
            }
            match flag {
                "--owner" => set(&mut result.owner, value.to_string(), flag)?,
                "--repo" => set(&mut result.name, value.to_string(), flag)?,
                "--visibility" => set(
                    &mut result.visibility,
                    match value {
                        "private" => Visibility::Private,
                        "public" => Visibility::Public,
                        _ => {
                            return Err(
                                invalid_input("--visibility must be private or public").into()
                            )
                        }
                    },
                    flag,
                )?,
                _ => unreachable!(),
            }
            index += 1;
        }
        Ok(result)
    }
}
fn set<T>(slot: &mut Option<T>, value: T, flag: &str) -> io::Result<()> {
    if slot.is_some() {
        return Err(invalid_input(format!("{flag} was provided twice")));
    }
    *slot = Some(value);
    Ok(())
}
fn name_check(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 100
        || value.starts_with('-')
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Err("Use letters, numbers, dots, dashes or underscores.")
    } else {
        Ok(())
    }
}
async fn prompt(
    question: &str,
    default: &str,
    check: fn(&str) -> Result<(), &'static str>,
    interrupts: &mut Interrupts,
) -> io::Result<String> {
    let question = question.to_string();
    let default = default.to_string();
    let answer = ask_typed(
        move || {
            ask_value(
                &mut io::stdin().lock(),
                &mut io::stdout(),
                &question,
                Some(&default),
                check,
            )
        },
        interrupts,
    )
    .await?;
    Ok(answer.unwrap_or_else(|| super::exit_interrupted()))
}
fn tools() -> Result<PublishTools, Box<dyn Error>> {
    Ok(shipslip::setup::detect_tools(
        &shipslip::setup::DetectionContext::from_environment(),
        &["git", "gh"],
    )
    .publish_tools()?)
}

#[derive(Debug)]
pub(super) struct Publication {
    result: PublishResult,
    visibility: Visibility,
}

pub(super) async fn run(args: Args) -> Result<ExitCode, Box<dyn Error>> {
    let mut interrupts = Interrupts::listen();
    run_at(args, &std::env::current_dir()?, &mut interrupts).await?;
    Ok(if interrupts.pending {
        ExitCode::from(INTERRUPTED)
    } else {
        ExitCode::SUCCESS
    })
}

pub(super) async fn run_at(
    args: Args,
    start: &Path,
    interrupts: &mut Interrupts,
) -> Result<Option<Publication>, Box<dyn Error>> {
    let mut reviewed = false;
    let result = run_at_inner(args, start, interrupts, &mut reviewed).await;
    if reviewed && result.is_err() {
        eprintln!(
            "Publication was not verified by this invocation. Any completed pushes and created GitHub repositories are retained."
        );
        eprintln!(
            "From {}, rerun slip publish github with the same owner, repository name and visibility to review or resume publication.",
            escape(&start.display().to_string())
        );
    }
    result
}

async fn run_at_inner(
    mut args: Args,
    start: &Path,
    interrupts: &mut Interrupts,
    reviewed: &mut bool,
) -> Result<Option<Publication>, Box<dyn Error>> {
    if !io::stdin().is_terminal() {
        let flag = if args.name.is_none() {
            Some("--repo")
        } else if args.visibility.is_none() {
            Some("--visibility")
        } else {
            None
        };
        return Err(invalid_input(match flag {
            Some(flag) => format!("stdin is not a terminal; pass {flag} explicitly"),
            None => {
                "publish needs an interactive terminal to review and confirm GitHub publication"
                    .into()
            }
        })
        .into());
    }
    let tools = tools()?;
    let root = tools.git.repository_root(start)?;
    let default_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("my-app");
    let mut name = match args.name.take() {
        Some(name) => name,
        None => {
            prompt(
                "GitHub repository name",
                default_name,
                name_check,
                interrupts,
            )
            .await?
        }
    };
    let visibility = match args.visibility {
        Some(visibility) => visibility,
        None => {
            match prompt(
                "Visibility (private/public)",
                "private",
                |value| {
                    if matches!(value, "private" | "public") {
                        Ok(())
                    } else {
                        Err("Choose private or public.")
                    }
                },
                interrupts,
            )
            .await?
            .as_str()
            {
                "public" => Visibility::Public,
                _ => Visibility::Private,
            }
        }
    };
    loop {
        let operations_root = default_operations_root()?;
        let preview_work = publish::preview(
            PublishRequest {
                root: root.clone(),
                owner: args.owner.clone(),
                name: name.clone(),
                visibility,
            },
            &tools,
            &operations_root,
        );
        let preview = tokio::select! {
            result = preview_work => result?,
            () = interrupts.recv() => {
                interrupts.pending = true;
                println!("Publication review cancelled.");
                return Ok(None);
            }
        };
        *reviewed = true;
        println!("\nPublish to GitHub");
        println!("Authenticated account: {}", escape(&preview.account));
        println!(
            "Repository: {} ({})",
            escape(&preview.repository()),
            if visibility == Visibility::Private {
                "private"
            } else {
                "public"
            }
        );
        println!("Branch: {}", escape(&preview.branch));
        println!("Reviewed HEAD: {}", escape(&preview.head));
        println!(
            "Commits in pushed history: {}",
            preview.history.commit_count
        );
        println!("Remote: {}", escape(&preview.remote_url));
        if !preview.history.violations.is_empty() {
            for violation in &preview.history.violations {
                eprintln!(
                    "Blocked secret: commit {} — {}",
                    escape(&violation.commit),
                    escape(&violation.path)
                );
            }
            return Err(PublishError::Secrets.into());
        }
        let status = tools.git.status(&root)?;
        if !status.trim().is_empty() {
            println!(
                "Local changes below are not included in this push:\n{}",
                escape(&status)
            );
        }
        if yes_no(
            "Create/resume this repository and publish the reviewed commit?",
            false,
            interrupts,
        )
        .await?
            != Some(true)
        {
            println!("Reviewed commit was not published; local commits remain available.");
            return Ok(None);
        }
        let confirmation = preview.confirm();
        let result = interrupts.defer(publish::execute_publish(&preview, confirmation, &tools, |event| println!("{}", match event {
            PublishEvent::IntentSaved => "Publication intent saved.", PublishEvent::RepositoryCreated => "GitHub repository created.", PublishEvent::RepositoryAdopted => "Resumed the marked empty GitHub repository.", PublishEvent::Pushed => "Reviewed commit pushed.", PublishEvent::Verified => "Remote SHA verified.", PublishEvent::DescriptionCleared => "Temporary repository marker cleared.", PublishEvent::Done => "Publication complete.",
        })), "Finishing the current publication safely; press Ctrl-C again to quit. Rerun slip publish github to resume an unfinished operation.").await;
        match result {
            Ok(result) => {
                println!(
                    "\nPublished {}\nVerified commit: {}",
                    escape(&result.url),
                    escape(&result.head)
                );
                return Ok(Some(Publication { result, visibility }));
            }
            Err(PublishError::Unresolved(repository)) => {
                drop(preview);
                eprintln!(
                    "{} cannot be safely adopted. The existing repository was not changed.",
                    escape(&repository)
                );
                if interrupts.pending
                    || yes_no("Choose a new repository name?", false, interrupts).await?
                        != Some(true)
                {
                    return Err(PublishError::Unresolved(repository).into());
                }
                name = prompt(
                    "New GitHub repository name",
                    &format!("{name}-new"),
                    name_check,
                    interrupts,
                )
                .await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(super) async fn offer_config_commit(
    root: &Path,
    path: &Path,
    publication: &Publication,
    interrupts: &mut Interrupts,
) -> Result<(), Box<dyn Error>> {
    let contents = fs::read(path)?;
    let tools = tools()?;
    let identity = tools.git.identity(root)?;
    println!(
        "Commit identity: {} <{}>",
        escape(&identity.name),
        escape(&identity.email)
    );
    println!("\nDeployment config diff:\n--- /dev/null\n+++ .shipslip.toml");
    for line in String::from_utf8_lossy(&contents).lines() {
        println!("+{}", escape(line));
    }
    if yes_no(
        "Commit and publish only this deployment config?",
        false,
        interrupts,
    )
    .await?
        != Some(true)
    {
        println!("Deployment config remains local-only.");
        return Ok(());
    }
    if fs::read(path)? != contents
        || tools.git.head(root)? != publication.result.head
        || tools.git.current_branch(root)? != publication.result.branch
    {
        return Err(invalid_input(
            "config or repository changed during review; review the config and publish again",
        )
        .into());
    }
    let head =
        tools
            .git
            .commit_config_reviewed(root, &contents, &publication.result.head, &identity)?;
    println!("Deployment config committed as {}", escape(&head));
    let (owner, name) = publication
        .result
        .repository
        .split_once('/')
        .ok_or_else(|| invalid_input("invalid published repository"))?;
    let result = run_at(
        Args {
            owner: Some(owner.into()),
            name: Some(name.into()),
            visibility: Some(publication.visibility),
        },
        root,
        interrupts,
    )
    .await;
    let published = match result {
        Ok(published) => published,
        Err(error) => {
            eprintln!(
                "Deployment config commit {} is retained locally; remote publication was not verified.",
                escape(&head)
            );
            return Err(error);
        }
    };
    if published.is_none() {
        println!("Deployment config commit remains local-only.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn argument_contract() {
        let parse = |args: &[&str]| {
            Args::parse(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(parse(&[]).unwrap(), Args::default());
        let args = parse(&["--owner=team", "--repo", "demo", "--visibility", "public"]).unwrap();
        assert_eq!(args.owner.as_deref(), Some("team"));
        assert_eq!(args.name.as_deref(), Some("demo"));
        assert_eq!(args.visibility, Some(Visibility::Public));
        for args in [
            vec!["--repo"],
            vec!["--source", "."],
            vec!["--force"],
            vec!["--visibility", "hidden"],
            vec!["--owner=one", "--owner=two"],
            vec!["--repo="],
            vec!["extra"],
        ] {
            assert!(parse(&args).is_err());
        }
    }
    #[tokio::test]
    async fn missing_non_tty_choices_fail_before_gh_authentication() {
        if io::stdin().is_terminal() {
            return;
        }
        let mut interrupts = Interrupts::from_channel(tokio::sync::mpsc::unbounded_channel().1);
        let error = run_at(Args::default(), Path::new("."), &mut interrupts)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("--repo"));
    }
}
