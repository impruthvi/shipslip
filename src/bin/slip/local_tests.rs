use super::*;
use std::fs::{self, File};
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::mpsc as sync_mpsc;

/// Use separate test processes so ambient config and stdin cannot affect CLI tests.
fn isolated(name: &str, config: Option<&str>) -> bool {
    isolated_output(name, config).is_some()
}

fn isolated_output(name: &str, config: Option<&str>) -> Option<std::process::Output> {
    if std::env::var("SHIPSLIP_LOCAL_TEST_CASE").as_deref() == Ok(name) {
        return None;
    }
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &format!("local_tests::{name}"), "--nocapture"])
        .env("SHIPSLIP_LOCAL_TEST_CASE", name)
        .env_remove("SHIPSLIP_CONFIG")
        .stdin(Stdio::null());
    if let Some(config) = config {
        command.env("SHIPSLIP_CONFIG", config);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(output)
}

fn parse(values: &[&str]) -> Result<Option<Command>, Box<dyn Error>> {
    parse_args_from(values.iter().map(|value| value.to_string()).collect())
}

#[test]
fn new_dispatch_preserves_choices_without_a_deploy_config() {
    if isolated(
        "new_dispatch_preserves_choices_without_a_deploy_config",
        None,
    ) {
        return;
    }
    use shipslip::laravel::{Auth, Database, StarterKit, Testing};
    for kit in ["none", "react", "vue", "svelte", "livewire"] {
        for auth in ["none", "laravel"] {
            let values = [
                "new",
                "my app",
                "--starter-kit",
                kit,
                "--auth",
                auth,
                "--database=pgsql",
                "--testing=phpunit",
                "--branch=release/app",
                "--boost",
            ];
            if kit == "none" && auth == "laravel" {
                assert!(parse(&values).is_err());
                continue;
            }
            let command = parse(&values).unwrap().unwrap();
            assert!(command.config.is_none());
            let Action::New(args) = command.action else {
                panic!("new must dispatch before deploy configuration");
            };
            assert_eq!(args.name, "my app");
            assert_eq!(args.starter_kit, Some(kit.parse::<StarterKit>().unwrap()));
            assert_eq!(args.auth, Some(auth.parse::<Auth>().unwrap()));
            assert_eq!(args.database, Some(Database::Pgsql));
            assert_eq!(args.testing, Some(Testing::Phpunit));
            assert_eq!(args.branch, "release/app");
            assert_eq!(args.boost, Some(true));
        }
    }
    let command = parse(&["new", "."]).unwrap().unwrap();
    assert!(matches!(command.action, Action::New(args) if args.name == "."));
}

#[test]
fn publish_dispatch_requires_the_github_provider() {
    if isolated("publish_dispatch_requires_the_github_provider", None) {
        return;
    }
    for values in [
        vec!["publish", "github"],
        vec![
            "publish",
            "github",
            "--owner=team",
            "--repo=app",
            "--visibility=private",
        ],
        vec![
            "publish",
            "github",
            "--repo",
            "app",
            "--visibility",
            "public",
        ],
    ] {
        let command = parse(&values).unwrap().unwrap();
        assert!(command.config.is_none());
        assert!(matches!(command.action, Action::Publish(_)));
    }
    for values in [
        vec!["publish"],
        vec!["publish", "gitlab"],
        vec!["publish", "--repo=app"],
    ] {
        let error = parse(&values).err().unwrap().to_string();
        assert!(error.contains("slip publish github"), "{error}");
    }
}

#[test]
fn local_commands_refuse_explicit_deploy_config() {
    if isolated("local_commands_refuse_explicit_deploy_config", None) {
        return;
    }
    for values in [
        vec!["--config", "/missing-config.toml", "new", "app"],
        vec!["--config", "/missing-config.toml", "publish", "github"],
        vec!["new", "app", "--config", "/missing-config.toml"],
        vec!["publish", "github", "--config", "/missing-config.toml"],
    ] {
        let error = parse(&values).err().unwrap().to_string();
        assert!(error.contains("--config"), "{error}");
    }
}

#[test]
fn local_commands_refuse_inherited_deploy_config() {
    if isolated(
        "local_commands_refuse_inherited_deploy_config",
        Some("/missing-config.toml"),
    ) {
        return;
    }
    for values in [vec!["new", "app"], vec!["publish", "github"]] {
        let error = parse(&values).err().unwrap().to_string();
        assert!(error.contains("unset SHIPSLIP_CONFIG"), "{error}");
    }
}

#[test]
fn local_commands_reject_unsupported_and_duplicate_flags() {
    if isolated(
        "local_commands_reject_unsupported_and_duplicate_flags",
        None,
    ) {
        return;
    }
    for values in [
        vec!["new", "app", "--force"],
        vec!["new", "app", "--auth=workos"],
        vec!["new", "app", "--starter-kit=none", "--auth=laravel"],
        vec!["new", "app", "--using=custom/kit"],
        vec!["new", "app", "--starter-kit=react", "--starter-kit=vue"],
        vec!["new", "app", "--branch=main", "--branch=other"],
        vec!["new", "app", "--database=sqlite", "--database=mysql"],
        vec!["new", "app", "--testing=pest", "--testing=phpunit"],
        vec!["new", "app", "--boost", "--boost"],
        vec!["publish", "github", "--source=app"],
        vec!["publish", "github", "--force"],
        vec!["publish", "github", "--owner=a", "--owner=b"],
        vec!["publish", "github", "--repo=a", "--repo=b"],
        vec![
            "publish",
            "github",
            "--visibility=private",
            "--visibility=public",
        ],
        vec!["publish", "github", "--visibility=internal"],
        vec!["publish", "github", "--repo="],
    ] {
        assert!(parse(&values).is_err(), "{values:?}");
    }
}

#[test]
fn non_terminal_new_names_each_missing_choice_before_tool_resolution() {
    if isolated(
        "non_terminal_new_names_each_missing_choice_before_tool_resolution",
        None,
    ) {
        return;
    }
    assert!(!io::stdin().is_terminal());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for (values, flag) in [
            (vec!["new", "app"], "--starter-kit"),
            (vec!["new", "app", "--starter-kit=react"], "--auth"),
            (vec!["new", "app", "--starter-kit=none"], "--database"),
            (
                vec!["new", "app", "--starter-kit=none", "--database=sqlite"],
                "--testing",
            ),
            (
                vec![
                    "new",
                    "app",
                    "--starter-kit=none",
                    "--database=sqlite",
                    "--testing=pest",
                ],
                "--boost or --no-boost",
            ),
        ] {
            let command = parse(&values).unwrap().unwrap();
            let Action::New(args) = command.action else {
                panic!("wrong dispatch");
            };
            let error = new::run(args).await.unwrap_err().to_string();
            assert!(
                error.contains(&format!("pass {flag} explicitly")),
                "{error}"
            );
            assert!(!error.contains("missing php"));
            assert!(!error.contains(".shipslip.toml"));
        }
        for boost in ["--boost", "--no-boost"] {
            let command = parse(&[
                "new",
                "app",
                "--starter-kit=none",
                "--database=sqlite",
                "--testing=pest",
                boost,
            ])
            .unwrap()
            .unwrap();
            let Action::New(args) = command.action else {
                panic!("wrong dispatch");
            };
            assert!(new::run(args)
                .await
                .unwrap_err()
                .to_string()
                .contains("interactive terminal"));
        }
    });
}

#[test]
fn non_terminal_publish_requires_review_before_github_calls() {
    if isolated(
        "non_terminal_publish_requires_review_before_github_calls",
        None,
    ) {
        return;
    }
    assert!(!io::stdin().is_terminal());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for (values, expected) in [
            (vec!["publish", "github"], "pass --repo explicitly"),
            (
                vec!["publish", "github", "--repo=app"],
                "pass --visibility explicitly",
            ),
            (
                vec!["publish", "github", "--repo=app", "--visibility=private"],
                "interactive terminal",
            ),
        ] {
            let command = parse(&values).unwrap().unwrap();
            let Action::Publish(args) = command.action else {
                panic!("wrong dispatch");
            };
            let error = publish::run(args).await.unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("gh auth login"));
            assert!(!error.contains(".shipslip.toml"));
        }
    });
}

#[test]
fn prompt_interrupt_child() {
    let Some(kind) = std::env::var_os("SHIPSLIP_PROMPT_INTERRUPT_CHILD") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        if kind == "choice" || kind == "boost" {
            let args = if kind == "boost" {
                vec![
                    "app",
                    "--starter-kit=none",
                    "--database=sqlite",
                    "--testing=pest",
                ]
            } else {
                vec!["app"]
            };
            let args = args.into_iter().map(str::to_string).collect::<Vec<_>>();
            new::run(new::Args::parse(&args).unwrap()).await.unwrap();
        } else {
            let mut interrupts = Interrupts::listen();
            new::yes_no("Publish reviewed project?", false, &mut interrupts)
                .await
                .unwrap();
        }
        panic!("Ctrl-C should exit immediately, never return into runtime shutdown");
    });
}

#[test]
fn ctrl_c_exits_blocked_choice_and_publication_confirmation_prompts() {
    use std::io::Read;
    for (kind, prompt) in [
        ("choice", "Starter kit"),
        ("boost", "Install Laravel Boost? (recommended) [Y/n]"),
        ("confirmation", "Publish reviewed project?"),
    ] {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: openpty initializes both live descriptor outputs; optional arguments are null.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        // SAFETY: each descriptor from successful openpty is converted to exactly one owner.
        let _master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_tests::prompt_interrupt_child",
                "--nocapture",
            ])
            .env("SHIPSLIP_PROMPT_INTERRUPT_CHILD", kind)
            .env_remove("SHIPSLIP_CONFIG")
            .stdin(Stdio::from(slave))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (sender, receiver) = sync_mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut buffer = [0; 512];
            while let Ok(count) = stdout.read(&mut buffer) {
                if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains(prompt) {
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                panic!("prompt did not start: {}", String::from_utf8_lossy(&output));
            }
            if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(100)) {
                output.extend(bytes);
            }
        }
        // Allow the spawned signal listener to register before delivering SIGINT.
        std::thread::sleep(Duration::from_millis(50));
        // SAFETY: this PID is the live child owned by this test; no other process is targeted.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Ctrl-C prompt shutdown hung");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        reader.join().unwrap();
        assert_eq!(
            status.code(),
            Some(i32::from(INTERRUPTED)),
            "{kind}: {status}"
        );
    }
}

#[test]
fn eof_after_side_effect_prompts_does_not_claim_nothing_was_written() {
    for question in [
        "Create this initial commit?",
        "Configure a server now?",
        "Commit and publish only this deployment config?",
        "Create/resume this repository and publish the reviewed commit?",
    ] {
        let error =
            ask_yes_no(&mut io::Cursor::new(b""), &mut Vec::new(), question, false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        let message = error.to_string();
        assert!(message.contains("question was answered"));
        assert!(!message.contains("nothing was written"));
        assert!(!message.contains("init finished"));
    }
}

#[test]
fn eof_after_creation_retains_project_and_reports_recovery() {
    if let Some(output) = isolated_output(
        "eof_after_creation_retains_project_and_reports_recovery",
        None,
    ) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Local project retained at"), "{stderr}");
        assert!(stderr.contains("git status"), "{stderr}");
        assert!(stderr.contains("slip publish github"), "{stderr}");
        assert!(stderr.contains("slip init"), "{stderr}");
        assert!(
            stderr.contains("input ended before the question was answered"),
            "{stderr}"
        );
        assert!(!stderr.contains("nothing was written"), "{stderr}");
        return;
    }
    use shipslip::create::{self, CreateRequest, Tools};
    use shipslip::git::Git;
    use shipslip::laravel::{Auth, Database, InstallerOptions, StarterKit, Testing};
    use std::os::unix::fs::PermissionsExt;

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp =
        Temp(std::env::temp_dir().join(format!("shipslip-eof-{}-{suffix}", std::process::id())));
    let start = temp.0.join("sandbox");
    let bin = temp.0.join("bin");
    fs::create_dir_all(&start).unwrap();
    fs::create_dir(&bin).unwrap();
    let script = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let php = script("php", "echo 'PHP 8.5.0'");
    let composer = script("composer", "echo 'Composer 2.9.0'");
    let node = script("node", "echo 'v26.0.0'");
    let npm = script("npm", "echo 'npm 12.0.0'");
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let actual_git = create::resolve_tool("git").unwrap();
    let git = script("git", &format!(
        "export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1\nexec {} -c user.name=EOFTester -c user.email=eof@example.com \"$@\"",
        quote(&actual_git.to_string_lossy())
    ));
    let help = quote(include_str!(
        "../../fixtures/laravel-installer-5.31.1-help.txt"
    ));
    let laravel = script(
        "laravel",
        &format!(
            r#"
if [ "$1" = --version ]; then echo 'Laravel Installer 5.31.1'; exit 0; fi
if [ "$2" = --help ]; then printf '%s\n' {help}; exit 0; fi
mkdir -p "$2/vendor" "$2/public/build/assets"
printf artisan > "$2/artisan"
printf autoload > "$2/vendor/autoload.php"
printf '%s' '{{"name":"laravel/laravel"}}' > "$2/composer.json"
printf '%s' '{{"packages":[{{"name":"laravel/framework","version":"v13.34.0"}}],"packages-dev":[{{"name":"pestphp/pest","version":"v5.0.0"}}]}}' > "$2/composer.lock"
printf '%s' '{{"scripts":{{"build":"vite build"}}}}' > "$2/package.json"
printf '{{}}' > "$2/package-lock.json"
printf '%s' '{{"resources/js/app.js":{{"file":"assets/app.js"}}}}' > "$2/public/build/manifest.json"
printf built > "$2/public/build/assets/app.js"
printf secret > "$2/.env"
"#
        ),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let preview = create::preview(
            CreateRequest {
                name: "app".into(),
                options: InstallerOptions {
                    starter_kit: StarterKit::None,
                    auth: Auth::None,
                    database: Database::Sqlite,
                    testing: Testing::Pest,
                    boost: false,
                },
                branch: "main".into(),
            },
            &start,
            Tools {
                php,
                composer,
                laravel,
                git: git.clone(),
                node,
                npm,
            },
            temp.0.join("operations"),
        )
        .await
        .unwrap();
        let confirmation = preview.confirm();
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let project = create::execute_create(preview, confirmation, receiver, |_| {})
            .await
            .unwrap();
        let root = project.root.clone();
        let mut interrupts = Interrupts::from_channel(tokio::sync::mpsc::unbounded_channel().1);
        let error = new::finish_created_project(project, "main", &mut interrupts)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("question was answered"));
        eprintln!("{error}");
        assert!(root.join("artisan").is_file());
        assert!(root.join(".git").is_dir());
        let git = Git::new(git);
        assert!(git
            .staged_paths(&root)
            .unwrap()
            .iter()
            .any(|path| path == "artisan"));
        assert!(!git
            .staged_paths(&root)
            .unwrap()
            .iter()
            .any(|path| path == ".env"));
        assert!(git.head(&root).is_err());
    });
}
