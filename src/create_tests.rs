use super::*;
use crate::laravel::{Auth, Database, StarterKit, Testing};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::time::Duration;

struct Fixture {
    root: PathBuf,
    start: PathBuf,
    operations: PathBuf,
    tools: Tools,
}

const SCRUBBED: &str = r#"
[ -z "${GH_TOKEN+x}" ] && [ -z "${GITHUB_TOKEN+x}" ] &&
[ -z "${GH_ENTERPRISE_TOKEN+x}" ] && [ -z "${GITHUB_ENTERPRISE_TOKEN+x}" ] &&
[ -z "${SHIPSLIP_GITHUB_TOKEN+x}" ] || exit 91
if IFS= read -r input; then echo 'interactive stdin unexpectedly open'; exit 92; fi
"#;

const SCAFFOLD: &str = r#"
project=$2
mkdir -p "$project/vendor" "$project/public/build/assets"
printf 'artisan\n' > "$project/artisan"
printf 'autoload\n' > "$project/vendor/autoload.php"
printf '%s' '{"name":"laravel/laravel","require":{"laravel/framework":"^13.0"}}' > "$project/composer.json"
printf '%s' '{"packages":[{"name":"laravel/framework","version":"v13.34.0"}],"packages-dev":[{"name":"pestphp/pest","version":"v4.0.0"}]}' > "$project/composer.lock"
printf '%s' '{"scripts":{"build":"vite build"}}' > "$project/package.json"
printf '{}' > "$project/package-lock.json"
printf '%s' '{"resources/js/app.js":{"file":"assets/app.js"}}' > "$project/public/build/manifest.json"
printf 'built\n' > "$project/public/build/assets/app.js"
printf 'SECRET_VALUE_FROM_ENV_FILE\n' > "$project/.env"
echo 'installer stdin closed and env scrubbed'
"#;

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "shipslip-create-tests-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let start = root.join("sandbox");
        let bin = root.join("bin");
        fs::create_dir(&start).unwrap();
        fs::create_dir(&bin).unwrap();
        let fixture = Self {
            operations: root.join("operations"),
            tools: Tools {
                php: bin.join("php"),
                composer: bin.join("composer"),
                laravel: bin.join("laravel"),
                git: resolve_tool("git").unwrap(),
                node: bin.join("node"),
                npm: bin.join("npm"),
            },
            root,
            start,
        };
        for binary in [
            &fixture.tools.composer,
            &fixture.tools.node,
            &fixture.tools.npm,
        ] {
            fixture.script(binary, &format!("{SCRUBBED}\necho 'fake tool 1.0'\n"));
        }
        fixture.script(&fixture.tools.php, &format!(
            "{SCRUBBED}\nif [ \"$1\" = --version ]; then echo 'PHP 8.5.0'; exit 0; fi\n[ \"$1\" = artisan ] && [ \"$2\" = test ] || exit 93\nprintf tested > .test-ran\necho 'application tests passed'\n"
        ));
        fixture.installer(SCAFFOLD);
        fixture
    }

    fn script(&self, path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\nset -eu\n{body}")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn installer(&self, body: &str) {
        let help =
            crate::script::shell_quote(include_str!("fixtures/laravel-installer-5.31.1-help.txt"));
        self.script(&self.tools.laravel, &format!(
            "{SCRUBBED}\nif [ \"$1\" = --version ]; then echo 'Laravel Installer 5.31.1'; exit 0; fi\nif [ \"$1\" = new ] && [ \"$2\" = --help ]; then printf '%s\\n' {help}; exit 0; fi\n[ \"$1\" = new ] || exit 94\n{body}"
        ));
    }

    async fn preview(&self, name: &str) -> CreatePreview {
        preview(
            request(name),
            &self.start,
            self.tools.clone(),
            self.operations.clone(),
        )
        .await
        .unwrap()
    }

    fn journal(&self, target: &Path, op_id: &str) -> (PathBuf, CreateJournal, String) {
        let path = self
            .operations
            .join("create")
            .join(target_key(target))
            .join(format!("{op_id}.json"));
        let text = fs::read_to_string(&path).unwrap();
        let journal = serde_json::from_str(&text).unwrap();
        (path, journal, text)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.start, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn request(name: &str) -> CreateRequest {
    CreateRequest {
        name: name.into(),
        options: InstallerOptions {
            starter_kit: StarterKit::None,
            auth: Auth::None,
            database: Database::Sqlite,
            testing: Testing::Pest,
            boost: false,
        },
        branch: "main".into(),
    }
}

#[test]
fn destinations_reject_hidden_files_existing_dirs_and_invalid_names() {
    let fixture = Fixture::new();
    fs::write(fixture.start.join(".hidden"), "existing").unwrap();
    assert!(Destination::check(&fixture.start, ".")
        .unwrap_err()
        .to_string()
        .contains("hidden"));
    fs::remove_file(fixture.start.join(".hidden")).unwrap();
    fs::create_dir(fixture.start.join("existing")).unwrap();
    assert!(Destination::check(&fixture.start, "existing").is_err());
    for name in ["", "..", "a/b", "/tmp/app"] {
        assert!(Destination::check(&fixture.start, name).is_err(), "{name}");
    }
}

#[test]
fn destinations_reject_live_and_broken_symlinks() {
    let fixture = Fixture::new();
    for target in [&fixture.start, &fixture.root.join("missing")] {
        let link = fixture.root.join("link");
        symlink(target, &link).unwrap();
        assert!(Destination::check(&fixture.root, "link")
            .unwrap_err()
            .to_string()
            .contains("symlink"));
        assert!(Destination::check(&link, ".").is_err());
        fs::remove_file(link).unwrap();
    }
}

#[test]
fn symlinked_parent_is_resolved_and_spaces_are_preserved() {
    let fixture = Fixture::new();
    let link = fixture.root.join("parent alias");
    symlink(&fixture.start, &link).unwrap();
    let destination = Destination::check(&link, "my app").unwrap();
    assert_eq!(destination.path, fixture.start.join("my app"));
    assert_eq!(destination.parent, fixture.start);
}

#[test]
fn enclosing_git_directory_or_worktree_file_is_rejected() {
    for as_directory in [true, false] {
        let fixture = Fixture::new();
        let git = fixture.root.join(".git");
        if as_directory {
            fs::create_dir(&git).unwrap();
        } else {
            fs::write(&git, "gitdir: /tmp/worktree").unwrap();
        }
        assert!(Destination::check(&fixture.start, "app").is_err());
        assert!(Destination::check(&fixture.start, ".").is_err());
    }
}

#[test]
fn read_only_parent_is_rejected_before_writing() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.start, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(Destination::check(&fixture.start, "app")
        .unwrap_err()
        .to_string()
        .contains("writable"));
    assert!(Destination::check(&fixture.start, ".").is_err());
    fs::set_permissions(&fixture.start, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(fs::read_dir(&fixture.start).unwrap().next().is_none());
}

#[test]
fn move_refuses_a_destination_created_after_preview_even_if_empty() {
    let fixture = Fixture::new();
    let destination = Destination::check(&fixture.start, "app").unwrap();
    let staging = fixture.root.join("staging");
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("new"), "new").unwrap();
    fs::create_dir(&destination.path).unwrap();
    let inode = fs::metadata(&destination.path).unwrap().ino();
    assert!(move_into_place(&destination, &staging).is_err());
    assert_eq!(fs::metadata(&destination.path).unwrap().ino(), inode);
    assert!(staging.join("new").is_file());
    assert!(fs::read_dir(&destination.path).unwrap().next().is_none());
}

#[test]
fn exclusive_rename_preserves_existing_files_and_empty_directories() {
    let fixture = Fixture::new();
    for directory in [false, true] {
        let source = fixture.root.join("source");
        let destination = fixture.root.join("existing");
        if directory {
            fs::create_dir(&source).unwrap();
            fs::create_dir(&destination).unwrap();
        } else {
            fs::write(&source, "new").unwrap();
            fs::write(&destination, "old").unwrap();
        }
        let inode = fs::metadata(&destination).unwrap().ino();
        assert!(rename_exclusive(&source, &destination).is_err());
        assert_eq!(fs::metadata(&destination).unwrap().ino(), inode);
        if directory {
            fs::remove_dir(&source).unwrap();
            fs::remove_dir(&destination).unwrap();
        } else {
            assert_eq!(fs::read_to_string(&destination).unwrap(), "old");
            fs::remove_file(&source).unwrap();
            fs::remove_file(&destination).unwrap();
        }
    }
}

#[test]
fn dot_move_preserves_cwd_inode_and_hidden_entries() {
    let fixture = Fixture::new();
    let destination = Destination::check(&fixture.start, ".").unwrap();
    let inode = fs::metadata(&fixture.start).unwrap().ino();
    let staging = fixture.root.join("staging");
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join(".env.example"), "example").unwrap();
    fs::write(staging.join("artisan"), "artisan").unwrap();
    move_into_place(&destination, &staging).unwrap();
    assert_eq!(fs::metadata(&fixture.start).unwrap().ino(), inode);
    assert!(fixture.start.join(".env.example").is_file());
    assert!(fixture.start.join("artisan").is_file());
    assert!(!staging.exists());
}

#[test]
fn dot_move_race_preserves_existing_content() {
    let fixture = Fixture::new();
    let destination = Destination::check(&fixture.start, ".").unwrap();
    let staging = fixture.root.join("staging");
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("artisan"), "new").unwrap();
    fs::write(fixture.start.join("artisan"), "user content").unwrap();
    assert!(move_into_place(&destination, &staging).is_err());
    assert_eq!(
        fs::read_to_string(fixture.start.join("artisan")).unwrap(),
        "user content"
    );
    assert!(staging.join("artisan").is_file());
}

#[test]
fn locks_exclude_same_target_allow_other_targets_and_are_stable() {
    let fixture = Fixture::new();
    let target = fixture.start.join("app");
    let first = OperationLock::acquire(&fixture.operations, &target).unwrap();
    assert!(matches!(
        OperationLock::acquire(&fixture.operations, &target),
        Err(CreateError::Busy(_))
    ));
    let second = OperationLock::acquire(&fixture.operations, &fixture.start.join("other")).unwrap();
    let path = fixture
        .operations
        .join("locks")
        .join(format!("{}.lock", target_key(&target)));
    let inode = fs::metadata(&path).unwrap().ino();
    drop(first);
    let _replacement = OperationLock::acquire(&fixture.operations, &target).unwrap();
    assert_eq!(fs::metadata(path).unwrap().ino(), inode);
    drop(second);
}

#[test]
fn cleanup_requires_matching_regular_marker_and_does_not_follow_symlinks() {
    let fixture = Fixture::new();
    let unrelated = fixture.root.join("unrelated");
    fs::create_dir(&unrelated).unwrap();
    fs::write(unrelated.join("keep"), "user content").unwrap();
    assert!(remove_owned_staging(&unrelated).is_err());
    let owned = fixture.root.join(".slip-new-operation");
    fs::create_dir(&owned).unwrap();
    fs::write(owned.join(".slip-operation"), "wrong").unwrap();
    assert!(remove_owned_staging(&owned).is_err());
    fs::remove_file(owned.join(".slip-operation")).unwrap();
    symlink(unrelated.join("keep"), owned.join(".slip-operation")).unwrap();
    assert!(remove_owned_staging(&owned).is_err());
    fs::remove_file(owned.join(".slip-operation")).unwrap();
    fs::write(owned.join(".slip-operation"), "operation").unwrap();
    symlink(&unrelated, owned.join("external")).unwrap();
    remove_owned_staging(&owned).unwrap();
    assert_eq!(
        fs::read_to_string(unrelated.join("keep")).unwrap(),
        "user content"
    );
    let link = fixture.root.join(".slip-new-link");
    symlink(&unrelated, &link).unwrap();
    assert!(remove_owned_staging(&link).is_err());
    assert!(unrelated.exists());
}

#[test]
fn fake_installer_success_scrubs_env_and_closes_stdin() {
    const CHILD: &str = "SHIPSLIP_CREATE_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "create::tests::fake_installer_success_scrubs_env_and_closes_stdin",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("GH_TOKEN", "TOKEN_MUST_NOT_REACH_INSTALLER")
            .env("GITHUB_TOKEN", "TOKEN_MUST_NOT_REACH_INSTALLER")
            .env("GH_ENTERPRISE_TOKEN", "TOKEN_MUST_NOT_REACH_INSTALLER")
            .env("GITHUB_ENTERPRISE_TOKEN", "TOKEN_MUST_NOT_REACH_INSTALLER")
            .env("SHIPSLIP_GITHUB_TOKEN", "TOKEN_MUST_NOT_REACH_INSTALLER")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("TOKEN_MUST_NOT_REACH_INSTALLER"));
        return;
    }
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let fixture = Fixture::new();
        for name in ["my app", "."] {
            let preview = fixture.preview(name).await;
            let target = preview.destination.path.clone();
            let op_id = preview.op_id.clone();
            let staging = preview.staging.clone();
            let confirmation = preview.confirm();
            let (_sender, receiver) = watch::channel(false);
            let mut events = Vec::new();
            let mut project = execute_create(preview, confirmation, receiver, |event| events.push(event)).await.unwrap();
            assert!(project.root.join(".test-ran").is_file());
            assert_eq!(project.framework_version, "v13.34.0");
            assert!(!project.root.join(".git").exists());
            assert!(!staging.exists());
            assert!(events.iter().any(|event| matches!(event, CreateEvent::Output(line) if line.contains("env scrubbed"))));
            assert!(events.iter().any(|event| matches!(event, CreateEvent::Verified(_))));
            assert!(matches!(OperationLock::acquire(&fixture.operations, &target), Err(CreateError::Busy(_))));
            let (path, journal, text) = fixture.journal(&target, &op_id);
            assert_eq!(journal.state, CreateState::Moved);
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
            assert!(!text.contains("TOKEN_MUST_NOT_REACH_INSTALLER"));
            assert!(!text.contains("SECRET_VALUE_FROM_ENV_FILE"));
            project.mark_committed("a".repeat(40)).unwrap();
            assert_eq!(fixture.journal(&target, &op_id).1.state, CreateState::Committed);
            drop(project);
            let _lock = OperationLock::acquire(&fixture.operations, &target).unwrap();
            fs::remove_dir_all(&target).unwrap();
            if name == "." { fs::create_dir(&fixture.start).unwrap(); }
        }
    });
}

#[tokio::test]
async fn installer_failure_keeps_owned_staging_and_marks_failed() {
    let fixture = Fixture::new();
    fixture.installer(
        "mkdir -p \"$2\"\nprintf partial > \"$2/partial\"\necho 'network failure' >&2\nexit 1\n",
    );
    let preview = fixture.preview("app").await;
    let staging = preview.staging.clone();
    let target = preview.destination.path.clone();
    let op_id = preview.op_id.clone();
    let confirmation = preview.confirm();
    let (_sender, receiver) = watch::channel(false);
    let error = execute_create(preview, confirmation, receiver, |_| {})
        .await
        .unwrap_err();
    assert!(matches!(error, CreateError::ScaffoldFailed { .. }));
    assert!(!target.exists());
    assert!(staging.join("project/partial").is_file());
    assert_eq!(
        fixture.journal(&target, &op_id).1.state,
        CreateState::ScaffoldFailed
    );
    remove_owned_staging(&staging).unwrap();
    assert!(!staging.exists());
}

#[tokio::test]
async fn installer_git_directory_is_rejected_before_move() {
    let fixture = Fixture::new();
    fixture.installer(&format!("{SCAFFOLD}\nmkdir \"$2/.git\"\n"));
    let preview = fixture.preview("app").await;
    let target = preview.destination.path.clone();
    let op_id = preview.op_id.clone();
    let confirmation = preview.confirm();
    let (_sender, receiver) = watch::channel(false);
    let error = execute_create(preview, confirmation, receiver, |_| {})
        .await
        .unwrap_err();
    assert!(error.to_string().contains("installer created .git"));
    assert!(!target.exists());
    assert_eq!(
        fixture.journal(&target, &op_id).1.state,
        CreateState::ScaffoldFailed
    );
}

#[tokio::test]
async fn installer_symlink_root_is_rejected() {
    let fixture = Fixture::new();
    let external = fixture.root.join("external");
    fs::create_dir(&external).unwrap();
    fixture.installer(&format!(
        "ln -s {} \"$2\"\n",
        crate::script::shell_quote(&external.to_string_lossy())
    ));
    let preview = fixture.preview("app").await;
    let confirmation = preview.confirm();
    let (_sender, receiver) = watch::channel(false);
    let error = execute_create(preview, confirmation, receiver, |_| {})
        .await
        .unwrap_err();
    assert!(error.to_string().contains("symlink"));
    assert!(external.exists());
}

#[tokio::test]
async fn application_test_failure_keeps_scaffold() {
    let fixture = Fixture::new();
    fixture.script(&fixture.tools.php, "if [ \"$1\" = --version ]; then echo 'PHP 8.5.0'; exit 0; fi\necho 'application test failed'\nexit 1\n");
    let preview = fixture.preview("app").await;
    let staging = preview.staging.clone();
    let target = preview.destination.path.clone();
    let op_id = preview.op_id.clone();
    let confirmation = preview.confirm();
    let (_sender, receiver) = watch::channel(false);
    assert!(execute_create(preview, confirmation, receiver, |_| {})
        .await
        .is_err());
    assert!(staging.join("project/artisan").is_file());
    assert_eq!(
        fixture.journal(&target, &op_id).1.state,
        CreateState::ScaffoldFailed
    );
}

#[tokio::test]
async fn creation_detects_a_move_race_without_overwriting() {
    let fixture = Fixture::new();
    let preview = fixture.preview("app").await;
    let target = preview.destination.path.clone();
    let staging = preview.staging.clone();
    let op_id = preview.op_id.clone();
    let confirmation = preview.confirm();
    let (_sender, receiver) = watch::channel(false);
    let error = execute_create(preview, confirmation, receiver, |event| {
        if matches!(event, CreateEvent::Verified(_)) {
            fs::create_dir(&target).unwrap();
            fs::write(target.join("keep"), "user content").unwrap();
        }
    })
    .await
    .unwrap_err();
    assert!(matches!(error, CreateError::ScaffoldFailed { .. }));
    assert_eq!(
        fs::read_to_string(target.join("keep")).unwrap(),
        "user content"
    );
    assert!(staging.join("project/artisan").is_file());
    assert_eq!(
        fixture.journal(&target, &op_id).1.state,
        CreateState::Verified
    );
}

#[tokio::test]
async fn cancellation_kills_descendant_heartbeat_and_marks_failed() {
    let fixture = Fixture::new();
    fixture.installer(
        r#"
mkdir -p "$2"
(while :; do printf '.' >> "$2/heartbeat"; sleep 0.03; done) &
printf '%s\n' "$!" > "$2/descendant.pid"
echo 'heartbeat running'
wait
"#,
    );
    let preview = fixture.preview("app").await;
    let staging = preview.staging.clone();
    let target = preview.destination.path.clone();
    let op_id = preview.op_id.clone();
    let confirmation = preview.confirm();
    let (sender, receiver) = watch::channel(false);
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        execute_create(preview, confirmation, receiver, |event| {
            if matches!(event, CreateEvent::Output(line) if line == "heartbeat running") {
                sender.send(true).unwrap();
            }
        }),
    )
    .await
    .expect("cancelled installer must finish promptly")
    .unwrap_err();
    assert!(matches!(error, CreateError::Cancelled(_)));
    assert_eq!(
        fixture.journal(&target, &op_id).1.state,
        CreateState::ScaffoldFailed
    );
    assert!(!target.exists());
    let heartbeat = staging.join("project/heartbeat");
    let first = fs::read(&heartbeat).unwrap_or_default();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        fs::read(&heartbeat).unwrap_or_default(),
        first,
        "descendant survived process-group cancellation"
    );
    let pid: i32 = fs::read_to_string(staging.join("project/descendant.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // A killed child can briefly remain a zombie; kill(pid, 0) is not proof of life.
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    assert!(
        state.trim().is_empty() || state.trim().starts_with('Z'),
        "descendant still live: {state}"
    );
}

#[tokio::test]
async fn confirmation_mutation_is_rejected_without_any_writes() {
    let fixture = Fixture::new();
    let mut preview = fixture.preview("app").await;
    let confirmation = preview.confirm();
    let staging = preview.staging.clone();
    preview.request.branch = "different".into();
    let (_sender, receiver) = watch::channel(false);
    assert!(execute_create(preview, confirmation, receiver, |_| {})
        .await
        .unwrap_err()
        .to_string()
        .contains("confirmation"));
    assert!(!fixture.operations.exists());
    assert!(!staging.exists());
}

#[tokio::test]
async fn invalid_git_branch_is_rejected_in_preview() {
    let fixture = Fixture::new();
    for branch in ["/", "foo/", "foo/.bar", "foo.lock"] {
        let mut request = request("app");
        request.branch = branch.into();
        assert!(
            preview(
                request,
                &fixture.start,
                fixture.tools.clone(),
                fixture.operations.clone()
            )
            .await
            .is_err(),
            "{branch}"
        );
    }
    assert!(!fixture.operations.exists());
    assert!(fs::read_dir(&fixture.start).unwrap().next().is_none());
}

#[test]
fn concurrent_exclusive_renames_have_exactly_one_winner() {
    let fixture = Fixture::new();
    let destination = fixture.root.join("destination");
    let sources = [fixture.root.join("first"), fixture.root.join("second")];
    for (index, source) in sources.iter().enumerate() {
        fs::create_dir(source).unwrap();
        fs::write(source.join("owner"), index.to_string()).unwrap();
    }
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = sources
        .iter()
        .cloned()
        .map(|source| {
            let barrier = barrier.clone();
            let destination = destination.clone();
            std::thread::spawn(move || {
                barrier.wait();
                rename_exclusive(&source, &destination).is_ok()
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|success| **success).count(), 1);
    let winner = results.iter().position(|success| *success).unwrap();
    assert_eq!(
        fs::read_to_string(destination.join("owner")).unwrap(),
        winner.to_string()
    );
    assert!(!sources[winner].exists());
    assert!(sources[1 - winner].join("owner").is_file());
}

#[test]
fn symlink_parent_aliases_share_the_same_target_lock() {
    let fixture = Fixture::new();
    let alias = fixture.root.join("alias");
    symlink(&fixture.start, &alias).unwrap();
    let direct = Destination::check(&fixture.start, "app").unwrap();
    let aliased = Destination::check(&alias, "app").unwrap();
    let _lock = OperationLock::acquire(&fixture.operations, &direct.path).unwrap();
    assert!(matches!(
        OperationLock::acquire(&fixture.operations, &aliased.path),
        Err(CreateError::Busy(_))
    ));
}

#[test]
fn installer_output_removes_terminal_controls_and_repeated_spinner_frames() {
    let mut output = ProcessOutput::default();
    assert_eq!(output.line("\x1b[?25l"), None);
    assert_eq!(
        output.line(" \x1b[36m⠶\x1b[39m Installing dependencies with npm..."),
        Some("Installing dependencies with npm...".into())
    );
    assert_eq!(output.line("\x1b[1G\x1b[2A\x1b[J"), None);
    assert_eq!(output.line("  ⠂ Installing dependencies with npm..."), None);
    assert_eq!(output.line("  ⠒ Installing dependencies with npm..."), None);
    assert_eq!(
        output.line("\x1b[32m✔\x1b[39m Installing dependencies with npm..."),
        Some("✔ Installing dependencies with npm...".into())
    );
    assert_eq!(
        output.line("⠶ Building assets..."),
        Some("Building assets...".into())
    );
    assert_eq!(
        output.line("\x1b]8;;https://example.invalid\x1b\\error details\x1b]8;;\x1b\\"),
        Some("error details".into())
    );
    for _ in 0..2 {
        assert_eq!(
            output.line("  npm ERR! download failed"),
            Some("  npm ERR! download failed".into())
        );
    }
    // Unrecognized control bytes still reach the CLI's safe escaping, never terminal execution.
    assert_eq!(
        crate::logs::escape(&output.line("error\x1bX\x07").unwrap()),
        "error\\x1bX\\x07"
    );
}

#[tokio::test]
async fn process_output_keeps_errors_and_exit_failure_while_cleaning_animations() {
    let fixture = Fixture::new();
    fixture.script(&fixture.tools.npm, r#"
[ "$NO_COLOR" = 1 ] && [ "$FORCE_COLOR" = 0 ] && [ "$TERM" = dumb ] || exit 95
printf '\033[?25l\n \033[36m⠶\033[39m Installing dependencies with npm...\n'
printf '\033[1G\033[2A\033[J\n ⠂ Installing dependencies with npm...\r ⠒ Installing dependencies with npm...\n'
printf '\033[32m✔\033[39m Installing dependencies with npm...\n'
printf 'npm WARN deprecated package\nnpm WARN deprecated package\n'
printf '\033[31mnpm ERR! network failed\033[0m\n' >&2
exit 1
"#);
    let (_cancel, mut receiver) = watch::channel(false);
    let mut lines = Vec::new();
    let result = run_process(
        &fixture.tools,
        &fixture.tools.npm,
        &[],
        &fixture.start,
        "npm",
        &mut receiver,
        &mut |event| {
            if let CreateEvent::Output(line) = event {
                lines.push(line);
            }
        },
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("exited unsuccessfully"));
    assert_eq!(
        lines
            .iter()
            .filter(|line| *line == "Installing dependencies with npm...")
            .count(),
        1
    );
    assert!(lines
        .iter()
        .any(|line| line == "✔ Installing dependencies with npm..."));
    assert_eq!(
        lines
            .iter()
            .filter(|line| *line == "npm WARN deprecated package")
            .count(),
        2
    );
    assert!(lines.iter().any(|line| line == "npm ERR! network failed"));
    assert!(lines
        .iter()
        .all(|line| !line.contains('\x1b') && !line.contains('\r')));
}

#[tokio::test]
async fn reviewed_tool_bindings_win_over_conflicting_interpreters_and_helpers() {
    let mut fixture = Fixture::new();
    let node_dir = fixture.root.join("reviewed node");
    fs::create_dir(&node_dir).unwrap();
    let node = node_dir.join("node");
    fixture.script(&node, "echo 'v26.reviewed'");
    fixture.tools.node = node;
    fixture.script(
        &fixture.tools.php.parent().unwrap().join("node"),
        "echo 'v18.shadow'",
    );
    let composer_dir = fixture.root.join("reviewed composer");
    fs::create_dir(&composer_dir).unwrap();
    fixture.tools.composer = composer_dir.join("composer");
    fs::write(
        &fixture.tools.composer,
        "#!/usr/bin/env php\ncomposer script\n",
    )
    .unwrap();
    fs::set_permissions(&fixture.tools.composer, fs::Permissions::from_mode(0o755)).unwrap();
    fixture.script(
        &fixture.tools.php,
        r#"
if [ "$1" = --version ]; then echo 'PHP reviewed'; exit 0; fi
if [ "$1" = artisan ]; then echo 'tests passed'; exit 0; fi
case "$1" in */composer) echo 'Composer using reviewed PHP';; *) exit 90;; esac
"#,
    );
    fixture.script(
        &fixture.tools.php.parent().unwrap().join("composer"),
        "echo 'Composer shadow'",
    );
    // npm's env shebang must also find the reviewed Node, despite the older sibling binary.
    fs::write(&fixture.tools.npm, "#!/usr/bin/env node\n").unwrap();
    let body = format!("node --version\ncomposer --version\nnpm --version\n{SCAFFOLD}");
    fixture.installer(&body);
    let preview = fixture.preview("app").await;
    assert_eq!(preview.versions["node"], "v26.reviewed");
    assert_eq!(preview.versions["npm"], "v26.reviewed");
    assert_eq!(preview.versions["composer"], "Composer using reviewed PHP");
    let confirmation = preview.confirm();
    let (_sender, receiver) = watch::channel(false);
    let mut output = Vec::new();
    let project = execute_create(preview, confirmation, receiver, |event| {
        if let CreateEvent::Output(line) = event {
            output.push(line);
        }
    })
    .await
    .unwrap();
    assert_eq!(
        output.iter().filter(|line| *line == "v26.reviewed").count(),
        2
    );
    assert!(output
        .iter()
        .any(|line| line == "Composer using reviewed PHP"));
    assert!(output.iter().all(|line| !line.contains("shadow")));
    assert!(project.root.exists());
}

#[test]
fn tool_bindings_are_private_and_removed_on_drop() {
    let fixture = Fixture::new();
    let binding = ToolPath::new(&fixture.tools).unwrap();
    let path = binding.path.clone();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let output = std::process::Command::new(path.join("node"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("fake tool 1.0"));
    drop(binding);
    assert!(!path.exists());
    let mut tools = fixture.tools.clone();
    tools.npm = fixture.root.join("missing npm");
    assert!(ToolPath::new(&tools).is_err());
}

#[test]
fn tool_wrappers_preserve_native_paths_arguments_and_script_location() {
    use std::os::unix::ffi::OsStringExt;
    let mut fixture = Fixture::new();
    let mut directory_name = b"native tools ' ".to_vec();
    #[cfg(target_os = "linux")]
    directory_name.push(0xff);
    #[cfg(not(target_os = "linux"))]
    directory_name.extend_from_slice("工具".as_bytes());
    let directory = fixture.root.join(OsString::from_vec(directory_name));
    fs::create_dir(&directory).unwrap();
    fixture.tools.node = directory.join("node");
    fixture.script(&fixture.tools.node, r#"printf '%s\n' "$0" "$1""#);
    let binding = ToolPath::new(&fixture.tools).unwrap();
    let argument = "literal ' \n $(not-a-command)";
    let output = std::process::Command::new(binding.path.join("node"))
        .arg(argument)
        .output()
        .unwrap();
    assert!(output.status.success());
    let mut expected = fixture.tools.node.as_os_str().as_encoded_bytes().to_vec();
    expected.push(b'\n');
    expected.extend_from_slice(argument.as_bytes());
    expected.push(b'\n');
    assert_eq!(output.stdout, expected);
    assert_eq!(
        fs::metadata(binding.path.join("node"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}
