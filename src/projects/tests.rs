use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::config::{ConfigError, InitAnswers, InitPlan};

/// A temp folder for the projects list and checkouts, removed on drop.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "shipslip-projects-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn list_path(&self) -> PathBuf {
        self.0.join("support/projects.json")
    }

    fn checkout(&self, name: &str) -> PathBuf {
        let root = self.0.join(name);
        fs::create_dir_all(root.join(".git")).unwrap();
        root
    }

    fn folder(&self, name: &str) -> PathBuf {
        let root = self.0.join(name);
        fs::create_dir_all(&root).unwrap();
        root
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const CONFIG: &str = "[project]\nname = \"shop\"\nstack = \"laravel\"\n\n\
    [env.staging]\nssh = \"web1\"\npath = \"/srv/shop\"\nbranch = \"main\"\nproduction = false\n\n\
    [env.production]\nssh = \"web2\"\npath = \"/srv/shop\"\nbranch = \"main\"\nproduction = true\n";

#[test]
fn a_missing_list_is_empty() {
    let fx = Fixture::new();
    assert_eq!(list(&fx.list_path()).unwrap(), Vec::new());
}

#[test]
fn add_lists_the_git_root_with_its_date() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    fs::create_dir_all(shop.join("app/Http")).unwrap();

    let added = add(&fx.list_path(), &shop.join("app/Http"), 1_000).unwrap();

    assert_eq!(added, shop);
    assert_eq!(
        list(&fx.list_path()).unwrap(),
        vec![Checkout {
            path: shop,
            added_at_ms: 1_000
        }]
    );
}

#[test]
fn adding_again_keeps_one_entry_and_its_first_date() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    let blog = fx.checkout("blog");
    add(&fx.list_path(), &shop, 1_000).unwrap();
    add(&fx.list_path(), &blog, 2_000).unwrap();
    add(&fx.list_path(), &shop, 3_000).unwrap();

    let listed = list(&fx.list_path()).unwrap();
    assert_eq!(
        listed,
        vec![
            Checkout {
                path: shop,
                added_at_ms: 1_000
            },
            Checkout {
                path: blog,
                added_at_ms: 2_000
            },
        ]
    );
}

#[test]
fn add_refuses_a_folder_outside_git_and_writes_nothing() {
    let fx = Fixture::new();
    let loose = fx.folder("loose");

    let error = add(&fx.list_path(), &loose, 1_000).unwrap_err();

    assert!(matches!(error, ProjectsError::NotGit(path) if path == loose));
    assert!(!fx.list_path().exists());
}

#[test]
fn the_list_is_written_owner_only_with_no_temp_file_left() {
    let fx = Fixture::new();
    add(&fx.list_path(), &fx.checkout("shop"), 1_000).unwrap();

    let names: Vec<_> = fs::read_dir(fx.list_path().parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["projects.json"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(fx.list_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn an_unreadable_list_is_an_error_and_add_leaves_it_alone() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.list_path().parent().unwrap()).unwrap();
    fs::write(fx.list_path(), "not json").unwrap();

    assert!(matches!(
        list(&fx.list_path()),
        Err(ProjectsError::Parse { .. })
    ));
    assert!(add(&fx.list_path(), &fx.checkout("shop"), 1_000).is_err());
    assert_eq!(fs::read_to_string(fx.list_path()).unwrap(), "not json");
}

#[test]
fn a_list_from_a_newer_version_is_refused() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.list_path().parent().unwrap()).unwrap();
    fs::write(fx.list_path(), r#"{"version":2,"checkouts":[]}"#).unwrap();

    assert!(matches!(
        list(&fx.list_path()),
        Err(ProjectsError::Version(2))
    ));
}

#[test]
fn the_list_sits_beside_the_approvals_file() {
    assert_eq!(
        path_beside(Path::new("/support/Shipslip/trust.json")),
        Path::new("/support/Shipslip/projects.json")
    );
}

#[test]
fn inspect_refuses_a_folder_outside_git() {
    let fx = Fixture::new();
    assert!(matches!(inspect(&fx.folder("loose")), Folder::NotGit));
    assert!(matches!(inspect(&fx.0.join("missing")), Folder::NotGit));
}

#[test]
fn inspect_reads_the_config_at_the_git_root() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    fs::write(shop.join(".shipslip.toml"), CONFIG).unwrap();
    fs::create_dir_all(shop.join("app")).unwrap();

    let Folder::Config(config) = inspect(&shop.join("app")) else {
        panic!("expected a config");
    };
    assert_eq!(config.repo_root(), shop);
    assert_eq!(config.project_name(), "shop");
    assert_eq!(
        config.environment_names().collect::<Vec<_>>(),
        ["production", "staging"]
    );
}

#[test]
fn inspect_plans_a_new_config_when_there_is_none() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    fs::write(
        shop.join("package.json"),
        r#"{"scripts":{"build":"vite build"}}"#,
    )
    .unwrap();
    fs::write(shop.join("yarn.lock"), "").unwrap();

    let Folder::NewConfig(plan) = inspect(&shop) else {
        panic!("expected a plan");
    };
    assert_eq!(plan.repo_root(), shop);
    assert_eq!(plan.project_name(), "shop");
    assert_eq!(plan.warnings().len(), 1, "{:?}", plan.warnings());
}

fn answers(env: &str) -> InitAnswers {
    InitAnswers {
        env: env.into(),
        ssh_alias: "web1".into(),
        path: "/srv/shop".into(),
        branch: "main".into(),
        production: false,
        maintenance: true,
        smoke_url: None,
    }
}

impl Fixture {
    fn trust_path(&self) -> PathBuf {
        self.0.join("support/trust.json")
    }

    fn plan(&self, checkout: &Path) -> InitPlan {
        match inspect(checkout) {
            Folder::NewConfig(plan) => plan,
            other => panic!("expected a plan, got {other:?}"),
        }
    }
}

fn approved(trust_path: &Path, checkout: &Path, env: &str) -> bool {
    let config = LoadedConfig::load(checkout, None).unwrap();
    config.trusted_target(env, trust_path).is_ok()
}

#[test]
fn add_with_config_writes_what_was_rendered_approves_it_and_lists_the_checkout() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    let plan = fx.plan(&shop);
    let text = plan.render(&answers("staging")).unwrap();

    let added = add_with_config(&fx.trust_path(), &plan, &answers("staging"), 5).unwrap();

    assert_eq!(added.checkout, shop);
    assert!(added.approval.is_ok());
    assert_eq!(
        fs::read_to_string(shop.join(".shipslip.toml")).unwrap(),
        text
    );
    assert!(approved(&fx.trust_path(), &shop, "staging"));
    let listed = list(&path_beside(&fx.trust_path())).unwrap();
    assert_eq!(listed.iter().map(|c| &c.path).collect::<Vec<_>>(), [&shop]);
}

#[test]
fn add_with_config_never_replaces_an_existing_config() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    let plan = fx.plan(&shop);
    // Written by someone else after the sheet read the folder.
    fs::write(shop.join(".shipslip.toml"), CONFIG).unwrap();

    let error = add_with_config(&fx.trust_path(), &plan, &answers("staging"), 5).unwrap_err();

    assert!(
        matches!(error, AddConfigError::Write(ConfigError::AlreadyExists(_))),
        "{error:?}"
    );
    assert_eq!(
        fs::read_to_string(shop.join(".shipslip.toml")).unwrap(),
        CONFIG
    );
    assert!(!fx.trust_path().exists(), "nothing approved");
    // Listed before the write, as the sheet lists it on leaving the folder step.
    let listed = list(&path_beside(&fx.trust_path())).unwrap();
    assert_eq!(listed.iter().map(|c| &c.path).collect::<Vec<_>>(), [&shop]);
}

#[test]
fn add_with_config_writes_nothing_when_the_list_cannot_be_written() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    let plan = fx.plan(&shop);
    fs::create_dir_all(fx.trust_path().parent().unwrap()).unwrap();
    fs::write(path_beside(&fx.trust_path()), "not json").unwrap();

    let error = add_with_config(&fx.trust_path(), &plan, &answers("staging"), 5).unwrap_err();

    assert!(matches!(error, AddConfigError::List(_)), "{error:?}");
    assert!(!shop.join(".shipslip.toml").exists());
    assert!(!fx.trust_path().exists());
}

#[test]
fn add_with_config_refuses_answers_that_break_a_rule_and_writes_nothing() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    let plan = fx.plan(&shop);

    let error = add_with_config(&fx.trust_path(), &plan, &answers("env.staging"), 5).unwrap_err();

    assert!(matches!(error, AddConfigError::Write(_)), "{error:?}");
    assert!(!shop.join(".shipslip.toml").exists());
}

#[test]
fn a_failed_approval_after_the_write_leaves_the_checkout_listed_and_unapproved() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    let plan = fx.plan(&shop);
    fs::create_dir_all(fx.trust_path().parent().unwrap()).unwrap();
    fs::write(fx.trust_path(), "not json").unwrap();

    let added = add_with_config(&fx.trust_path(), &plan, &answers("staging"), 5).unwrap();

    assert!(added.approval.is_err());
    assert!(shop.join(".shipslip.toml").exists());
    let listed = list(&path_beside(&fx.trust_path())).unwrap();
    assert_eq!(listed.iter().map(|c| &c.path).collect::<Vec<_>>(), [&shop]);
    fs::remove_file(fx.trust_path()).unwrap();
    assert!(!approved(&fx.trust_path(), &shop, "staging"));
}

#[test]
fn add_with_config_approves_the_rendered_text_not_a_later_edit() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    let plan = fx.plan(&shop);
    add_with_config(&fx.trust_path(), &plan, &answers("staging"), 5).unwrap();
    let written = fs::read_to_string(shop.join(".shipslip.toml")).unwrap();

    fs::write(
        shop.join(".shipslip.toml"),
        written.replace("\"web1\"", "\"web9\""),
    )
    .unwrap();

    assert!(!approved(&fx.trust_path(), &shop, "staging"));
}

#[test]
fn inspect_says_when_the_config_is_invalid() {
    let fx = Fixture::new();
    let shop = fx.checkout("shop");
    fs::write(shop.join(".shipslip.toml"), "[project]\nname = \"\"\n").unwrap();

    assert!(matches!(
        inspect(&shop),
        Folder::Unreadable { checkout, .. } if checkout == shop
    ));
}
