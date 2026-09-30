//! Project configuration and local approval of deploy settings.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::script::{is_safe_branch, wrap_step};
use crate::DeployTarget;

const DEFAULT_STEPS: [&str; 4] = [
    "composer install --no-dev --no-interaction --prefer-dist --optimize-autoloader",
    "php artisan migrate --force",
    "php artisan optimize",
    "php artisan queue:restart",
];
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("`{0}` is not inside a git repository")]
    NoGitRoot(PathBuf),
    #[error(
        "no `.shipslip.toml` found between `{0}` and its git root; pass --config to select a file"
    )]
    NotFound(PathBuf),
    #[error("could not access `{path}`: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid `{path}`: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("recipe step {step} for `{env}` failed the local bash syntax check: {message}")]
    InvalidStep {
        env: String,
        step: usize,
        message: String,
    },
    #[error("invalid trust store `{path}`: {source}")]
    TrustParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("could not serialize trust data: {0}")]
    TrustSerialize(#[from] serde_json::Error),
    #[error("unsupported trust store version {0}")]
    TrustVersion(u32),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    project: Project,
    env: BTreeMap<String, Environment>,
    recipe: Option<Recipes>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Project {
    name: String,
    stack: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Environment {
    #[serde(rename = "ssh")]
    ssh_alias: String,
    path: String,
    branch: String,
    production: bool,
    log: Option<String>,
    #[serde(default)]
    log_daily: bool,
    smoke_url: Option<String>,
    #[serde(default)]
    maintenance: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipes {
    deploy: Option<Recipe>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    steps: Vec<String>,
}

/// One parsed `.shipslip.toml`, tied to a local git repository.
#[derive(Debug)]
pub struct LoadedConfig {
    path: PathBuf,
    repo_root: PathBuf,
    project_name: String,
    envs: BTreeMap<String, Environment>,
    steps: Vec<String>,
    default_recipe: bool,
}

impl LoadedConfig {
    pub fn load(start: &Path, explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let start = fs::canonicalize(start).map_err(|source| ConfigError::Io {
            path: start.to_path_buf(),
            source,
        })?;
        let repo_root = git_root(&start).ok_or_else(|| ConfigError::NoGitRoot(start.clone()))?;
        let path = if let Some(path) = explicit {
            fs::canonicalize(path).map_err(|source| ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })?
        } else {
            config_path(&start, &repo_root).ok_or_else(|| ConfigError::NotFound(start.clone()))?
        };
        let raw = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        let config: FileConfig = toml::from_str(&raw).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
        if config.project.name.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "project.name must not be empty".into(),
            ));
        }
        if config.project.stack != "laravel" {
            return Err(ConfigError::Invalid(
                "project.stack must be `laravel` in this release".into(),
            ));
        }
        if config.env.is_empty() {
            return Err(ConfigError::Invalid(
                "at least one [env.NAME] is required".into(),
            ));
        }
        let (steps, default_recipe) = match config.recipe.and_then(|recipes| recipes.deploy) {
            Some(recipe) => (recipe.steps, false),
            None => (
                DEFAULT_STEPS
                    .iter()
                    .map(|step| (*step).to_string())
                    .collect(),
                true,
            ),
        };
        if steps.is_empty() {
            return Err(ConfigError::Invalid(
                "recipe.deploy.steps must not be empty".into(),
            ));
        }
        for (name, env) in &config.env {
            validate_environment(name, env)?;
            for (index, step) in steps.iter().enumerate() {
                check_step_syntax(name, index + 1, &env.path, step)?;
            }
        }

        Ok(Self {
            path,
            repo_root,
            project_name: config.project.name,
            envs: config.env,
            steps,
            default_recipe,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    pub fn uses_default_recipe(&self) -> bool {
        self.default_recipe
    }

    pub fn environment_names(&self) -> impl Iterator<Item = &str> {
        self.envs.keys().map(String::as_str)
    }

    pub fn target(&self, name: &str) -> Option<DeployTarget> {
        let env = self.envs.get(name)?;
        Some(DeployTarget {
            env: name.to_string(),
            production: env.production,
            ssh_alias: env.ssh_alias.clone(),
            path: env.path.clone(),
            branch: env.branch.clone(),
            steps: self.steps.clone(),
            maintenance: env.maintenance,
        })
    }

    pub fn trust_snapshot(&self, name: &str) -> Option<TrustSnapshot> {
        let env = self.envs.get(name)?;
        Some(TrustSnapshot {
            ssh_alias: env.ssh_alias.clone(),
            path: env.path.clone(),
            branch: env.branch.clone(),
            production: env.production,
            maintenance: env.maintenance,
            log: env.log.clone(),
            log_daily: env.log_daily,
            smoke_url: env.smoke_url.clone(),
            steps: self.steps.clone(),
        })
    }
}

fn git_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

fn config_path(start: &Path, root: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let path = dir.join(".shipslip.toml");
        if path.is_file() {
            return Some(path);
        }
        if dir == root {
            break;
        }
    }
    None
}

fn validate_environment(name: &str, env: &Environment) -> Result<(), ConfigError> {
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(ConfigError::Invalid(format!(
            "environment name `{name}` must not be empty or contain whitespace"
        )));
    }
    if env.ssh_alias.is_empty()
        || env.ssh_alias.starts_with('-')
        || !env
            .ssh_alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ConfigError::Invalid(format!(
            "env.{name}.ssh must be an SSH host alias"
        )));
    }
    if !Path::new(&env.path).is_absolute() {
        return Err(ConfigError::Invalid(format!(
            "env.{name}.path must be an absolute path"
        )));
    }
    if !is_safe_branch(&env.branch) {
        return Err(ConfigError::Invalid(format!(
            "env.{name}.branch is not a supported branch name"
        )));
    }
    Ok(())
}

fn check_step_syntax(env: &str, step: usize, path: &str, body: &str) -> Result<(), ConfigError> {
    let mut child = Command::new("bash")
        .arg("-n")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|error| ConfigError::InvalidStep {
            env: env.into(),
            step,
            message: error.to_string(),
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(wrap_step(path, body).as_bytes())
            .map_err(|error| ConfigError::InvalidStep {
                env: env.into(),
                step,
                message: error.to_string(),
            })?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| ConfigError::InvalidStep {
            env: env.into(),
            step,
            message: error.to_string(),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ConfigError::InvalidStep {
            env: env.into(),
            step,
            message: String::from_utf8_lossy(&output.stderr).trim().into(),
        })
    }
}

/// The settings approved for one environment. A stored copy enables `slip trust`
/// to show what changed since the last approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustSnapshot {
    pub ssh_alias: String,
    pub path: String,
    pub branch: String,
    pub production: bool,
    pub maintenance: bool,
    pub log: Option<String>,
    pub log_daily: bool,
    pub smoke_url: Option<String>,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustStatus {
    Trusted,
    Untrusted { previous: Option<TrustSnapshot> },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustRecord {
    hash: String,
    snapshot: TrustSnapshot,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustStore {
    version: u32,
    repositories: BTreeMap<String, BTreeMap<String, TrustRecord>>,
}

impl Default for TrustStore {
    fn default() -> Self {
        Self {
            version: 1,
            repositories: BTreeMap::new(),
        }
    }
}

pub fn default_trust_path() -> Result<PathBuf, ConfigError> {
    let home =
        std::env::var_os("HOME").ok_or_else(|| ConfigError::Invalid("HOME is not set".into()))?;
    let home = PathBuf::from(home);
    #[cfg(target_os = "macos")]
    let path = home.join("Library/Application Support/Shipslip/trust.json");
    #[cfg(not(target_os = "macos"))]
    let path = home.join(".config/shipslip/trust.json");
    Ok(path)
}

pub fn trust_status(
    path: &Path,
    repo_root: &Path,
    env: &str,
    snapshot: &TrustSnapshot,
) -> Result<TrustStatus, ConfigError> {
    let store = load_trust_store(path)?;
    let key = repo_root.to_string_lossy();
    let record = store
        .repositories
        .get(key.as_ref())
        .and_then(|envs| envs.get(env));
    match record {
        Some(record) if record.hash == snapshot_hash(snapshot)? => Ok(TrustStatus::Trusted),
        Some(record) => Ok(TrustStatus::Untrusted {
            previous: Some(record.snapshot.clone()),
        }),
        None => Ok(TrustStatus::Untrusted { previous: None }),
    }
}

pub fn approve_trust(
    path: &Path,
    repo_root: &Path,
    env: &str,
    snapshot: TrustSnapshot,
) -> Result<(), ConfigError> {
    let mut store = load_trust_store(path)?;
    let record = TrustRecord {
        hash: snapshot_hash(&snapshot)?,
        snapshot,
    };
    store
        .repositories
        .entry(repo_root.to_string_lossy().into_owned())
        .or_default()
        .insert(env.to_string(), record);
    save_trust_store(path, &store)
}

fn snapshot_hash(snapshot: &TrustSnapshot) -> Result<String, ConfigError> {
    let bytes = serde_json::to_vec(snapshot)?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn load_trust_store(path: &Path) -> Result<TrustStore, ConfigError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(TrustStore::default()),
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let store: TrustStore =
        serde_json::from_slice(&bytes).map_err(|source| ConfigError::TrustParse {
            path: path.to_path_buf(),
            source,
        })?;
    if store.version != 1 {
        return Err(ConfigError::TrustVersion(store.version));
    }
    Ok(store)
}

fn save_trust_store(path: &Path, store: &TrustStore) -> Result<(), ConfigError> {
    let parent = path
        .parent()
        .ok_or_else(|| ConfigError::Invalid("trust store path has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|source| {
            ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            }
        })?;
    }
    let suffix = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("tmp.{}.{}", std::process::id(), suffix));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|source| ConfigError::Io {
        path: temp.clone(),
        source,
    })?;
    serde_json::to_writer_pretty(&mut file, store)?;
    file.write_all(b"\n").map_err(|source| ConfigError::Io {
        path: temp.clone(),
        source,
    })?;
    file.sync_all().map_err(|source| ConfigError::Io {
        path: temp.clone(),
        source,
    })?;
    fs::rename(&temp, path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("shipslip-config-{}-{id}", std::process::id()));
            fs::create_dir_all(root.join(".git")).unwrap();
            Self { root }
        }

        fn write_config(&self, extra: &str, alias: &str, step: &str) {
            fs::write(
                self.root.join(".shipslip.toml"),
                format!(
                    "[project]\nname = \"example\"\nstack = \"laravel\"\n\n\
                     [env.staging]\nssh = \"{alias}\"\npath = \"/srv/example\"\n\
                     branch = \"main\"\nproduction = false\n{extra}\n\
                     [recipe.deploy]\nsteps = [\"{step}\"]\n"
                ),
            )
            .unwrap();
        }

        fn load(&self) -> Result<LoadedConfig, ConfigError> {
            LoadedConfig::load(&self.root, None)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn strict_config_and_local_syntax_check() {
        let fixture = Fixture::new();
        fixture.write_config("unknown = true", "staging-host", "echo ok");
        assert!(matches!(fixture.load(), Err(ConfigError::Parse { .. })));

        fixture.write_config("", "staging-host", "echo 'unterminated");
        assert!(matches!(
            fixture.load(),
            Err(ConfigError::InvalidStep { step: 1, .. })
        ));

        fixture.write_config("", "staging-host", "echo ok");
        let config = fixture.load().unwrap();
        assert_eq!(config.target("staging").unwrap().steps, ["echo ok"]);
    }

    #[test]
    fn new_or_changed_alias_and_step_require_new_approval() {
        let fixture = Fixture::new();
        let store = fixture.root.join("trust.json");
        fixture.write_config("", "staging-one", "echo one");
        let config = fixture.load().unwrap();
        let snapshot = config.trust_snapshot("staging").unwrap();
        assert_eq!(
            trust_status(&store, config.repo_root(), "staging", &snapshot).unwrap(),
            TrustStatus::Untrusted { previous: None }
        );
        approve_trust(&store, config.repo_root(), "staging", snapshot.clone()).unwrap();
        assert_eq!(
            trust_status(&store, config.repo_root(), "staging", &snapshot).unwrap(),
            TrustStatus::Trusted
        );

        fixture.write_config("", "staging-two", "echo one");
        let changed_alias = fixture.load().unwrap().trust_snapshot("staging").unwrap();
        assert!(matches!(
            trust_status(&store, config.repo_root(), "staging", &changed_alias).unwrap(),
            TrustStatus::Untrusted { previous: Some(_) }
        ));
        approve_trust(&store, config.repo_root(), "staging", changed_alias.clone()).unwrap();

        let mut changed_production = changed_alias;
        changed_production.production = true;
        assert!(matches!(
            trust_status(&store, config.repo_root(), "staging", &changed_production).unwrap(),
            TrustStatus::Untrusted { previous: Some(_) }
        ));

        fixture.write_config("", "staging-two", "echo two");
        let changed_step = fixture.load().unwrap().trust_snapshot("staging").unwrap();
        assert!(matches!(
            trust_status(&store, config.repo_root(), "staging", &changed_step).unwrap(),
            TrustStatus::Untrusted { previous: Some(_) }
        ));
    }

    #[test]
    fn production_must_be_explicit() {
        let fixture = Fixture::new();
        fixture.write_config("", "staging-host", "echo ok");
        let path = fixture.root.join(".shipslip.toml");
        let raw = fs::read_to_string(&path)
            .unwrap()
            .replace("production = false\n", "");
        fs::write(path, raw).unwrap();
        assert!(matches!(fixture.load(), Err(ConfigError::Parse { .. })));
    }
}
