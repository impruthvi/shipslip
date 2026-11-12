//! The Add project sheet: what a picked folder holds, adding its checkout to
//! the projects list, and writing a new config from the answers.

use std::path::Path;

use serde::{Deserialize, Serialize};
use shipslip::config::{trust_status, InitAnswers, InitField, InitPlan, TrustStatus};
use shipslip::git::Git;
use shipslip::logs::escape_field;
use shipslip::projects::{self, AddConfigError, Folder};

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FolderView {
    /// The folder panel was closed without a pick.
    Cancelled,
    NotGit {
        folder: String,
    },
    /// A git checkout with no `.shipslip.toml`: the sheet asks the `init`
    /// questions.
    NewConfig {
        checkout: String,
        /// The branch checked out now, the branch question's default.
        branch: Option<String>,
    },
    Unreadable {
        checkout: String,
        /// The config error, one entry per line.
        message: Vec<String>,
    },
    Config {
        checkout: String,
        project: String,
        envs: Vec<FolderEnv>,
    },
}

/// One environment as the config reads now. Shown read only.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEnv {
    pub name: String,
    /// `ssh_alias:path`.
    pub target: String,
    pub branch: String,
    pub production: bool,
    /// The settings as they read now are approved on this Mac.
    pub approved: bool,
}

/// Reads `folder` for the sheet. An approvals file that can't be read
/// approves nothing.
pub fn inspect(folder: &Path, trust_path: &Path, git: Option<&Git>) -> FolderView {
    let field = |path: &Path| escape_field(&path.to_string_lossy());
    match projects::inspect(folder) {
        Folder::NotGit => FolderView::NotGit {
            folder: field(folder),
        },
        Folder::NewConfig(plan) => FolderView::NewConfig {
            checkout: field(plan.repo_root()),
            branch: git
                .and_then(|git| git.current_branch(plan.repo_root()).ok())
                .filter(|branch| InitField::Branch.check(branch).is_ok()),
        },
        Folder::Unreadable { checkout, error } => FolderView::Unreadable {
            checkout: field(&checkout),
            message: error.to_string().lines().map(escape_field).collect(),
        },
        Folder::Config(config) => {
            let mut names: Vec<&str> = config.environment_names().collect();
            names.sort_by(|a, b| shipslip::records::env_order(a, b));
            let envs = names
                .into_iter()
                .filter_map(|name| {
                    let target = config.target(name)?;
                    let snapshot = config.trust_snapshot(name)?;
                    let approved = matches!(
                        trust_status(trust_path, config.repo_root(), name, &snapshot),
                        Ok(TrustStatus::Trusted)
                    );
                    Some(FolderEnv {
                        name: escape_field(name),
                        target: escape_field(&format!("{}:{}", target.ssh_alias, target.path)),
                        branch: escape_field(&target.branch),
                        production: target.production,
                        approved,
                    })
                })
                .collect();
            FolderView::Config {
                checkout: field(config.repo_root()),
                project: escape_field(config.project_name()),
                envs,
            }
        }
    }
}

/// Lists the checkout `folder` belongs to and returns its path as the map
/// spells it. Only a folder whose config reads, or that has none yet, is
/// added; the sheet never offers anything else.
pub fn add(folder: &Path, trust_path: &Path, now_ms: u128) -> Result<String, String> {
    match projects::inspect(folder) {
        Folder::Config(_) | Folder::NewConfig(_) => {}
        Folder::NotGit => return Err(not_git(folder)),
        Folder::Unreadable { .. } => {
            return Err("This folder's .shipslip.toml can't be read.".into())
        }
    }
    projects::add(&projects::path_beside(trust_path), folder, now_ms)
        .map(|checkout| escape_field(&checkout.to_string_lossy()))
        .map_err(|error| error.to_string())
}

/// The answers to the `init` questions, as the sheet's fields hold them.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answers {
    pub env: String,
    pub ssh_alias: String,
    pub path: String,
    pub branch: String,
    pub production: bool,
    pub maintenance: bool,
    /// Empty: no smoke check.
    pub smoke_url: String,
}

impl From<Answers> for InitAnswers {
    fn from(answers: Answers) -> Self {
        let smoke_url = answers.smoke_url.trim().to_string();
        Self {
            env: answers.env.trim().into(),
            ssh_alias: answers.ssh_alias.trim().into(),
            path: answers.path.trim().into(),
            branch: answers.branch.trim().into(),
            production: answers.production,
            maintenance: answers.maintenance,
            smoke_url: (!smoke_url.is_empty()).then_some(smoke_url),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldProblem {
    pub field: InitField,
    pub message: String,
}

/// The config the answers would write, for the approval step.
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConfigText {
    /// Answers that break the library's rules, one entry per field.
    Invalid { problems: Vec<FieldProblem> },
    Rendered {
        /// The `.shipslip.toml` text, one entry per line.
        lines: Vec<String>,
        /// Frontend setup the recipe leaves out; read before approving.
        warnings: Vec<String>,
    },
}

/// Renders the config for the checkout `folder` belongs to. The plan comes
/// back with the text so the add writes exactly what was shown.
pub fn render(
    folder: &Path,
    answers: &InitAnswers,
) -> Result<(ConfigText, Option<InitPlan>), String> {
    let problems = answers.problems();
    if !problems.is_empty() {
        let problems = problems
            .into_iter()
            .map(|(field, message)| FieldProblem {
                field,
                message: message.into(),
            })
            .collect();
        return Ok((ConfigText::Invalid { problems }, None));
    }
    let plan = match projects::inspect(folder) {
        Folder::NewConfig(plan) => plan,
        Folder::NotGit => return Err(not_git(folder)),
        Folder::Config(_) => {
            return Err(
                "This checkout has a .shipslip.toml now. Choose the folder again to read it."
                    .into(),
            )
        }
        Folder::Unreadable { error, .. } => return Err(escape_field(&error.to_string())),
    };
    let text = plan
        .render(answers)
        .map_err(|error| escape_field(&error.to_string()))?;
    let view = ConfigText::Rendered {
        lines: text.lines().map(escape_field).collect(),
        warnings: plan.warnings().iter().map(|w| escape_field(w)).collect(),
    };
    Ok((view, Some(plan)))
}

/// What "Approve and add" did.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigAdded {
    /// The checkout, as the map spells it.
    pub checkout: String,
    /// The config was written but approving it failed, so the checkout
    /// needs approval.
    pub approval_error: Option<String>,
}

/// Writes the rendered config, approves it and lists the checkout.
pub fn add_with_config(
    plan: &InitPlan,
    answers: &InitAnswers,
    trust_path: &Path,
    now_ms: u128,
) -> Result<ConfigAdded, String> {
    let added =
        projects::add_with_config(trust_path, plan, answers, now_ms).map_err(|error| match error {
            AddConfigError::Write(shipslip::config::ConfigError::AlreadyExists(_)) => {
                "This checkout has a .shipslip.toml now, so Shipslip did not write one. Choose the folder again to read it.".into()
            }
            error => escape_field(&error.to_string()),
        })?;
    Ok(ConfigAdded {
        checkout: escape_field(&added.checkout.to_string_lossy()),
        approval_error: added
            .approval
            .err()
            .map(|error| escape_field(&error.to_string())),
    })
}

fn not_git(folder: &Path) -> String {
    format!(
        "{} is not inside a git repository, so there is nothing to deploy from it.",
        folder.display()
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use shipslip::config::{approve_trust, LoadedConfig};

    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "shipslip-add-project-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }

        fn trust(&self) -> PathBuf {
            self.0.join("support/trust.json")
        }

        fn checkout(&self, config: Option<&str>) -> PathBuf {
            let root = self.0.join("shop");
            fs::create_dir_all(root.join(".git")).unwrap();
            if let Some(config) = config {
                fs::write(root.join(".shipslip.toml"), config).unwrap();
            }
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
        [env.production]\nssh = \"web2\"\npath = \"/srv/shop\"\nbranch = \"release\"\nproduction = true\n";

    #[test]
    fn a_config_shows_each_environment_and_whether_it_is_approved() {
        let fx = Fixture::new();
        let shop = fx.checkout(Some(CONFIG));
        let config = LoadedConfig::load(&shop, None).unwrap();
        approve_trust(
            &fx.trust(),
            &shop,
            "staging",
            config.trust_snapshot("staging").unwrap(),
        )
        .unwrap();

        let json = serde_json::to_value(inspect(&shop, &fx.trust(), None)).unwrap();
        assert_eq!(json["kind"], "config");
        assert_eq!(json["checkout"], shop.to_string_lossy().as_ref());
        assert_eq!(json["project"], "shop");
        assert_eq!(
            json["envs"],
            serde_json::json!([
                {"name": "production", "target": "web2:/srv/shop", "branch": "release", "production": true, "approved": false},
                {"name": "staging", "target": "web1:/srv/shop", "branch": "main", "production": false, "approved": true},
            ])
        );
    }

    #[test]
    fn a_folder_outside_git_is_refused_and_not_added() {
        let fx = Fixture::new();
        let loose = fx.0.join("loose");
        fs::create_dir_all(&loose).unwrap();

        let json = serde_json::to_value(inspect(&loose, &fx.trust(), None)).unwrap();
        assert_eq!(json["kind"], "notGit");
        let error = add(&loose, &fx.trust(), 1).unwrap_err();
        assert!(error.contains("not inside a git repository"), "{error}");
        assert!(!projects::path_beside(&fx.trust()).exists());
    }

    #[test]
    fn a_checkout_without_a_config_asks_the_questions_and_can_be_listed() {
        let fx = Fixture::new();
        let shop = fx.checkout(None);

        let json = serde_json::to_value(inspect(&shop, &fx.trust(), None)).unwrap();
        assert_eq!(json["kind"], "newConfig");
        assert_eq!(json["checkout"], shop.to_string_lossy().as_ref());
        assert_eq!(add(&shop, &fx.trust(), 1).unwrap(), shop.to_string_lossy());
        assert!(!shop.join(".shipslip.toml").exists());
    }

    fn answers(env: &str) -> InitAnswers {
        Answers {
            env: env.into(),
            ssh_alias: " web1 ".into(),
            path: "/srv/shop".into(),
            branch: "main".into(),
            production: false,
            maintenance: true,
            smoke_url: " ".into(),
        }
        .into()
    }

    #[test]
    fn render_names_each_answer_that_breaks_the_librarys_rule() {
        let fx = Fixture::new();
        let shop = fx.checkout(None);
        let mut bad = answers("env.staging");
        bad.path = "srv".into();

        let (view, plan) = render(&shop, &bad).unwrap();

        assert!(plan.is_none());
        assert_eq!(
            serde_json::to_value(view).unwrap(),
            serde_json::json!({"kind": "invalid", "problems": [
                {"field": "env", "message": "Use only letters, digits, `-` and `_`."},
                {"field": "path", "message": "Use an absolute path, such as /var/www/app."},
            ]})
        );
    }

    #[test]
    fn render_shows_the_text_init_writes_and_the_add_writes_it() {
        let fx = Fixture::new();
        let shop = fx.checkout(None);
        let answers = answers("staging");

        let (view, plan) = render(&shop, &answers).unwrap();
        let json = serde_json::to_value(view).unwrap();
        assert_eq!(json["kind"], "rendered");
        let lines: Vec<String> = serde_json::from_value(json["lines"].clone()).unwrap();
        assert!(lines.contains(&"ssh = \"web1\"".to_string()), "{lines:?}");

        let added = add_with_config(&plan.unwrap(), &answers, &fx.trust(), 3).unwrap();
        assert_eq!(added.checkout, shop.to_string_lossy());
        assert_eq!(added.approval_error, None);
        let written = fs::read_to_string(shop.join(".shipslip.toml")).unwrap();
        assert_eq!(written.lines().collect::<Vec<_>>(), lines);
    }

    #[test]
    fn a_failed_approval_after_the_write_says_so_and_the_checkout_needs_approval() {
        let fx = Fixture::new();
        let shop = fx.checkout(None);
        let (_, plan) = render(&shop, &answers("staging")).unwrap();
        fs::create_dir_all(fx.trust().parent().unwrap()).unwrap();
        fs::write(fx.trust(), "not json").unwrap();

        let added = add_with_config(&plan.unwrap(), &answers("staging"), &fx.trust(), 3).unwrap();

        assert!(added.approval_error.is_some());
        assert!(shop.join(".shipslip.toml").exists());
        let json = serde_json::to_value(inspect(&shop, &fx.trust(), None)).unwrap();
        assert_eq!(json["envs"][0]["approved"], false);
    }

    #[test]
    fn render_refuses_a_folder_that_is_no_longer_a_new_config() {
        let fx = Fixture::new();
        let loose = fx.0.join("loose");
        fs::create_dir_all(&loose).unwrap();
        let error = render(&loose, &answers("staging")).unwrap_err();
        assert!(error.contains("not inside a git repository"), "{error}");

        let shop = fx.checkout(Some(CONFIG));
        let error = render(&shop, &answers("staging")).unwrap_err();
        assert!(error.contains("has a .shipslip.toml now"), "{error}");

        fs::write(shop.join(".shipslip.toml"), "[project]\nname = \"\"\n").unwrap();
        let error = render(&shop, &answers("staging")).unwrap_err();
        assert!(error.starts_with("invalid"), "{error}");
    }

    #[test]
    fn the_add_never_replaces_a_config_written_since_the_render() {
        let fx = Fixture::new();
        let shop = fx.checkout(None);
        let (_, plan) = render(&shop, &answers("staging")).unwrap();
        fs::write(shop.join(".shipslip.toml"), CONFIG).unwrap();

        let error =
            add_with_config(&plan.unwrap(), &answers("staging"), &fx.trust(), 3).unwrap_err();

        assert!(error.contains("did not write one"), "{error}");
        assert_eq!(
            fs::read_to_string(shop.join(".shipslip.toml")).unwrap(),
            CONFIG
        );
    }

    #[test]
    fn an_invalid_config_says_why() {
        let fx = Fixture::new();
        let shop = fx.checkout(Some("[project]\nname = \"\"\n"));

        let json = serde_json::to_value(inspect(&shop, &fx.trust(), None)).unwrap();
        assert_eq!(json["kind"], "unreadable");
        assert!(json["message"][0].as_str().unwrap().starts_with("invalid"));
    }

    #[test]
    fn add_lists_the_checkout_beside_the_approvals_file() {
        let fx = Fixture::new();
        let shop = fx.checkout(Some(CONFIG));

        assert_eq!(add(&shop, &fx.trust(), 7).unwrap(), shop.to_string_lossy());
        let listed = projects::list(&projects::path_beside(&fx.trust())).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, shop);
    }
}
