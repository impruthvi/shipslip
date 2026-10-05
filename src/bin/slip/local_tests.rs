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
fn in_flow_terminal_child() {
    let Ok(kind) = std::env::var("SHIPSLIP_IN_FLOW_TERMINAL_CHILD") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let code = runtime.block_on(async {
        if kind == "publish" {
            publish::run(publish::Args::default()).await.unwrap()
        } else {
            let args = [
                "shop",
                "--starter-kit=none",
                "--auth=none",
                "--database=sqlite",
                "--testing=pest",
                "--branch=feature/shop",
                "--no-boost",
            ]
            .map(String::from);
            new::run(new::Args::parse(&args).unwrap()).await.unwrap()
        }
    });
    assert_eq!(code, ExitCode::from(3));
    std::process::exit(3);
}

#[test]
fn interactive_new_and_publish_exit_three_with_rerun_hints_and_no_install() {
    use std::io::Write;
    for kind in ["new", "publish"] {
        let fixture = super::setup_fixture::Fixture::new();
        fixture.complete();
        for name in ["node", "npm", "gh"] {
            fs::remove_file(fixture.root.join("brew/bin").join(name)).unwrap();
        }
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: openpty initializes both descriptor outputs; optional arguments are null.
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
        // SAFETY: successful openpty returned two distinct, owned descriptors.
        let mut master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_tests::in_flow_terminal_child",
                "--nocapture",
            ])
            .env("SHIPSLIP_IN_FLOW_TERMINAL_CHILD", kind)
            .env("HOME", fixture.root.join("home"))
            .env("COMPOSER_HOME", fixture.root.join("home/.composer"))
            .env("XDG_STATE_HOME", fixture.root.join("state"))
            .env("PATH", fixture.root.join("brew/bin"))
            .env_remove("SHIPSLIP_CONFIG")
            .current_dir(&fixture.root)
            .stdin(Stdio::from(slave))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        master.write_all(b"n\n").unwrap();
        let pid = child.id();
        let (sender, receiver) = sync_mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _ = sender.send(child.wait_with_output());
        });
        let output = match receiver.recv_timeout(Duration::from_secs(20)) {
            Ok(output) => output.unwrap(),
            Err(error) => {
                // SAFETY: this is the test's child, still owned by the waiter thread.
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }
                waiter.join().unwrap();
                panic!("terminal flow hung: {error}");
            }
        };
        waiter.join().unwrap();
        assert_eq!(
            output.status.code(),
            Some(3),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(if kind == "new" {
            "Missing for project creation"
        } else {
            "Missing for GitHub publishing"
        }));
        assert!(text.contains(if kind == "new" { "slip new 'shop' --starter-kit none --auth none --database sqlite --testing pest --branch 'feature/shop' --no-boost" } else { "slip publish github" }));
        assert!(!fixture.root.join("actions").exists());
        assert!(!fixture.root.join("shop").exists());
    }
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
        vec!["--config", "/missing-config.toml", "setup"],
        vec!["setup", "--config", "/missing-config.toml"],
        vec!["setup", "--config=/missing-config.toml"],
        vec!["--config", "/missing-config.toml", "doctor"],
        vec!["doctor", "--config", "/missing-config.toml"],
        vec!["doctor", "--config=/missing-config.toml"],
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
    for values in [
        vec!["new", "app"],
        vec!["publish", "github"],
        vec!["doctor"],
        vec!["setup"],
    ] {
        let error = parse(&values).err().unwrap().to_string();
        assert!(error.contains("unset SHIPSLIP_CONFIG"), "{error}");
    }
}

#[tokio::test]
async fn non_terminal_setup_refuses_before_detection() {
    if isolated("non_terminal_setup_refuses_before_detection", None) {
        return;
    }
    std::env::set_var("HOME", "/missing-setup-home");
    std::env::set_var("PATH", "/missing-setup-path");
    let result = setup::run_setup(setup::RepairArgs::default()).await;
    let error = result.err().unwrap();
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(error.to_string().contains("interactive terminal"));
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

struct DoctorFixture {
    root: PathBuf,
    context: shipslip::setup::DetectionContext,
}
impl DoctorFixture {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "shipslip-doctor-tests-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let mut context = shipslip::setup::DetectionContext::from_environment();
        context.environment = BTreeMap::from([
            ("HOME".into(), root.join("home").into_os_string()),
            ("PATH".into(), root.join("bin").into_os_string()),
            (
                "COMPOSER_HOME".into(),
                root.join("composer").into_os_string(),
            ),
        ]);
        context.macos = false;
        context.system_bin = root.join("system");
        context.xdg_system = root.join("xdg-system");
        context.herd_app = root.join("Herd.app");
        context.brew_candidates = vec![];
        fs::create_dir(root.join("home")).unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        fs::create_dir(root.join("composer")).unwrap();
        let fixture = Self { root, context };
        fixture.script(
            &fixture.root.join("bin/php"),
            &format!(
                "if [ \"$1\" = -m ]; then printf '%s\\n' {}; else echo 'PHP 1.0.0'; fi",
                Self::quote(&shipslip::setup::PHP_EXTENSIONS.join("\n"))
            ),
        );
        fixture.script(&fixture.root.join("bin/laravel"), &format!("if [ \"$1\" = --version ]; then echo 'Laravel Installer 5.31.1'; else printf '%s\\n' {}; fi", Self::quote(include_str!("../../fixtures/laravel-installer-5.31.1-help.txt"))));
        for tool in ["composer", "node", "npm", "gh"] {
            fixture.script(&fixture.root.join("bin").join(tool), "echo tool-test");
        }
        fixture.script(&fixture.root.join("bin/git"), "echo git-ready");
        fixture
    }
    fn quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
    fn script(&self, path: &Path, body: &str) {
        use std::io::Write;
        use std::process::{Command, Stdio};
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Keep writable script descriptors out of the parallel test runner.
        let mut writer = Command::new("/bin/sh")
            .args([
                "-c",
                "umask 077; /bin/cat > \"$1\" && /bin/chmod 700 \"$1\"",
                "shipslip-fixture-writer",
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(format!("#!/bin/sh\nset -eu\n{body}\n").as_bytes())
            .unwrap();
        let output = writer.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    async fn report(&self) -> shipslip::setup::DetectionReport {
        shipslip::setup::detect(
            &self.context,
            &shipslip::setup::DetectionOptions {
                purposes: vec![shipslip::setup::Purpose::Create],
                root: self.root.clone(),
                installer_options: None,
            },
        )
        .await
        .unwrap()
    }
    async fn run(&self, purpose: shipslip::setup::Purpose) -> Result<ExitCode, Box<dyn Error>> {
        setup::run_with_context(
            setup::Args {
                purpose: Some(purpose),
                ..setup::Args::default()
            },
            &self.context,
            self.root.clone(),
        )
        .await
    }
    async fn run_json(
        &self,
        purpose: shipslip::setup::Purpose,
    ) -> Result<ExitCode, Box<dyn Error>> {
        setup::run_with_context(
            setup::Args {
                purpose: Some(purpose),
                json: true,
            },
            &self.context,
            self.root.clone(),
        )
        .await
    }
}
impl Drop for DoctorFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn doctor_dispatches_without_project_or_terminal() {
    if isolated("doctor_dispatches_without_project_or_terminal", None) {
        return;
    }
    assert!(!io::stdin().is_terminal());
    let command = parse(&["doctor", "--for", "create"]).unwrap().unwrap();
    assert!(command.config.is_none());
    let Action::Doctor(args) = command.action else {
        panic!("wrong dispatch");
    };
    let fixture = DoctorFixture::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(setup::run_with_context(
                args,
                &fixture.context,
                fixture.root.clone()
            ))
            .unwrap(),
        ExitCode::SUCCESS
    );
    assert!(!fixture.root.join(".shipslip.toml").exists());
}

#[test]
fn doctor_for_filters_requirements() {
    if isolated("doctor_for_filters_requirements", None) {
        return;
    }
    use shipslip::setup::Purpose;
    for (values, expected) in [
        (vec!["doctor"], None),
        (vec!["doctor", "--for=create"], Some(Purpose::Create)),
        (vec!["doctor", "--for", "publish"], Some(Purpose::Publish)),
    ] {
        let Action::Doctor(args) = parse(&values).unwrap().unwrap().action else {
            panic!("wrong dispatch");
        };
        assert_eq!(args.purpose, expected);
    }
    let fixture = DoctorFixture::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for purpose in [Purpose::Create, Purpose::Publish] {
            let report = shipslip::setup::detect(
                &fixture.context,
                &shipslip::setup::DetectionOptions {
                    purposes: vec![purpose],
                    root: fixture.root.clone(),
                    installer_options: None,
                },
            )
            .await
            .unwrap();
            assert!(report
                .findings
                .iter()
                .all(|finding| finding.requirement.purposes.contains(&purpose)));
            if purpose == Purpose::Publish {
                assert_eq!(report.findings.len(), 3);
            } else {
                assert!(!report
                    .findings
                    .iter()
                    .any(|finding| finding.requirement.id
                        == shipslip::setup::RequirementId::GithubAuth));
            }
        }
    });
}

#[test]
fn doctor_rejects_invalid_options() {
    if isolated("doctor_rejects_invalid_options", None) {
        return;
    }
    for values in [
        vec!["doctor", "--for"],
        vec!["doctor", "--for="],
        vec!["doctor", "--for=deploy"],
        vec!["doctor", "--for=unknown"],
        vec!["doctor", "--for=create", "--for=publish"],
        vec!["doctor", "--json", "--json"],
        vec!["doctor", "--json=true"],
        vec!["doctor", "--unknown"],
        vec!["doctor", "create"],
    ] {
        let error = parse(&values).err().unwrap();
        assert_eq!(
            error_exit_code(error.as_ref()),
            ExitCode::from(2),
            "{values:?}"
        );
    }
}

#[test]
fn doctor_json_parses_options() {
    for values in [
        vec!["doctor", "--json"],
        vec!["doctor", "--json", "--for", "create"],
        vec!["doctor", "--for=publish", "--json"],
    ] {
        let args: Vec<_> = values.into_iter().map(String::from).collect();
        assert!(setup::Args::parse(&args[1..]).unwrap().json);
    }
    assert!(!setup::Args::default().json);
}

#[test]
fn doctor_json_request_only_matches_doctor() {
    for (values, expected) in [
        (vec!["doctor", "--json"], true),
        (vec!["doctor", "--for", "publish", "--json"], true),
        (vec!["--config", "deploy.toml", "doctor", "--json"], true),
        (vec!["doctor", "--for", "publish"], false),
        (vec!["new", "doctor", "--json"], false),
        (vec!["--config", "doctor", "new", "--json"], false),
        (vec!["--config", "doctor"], false),
        (vec![], false),
    ] {
        let args: Vec<_> = values.into_iter().map(String::from).collect();
        assert_eq!(setup::json_requested(&args), expected, "{args:?}");
    }
}

fn json_documents(output: &std::process::Output) -> Vec<serde_json::Value> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("Project creation:"));
    assert!(!stdout.contains("GitHub publishing:"));
    assert!(!stdout.contains('\u{1b}'));
    stdout
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn doctor_json_output_matches_readiness_and_filters_purpose() {
    if let Some(output) = isolated_output(
        "doctor_json_output_matches_readiness_and_filters_purpose",
        None,
    ) {
        let documents = json_documents(&output);
        assert_eq!(documents.len(), 4);
        for (document, (code, purpose)) in
            documents
                .iter()
                .zip([(0, "create"), (0, "create"), (3, "create"), (0, "publish")])
        {
            assert_eq!(document["schema"], 1);
            assert_eq!(document["exit_code"], code);
            assert_eq!(document["ready"], code == 0);
            assert_eq!(document["purposes"], serde_json::json!([purpose]));
            assert_eq!(document["error"], serde_json::Value::Null);
            assert!(document["findings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|finding| {
                    finding["requirement"]["purposes"]
                        .as_array()
                        .unwrap()
                        .contains(&serde_json::json!(purpose))
                }));
        }
        assert!(documents[1]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["requirement"]["id"] == "laravel"
                    && finding["state"]["status"] == "off_path"
            }));
        assert_eq!(documents[3]["findings"].as_array().unwrap().len(), 3);
        return;
    }
    use shipslip::setup::Purpose;
    let fixture = DoctorFixture::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert_eq!(
            fixture.run_json(Purpose::Create).await.unwrap(),
            ExitCode::SUCCESS
        );
        let bin = fixture.root.join("composer/vendor/bin");
        fs::create_dir_all(&bin).unwrap();
        fs::rename(fixture.root.join("bin/laravel"), bin.join("laravel")).unwrap();
        assert_eq!(
            fixture.run_json(Purpose::Create).await.unwrap(),
            ExitCode::SUCCESS
        );
        fs::remove_file(fixture.root.join("bin/node")).unwrap();
        fs::remove_file(fixture.root.join("bin/npm")).unwrap();
        assert_eq!(
            fixture.run_json(Purpose::Create).await.unwrap(),
            ExitCode::from(3)
        );
        assert_eq!(
            fixture.run_json(Purpose::Publish).await.unwrap(),
            ExitCode::SUCCESS
        );
    });
}

#[test]
fn doctor_json_errors_record_exit_codes() {
    if let Some(output) = isolated_output("doctor_json_errors_record_exit_codes", None) {
        let documents = json_documents(&output);
        assert_eq!(documents.len(), 4);
        for (document, code) in documents.iter().zip([2, 2, 2, 1]) {
            assert_eq!(document["schema"], 1);
            assert_eq!(document["exit_code"], code);
            assert_eq!(document["ready"], false);
            assert!(document["findings"].as_array().unwrap().is_empty());
            assert!(document["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()));
        }
        assert!(output.stderr.is_empty());
        return;
    }
    for values in [
        vec!["doctor", "--json", "--for=deploy"],
        vec!["doctor", "--json", "--json"],
        vec!["--config", "deploy.toml", "doctor", "--json"],
    ] {
        let error = parse(&values).err().unwrap();
        assert_eq!(finish_run(Err(error), true), ExitCode::from(2));
    }
    let fixture = DoctorFixture::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(setup::run_with_context(
        setup::Args {
            json: true,
            ..setup::Args::default()
        },
        &fixture.context,
        fixture.root.join("missing"),
    ));
    assert_eq!(finish_run(result, true), ExitCode::FAILURE);
}

#[test]
fn doctor_json_preserves_auth_redaction() {
    if let Some(output) = isolated_output("doctor_json_preserves_auth_redaction", None) {
        let documents = json_documents(&output);
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0]["exit_code"], 3);
        let document = documents[0].to_string();
        assert!(!document.contains("doctor-json-secret"));
        assert!(document.contains("[REDACTED]"));
        return;
    }
    let mut fixture = DoctorFixture::new();
    fixture
        .context
        .environment
        .insert("GH_TOKEN".into(), "doctor-json-secret".into());
    fixture.script(
        &fixture.root.join("bin/gh"),
        "if [ \"$1\" = --version ]; then echo gh-test; else echo \"$GH_TOKEN\" >&2; exit 1; fi",
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(fixture.run_json(shipslip::setup::Purpose::Publish))
            .unwrap(),
        ExitCode::from(3)
    );
}

#[test]
fn doctor_off_path_only_exits_zero_with_shell_warning() {
    if let Some(output) =
        isolated_output("doctor_off_path_only_exits_zero_with_shell_warning", None)
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("export PATH='"), "{stdout}");
        assert!(stdout.contains(":\"$PATH\""), "{stdout}");
        assert!(
            stdout.contains("installed outside PATH; usable by slip"),
            "{stdout}"
        );
        return;
    }
    let fixture = DoctorFixture::new();
    let bin = fixture.root.join("composer/vendor/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::rename(fixture.root.join("bin/laravel"), bin.join("laravel")).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(fixture.run(shipslip::setup::Purpose::Create))
            .unwrap(),
        ExitCode::SUCCESS
    );
}

#[test]
fn doctor_config_warning_does_not_change_readiness() {
    if let Some(output) = isolated_output("doctor_config_warning_does_not_change_readiness", None) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("config.json") && stdout.contains("Warning:"),
            "{stdout}"
        );
        return;
    }
    let fixture = DoctorFixture::new();
    fs::write(fixture.root.join("composer/config.json"), "malformed").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(fixture.run(shipslip::setup::Purpose::Create))
            .unwrap(),
        ExitCode::SUCCESS
    );
}

#[test]
fn doctor_missing_requirements_exit_three() {
    if let Some(output) = isolated_output("doctor_missing_requirements_exit_three", None) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("node: missing") && stdout.contains("npm: missing"),
            "{stdout}"
        );
        assert!(stdout.contains("Not ready:"), "{stdout}");
        return;
    }
    let fixture = DoctorFixture::new();
    fs::remove_file(fixture.root.join("bin/node")).unwrap();
    fs::remove_file(fixture.root.join("bin/npm")).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(fixture.run(shipslip::setup::Purpose::Create))
            .unwrap(),
        ExitCode::from(3)
    );
}

#[test]
fn doctor_unverified_only_exits_zero() {
    if let Some(output) = isolated_output("doctor_unverified_only_exits_zero", None) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("warning: unverified"), "{stdout}");
        return;
    }
    let fixture = DoctorFixture::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut report = fixture.report().await;
        report.findings[0].state = shipslip::setup::FindingState::Unverified {
            reason: "probe unavailable".into(),
        };
        assert!(report.ready());
        println!(
            "{}",
            setup::render(&report, &[shipslip::setup::Purpose::Create])
        );
        assert_eq!(setup::report_exit_code(&report), ExitCode::SUCCESS);
    });
}

#[test]
fn doctor_internal_failure_exits_one() {
    let fixture = DoctorFixture::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(setup::run_with_context(
            setup::Args::default(),
            &fixture.context,
            fixture.root.join("missing"),
        ))
        .unwrap_err();
    assert_eq!(error_exit_code(error.as_ref()), ExitCode::FAILURE);
}

#[test]
fn doctor_output_escapes_external_text() {
    let fixture = DoctorFixture::new();
    fixture.script(
        &fixture.root.join("bin/node"),
        "printf '\x1b[31mfailed\x1b[0m' >&2; exit 1",
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let report = runtime.block_on(fixture.report());
    let output = setup::render(&report, &[shipslip::setup::Purpose::Create]);
    assert!(!output.contains('\x1b'));
    assert!(output.contains("failed"));
    let quoted_dir = Path::new("/some user's tools");
    assert_eq!(
        setup::shell_line(quoted_dir),
        "export PATH='/some user'\\''s tools':\"$PATH\""
    );
}

#[test]
fn fresh_process_finds_custom_composer_bin_dir() {
    if let Some(root) = std::env::var_os("SHIPSLIP_COMPOSER_FIXTURE") {
        let root = PathBuf::from(root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = runtime
            .block_on(shipslip::setup::detect(
                &shipslip::setup::DetectionContext::from_environment(),
                &shipslip::setup::DetectionOptions {
                    purposes: vec![shipslip::setup::Purpose::Create],
                    root: root.clone(),
                    installer_options: None,
                },
            ))
            .unwrap();
        assert!(report.ready());
        let laravel = report
            .findings
            .iter()
            .find(|finding| {
                finding.requirement.id == shipslip::setup::RequirementId::Tool("laravel")
            })
            .unwrap();
        assert!(
            matches!(&laravel.state, shipslip::setup::FindingState::OffPath { path, .. } if path == &root.join("composer/custom tools/laravel"))
        );
        return;
    }
    let fixture = DoctorFixture::new();
    let bin = fixture.root.join("composer/custom tools");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        fixture.root.join("composer/config.json"),
        r#"{"config":{"bin-dir":"custom tools"}}"#,
    )
    .unwrap();
    fs::rename(fixture.root.join("bin/laravel"), bin.join("laravel")).unwrap();
    fixture.script(
        &fixture.root.join("bin/composer"),
        "[ \"$*\" = --version ]; echo composer-ready",
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "local_tests::fresh_process_finds_custom_composer_bin_dir",
            "--nocapture",
        ])
        .env("SHIPSLIP_COMPOSER_FIXTURE", &fixture.root)
        .envs(&fixture.context.environment)
        .env_remove("SHIPSLIP_CONFIG")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
