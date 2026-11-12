//! The Add project sheet's folder step: what a picked folder holds, and
//! adding its checkout to the projects list.

use std::path::Path;

use serde::Serialize;
use shipslip::config::{trust_status, TrustStatus};
use shipslip::logs::escape_field;
use shipslip::projects::{self, Folder};

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
    NoConfig {
        checkout: String,
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
pub fn inspect(folder: &Path, trust_path: &Path) -> FolderView {
    let field = |path: &Path| escape_field(&path.to_string_lossy());
    match projects::inspect(folder) {
        Folder::NotGit => FolderView::NotGit {
            folder: field(folder),
        },
        Folder::NoConfig { checkout } => FolderView::NoConfig {
            checkout: field(&checkout),
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
/// spells it. Only a folder whose config reads is added; the sheet never
/// offers anything else.
pub fn add(folder: &Path, trust_path: &Path, now_ms: u128) -> Result<String, String> {
    match projects::inspect(folder) {
        Folder::Config(_) => {}
        Folder::NotGit => return Err(not_git(folder)),
        Folder::NoConfig { .. } | Folder::Unreadable { .. } => {
            return Err("This folder has no .shipslip.toml that Shipslip can read.".into())
        }
    }
    projects::add(&projects::path_beside(trust_path), folder, now_ms)
        .map(|checkout| escape_field(&checkout.to_string_lossy()))
        .map_err(|error| error.to_string())
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

        let json = serde_json::to_value(inspect(&shop, &fx.trust())).unwrap();
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

        let json = serde_json::to_value(inspect(&loose, &fx.trust())).unwrap();
        assert_eq!(json["kind"], "notGit");
        let error = add(&loose, &fx.trust(), 1).unwrap_err();
        assert!(error.contains("not inside a git repository"), "{error}");
        assert!(!projects::path_beside(&fx.trust()).exists());
    }

    #[test]
    fn a_checkout_without_a_config_is_not_added() {
        let fx = Fixture::new();
        let shop = fx.checkout(None);

        let json = serde_json::to_value(inspect(&shop, &fx.trust())).unwrap();
        assert_eq!(json["kind"], "noConfig");
        assert!(add(&shop, &fx.trust(), 1).is_err());
        assert!(!projects::path_beside(&fx.trust()).exists());
    }

    #[test]
    fn an_invalid_config_says_why() {
        let fx = Fixture::new();
        let shop = fx.checkout(Some("[project]\nname = \"\"\n"));

        let json = serde_json::to_value(inspect(&shop, &fx.trust())).unwrap();
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
