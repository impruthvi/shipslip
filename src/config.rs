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
use crate::{DeployTarget, LogChannel};

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
    #[error("`{0}` already exists; edit it instead of running init")]
    AlreadyExists(PathBuf),
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
    timezone: Option<String>,
    #[serde(default)]
    logs: BTreeMap<String, ChannelSettings>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelSettings {
    #[serde(default)]
    hide: bool,
    rename: Option<String>,
    path: Option<String>,
}

impl From<ChannelSettings> for LogChannel {
    fn from(settings: ChannelSettings) -> Self {
        let ChannelSettings { hide, rename, path } = settings;
        Self { hide, rename, path }
    }
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
        Self::parse(&raw, path, repo_root)
    }

    fn parse(raw: &str, path: PathBuf, repo_root: PathBuf) -> Result<Self, ConfigError> {
        let config: FileConfig = toml::from_str(raw).map_err(|source| ConfigError::Parse {
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

    // Both destructure `Environment` without `..`, so a new setting must be
    // added to the deploy target and to the approved snapshot.
    pub fn target(&self, name: &str) -> Option<DeployTarget> {
        let Environment {
            ssh_alias,
            path,
            branch,
            production,
            log,
            log_daily,
            smoke_url,
            maintenance,
            timezone,
            logs,
        } = self.envs.get(name)?.clone();
        Some(DeployTarget {
            env: name.to_string(),
            production,
            ssh_alias,
            path,
            branch,
            steps: self.steps.clone(),
            maintenance,
            watch_log: true,
            log,
            log_daily,
            smoke_url,
            timezone,
            logs: log_channels(logs),
        })
    }

    pub fn trust_snapshot(&self, name: &str) -> Option<TrustSnapshot> {
        let Environment {
            ssh_alias,
            path,
            branch,
            production,
            log,
            log_daily,
            smoke_url,
            maintenance,
            timezone,
            logs,
        } = self.envs.get(name)?.clone();
        Some(TrustSnapshot {
            ssh_alias,
            path,
            branch,
            production,
            maintenance,
            log,
            log_daily,
            smoke_url,
            timezone,
            logs: log_channels(logs),
            steps: self.steps.clone(),
        })
    }
}

fn log_channels(settings: BTreeMap<String, ChannelSettings>) -> BTreeMap<String, LogChannel> {
    settings
        .into_iter()
        .map(|(name, settings)| (name, settings.into()))
        .collect()
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
    if !is_ssh_alias(&env.ssh_alias) {
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
    if env.log.as_ref().is_some_and(|log| log.trim().is_empty()) {
        return Err(ConfigError::Invalid(format!(
            "env.{name}.log must not be empty"
        )));
    }
    if env
        .smoke_url
        .as_deref()
        .is_some_and(|url| !is_smoke_url(url))
    {
        return Err(ConfigError::Invalid(format!(
            "env.{name}.smoke_url must be an HTTP or HTTPS URL without whitespace"
        )));
    }
    if let Some(timezone) = &env.timezone {
        if !is_known_timezone(timezone) {
            return Err(ConfigError::Invalid(format!(
                "env.{name}.timezone `{timezone}` is not a known IANA timezone"
            )));
        }
    }
    for (channel, settings) in &env.logs {
        validate_channel(name, channel, settings)?;
    }
    Ok(())
}

fn validate_channel(
    env: &str,
    channel: &str,
    settings: &ChannelSettings,
) -> Result<(), ConfigError> {
    let invalid = |message: &str| {
        Err(ConfigError::Invalid(format!(
            "env.{env}.logs.{channel} {message}"
        )))
    };
    if !is_channel_name(channel) {
        return invalid("must be named with letters, digits, `.`, `_` or `-`");
    }
    let ChannelSettings { hide, rename, path } = settings;
    if rename
        .as_deref()
        .is_some_and(|rename| !is_channel_name(rename))
    {
        return invalid("rename must use letters, digits, `.`, `_` or `-`");
    }
    if path.as_deref().is_some_and(|path| path.trim().is_empty()) {
        return invalid("path must not be empty");
    }
    match (hide, rename, path) {
        (true, None, None) | (false, Some(_), _) | (false, None, Some(_)) => Ok(()),
        (true, _, _) => invalid("cannot both hide and rename or add a path"),
        (false, None, None) => invalid("must set hide, rename or path"),
    }
}

fn is_channel_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A zone with a file in the local tz database. GNU `date` on the server
/// silently treats an unknown zone as UTC, so names are checked here.
fn is_known_timezone(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
        && Path::new("/usr/share/zoneinfo").join(name).is_file()
}

/// An OpenSSH host alias that cannot be read as an option.
pub fn is_ssh_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('-')
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// An HTTP or HTTPS URL with a host and no whitespace.
pub fn is_smoke_url(url: &str) -> bool {
    let host = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .and_then(|rest| rest.split(['/', '?', '#']).next());
    host.is_some_and(|host| !host.is_empty()) && !url.chars().any(char::is_whitespace)
}

/// A branch name Shipslip can deploy.
pub fn is_branch_name(branch: &str) -> bool {
    is_safe_branch(branch)
}

/// An environment name `init` can use as a plain TOML key.
pub fn is_env_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// Answers for a new `.shipslip.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitAnswers {
    pub env: String,
    pub ssh_alias: String,
    pub path: String,
    pub branch: String,
    pub production: bool,
    pub maintenance: bool,
    pub smoke_url: Option<String>,
}

/// Where `init` writes a new config: the git root, if no config is found.
#[derive(Debug)]
pub struct InitPlan {
    path: PathBuf,
    repo_root: PathBuf,
    project_name: String,
}

impl InitPlan {
    pub fn new(start: &Path) -> Result<Self, ConfigError> {
        let start = fs::canonicalize(start).map_err(|source| ConfigError::Io {
            path: start.to_path_buf(),
            source,
        })?;
        let repo_root = git_root(&start).ok_or_else(|| ConfigError::NoGitRoot(start.clone()))?;
        if let Some(existing) = config_path(&start, &repo_root) {
            return Err(ConfigError::AlreadyExists(existing));
        }
        let project_name = repo_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "app".into());
        Ok(Self {
            path: repo_root.join(".shipslip.toml"),
            repo_root,
            project_name,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The config text, checked with the same rules as [`LoadedConfig::load`].
    pub fn render(&self, answers: &InitAnswers) -> Result<String, ConfigError> {
        if !is_env_name(&answers.env) {
            return Err(ConfigError::Invalid(format!(
                "environment name `{}` may only use letters, digits, `-` and `_`",
                answers.env
            )));
        }
        let quote = |value: &str| toml::Value::String(value.into()).to_string();
        let mut text = format!(
            "# Shipslip deploy config. Review every value, then run `slip trust {env}`.\n\n\
             [project]\nname = {name}\nstack = \"laravel\"\n\n\
             [env.{env}]\n\
             # Host alias from your ~/.ssh/config.\nssh = {ssh}\n\
             # Absolute path of the app's Git checkout on the server.\npath = {path}\n\
             branch = {branch}\n\
             # Production deploys ask you to type the environment name to confirm.\n\
             production = {production}\n\
             # Runs `php artisan down` before the steps and `php artisan up` once they all succeed.\n\
             maintenance = {maintenance}\n",
            env = answers.env,
            name = quote(&self.project_name),
            ssh = quote(&answers.ssh_alias),
            path = quote(&answers.path),
            branch = quote(&answers.branch),
            production = answers.production,
            maintenance = answers.maintenance,
        );
        match &answers.smoke_url {
            Some(url) => text.push_str(&format!(
                "# Requested from your computer after the steps; 2xx passes.\nsmoke_url = {}\n",
                quote(url)
            )),
            None => text.push_str("# smoke_url = \"https://example.com/health\"\n"),
        }
        text.push_str(
            "\n[recipe.deploy]\n\
             # Run in order on the server. Remove any step that is not safe for this app.\n\
             steps = [\n",
        );
        for step in DEFAULT_STEPS {
            if step.contains("migrate") {
                text.push_str("  # Runs database migrations on every deploy.\n");
            }
            text.push_str(&format!("  {},\n", quote(step)));
        }
        text.push_str("]\n");
        LoadedConfig::parse(&text, self.path.clone(), self.repo_root.clone())?;
        Ok(text)
    }

    /// Writes the config. Never replaces an existing file.
    pub fn write(&self, answers: &InitAnswers) -> Result<(), ConfigError> {
        let text = self.render(answers)?;
        let io_error = |source| ConfigError::Io {
            path: self.path.clone(),
            source,
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => ConfigError::AlreadyExists(self.path.clone()),
                _ => io_error(error),
            })?;
        file.write_all(text.as_bytes()).map_err(|error| {
            let _ = fs::remove_file(&self.path);
            io_error(error)
        })
    }
}

/// Syntax added in bash 4 that bash 3.2, the stock macOS bash, rejects.
const BASH4_SYNTAX: [&str; 3] = ["|&", ";&", "&>>"];

fn local_bash_major() -> Option<u32> {
    let output = Command::new("bash")
        .args(["-c", "echo ${BASH_VERSINFO[0]}"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
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
    if output.status.success()
        || (BASH4_SYNTAX.iter().any(|syntax| body.contains(syntax))
            && local_bash_major().is_some_and(|major| major < 4))
    {
        // Older local bash cannot parse this step; the server's `bash -n`
        // in preflight decides before anything changes.
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
/// to show what changed since the last approval. Trust is decided by the
/// stored hash, so stored copies from other versions load leniently.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrustSnapshot {
    pub ssh_alias: String,
    pub path: String,
    pub branch: String,
    pub production: bool,
    pub maintenance: bool,
    pub log: Option<String>,
    pub log_daily: bool,
    pub smoke_url: Option<String>,
    // Left out when unset so approvals made before `slip logs` keep their hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub logs: BTreeMap<String, LogChannel>,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustStatus {
    Trusted,
    Untrusted { previous: Option<TrustSnapshot> },
}

#[derive(Debug, Serialize, Deserialize)]
struct TrustRecord {
    hash: String,
    snapshot: TrustSnapshot,
}

#[derive(Debug, Serialize, Deserialize)]
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

    fn answers() -> InitAnswers {
        InitAnswers {
            env: "staging".into(),
            ssh_alias: "app-staging".into(),
            path: "/var/www/app".into(),
            branch: "main".into(),
            production: false,
            maintenance: true,
            smoke_url: Some("https://staging.example.com/health".into()),
        }
    }

    #[test]
    fn init_writes_a_config_that_loads_with_an_explicit_recipe() {
        let fixture = Fixture::new();
        let plan = InitPlan::new(&fixture.root).unwrap();
        plan.write(&answers()).unwrap();

        let text = fs::read_to_string(fixture.root.join(".shipslip.toml")).unwrap();
        assert!(text.contains(
            "# Runs database migrations on every deploy.\n  \"php artisan migrate --force\""
        ));
        let config = fixture.load().unwrap();
        assert!(!config.uses_default_recipe());
        assert_eq!(
            config.project_name(),
            fixture.root.file_name().unwrap().to_str().unwrap()
        );
        let target = config.target("staging").unwrap();
        assert_eq!(target.ssh_alias, "app-staging");
        assert_eq!(target.path, "/var/www/app");
        assert_eq!(target.branch, "main");
        assert!(!target.production);
        assert!(target.maintenance);
        assert_eq!(
            target.smoke_url.as_deref(),
            Some("https://staging.example.com/health")
        );
        assert_eq!(target.steps, DEFAULT_STEPS);
    }

    #[test]
    fn init_keeps_production_and_an_unset_smoke_url() {
        let fixture = Fixture::new();
        let answers = InitAnswers {
            env: "production".into(),
            production: true,
            maintenance: false,
            smoke_url: None,
            ..answers()
        };
        InitPlan::new(&fixture.root)
            .unwrap()
            .write(&answers)
            .unwrap();
        let target = fixture.load().unwrap().target("production").unwrap();
        assert!(target.production);
        assert!(!target.maintenance);
        assert_eq!(target.smoke_url, None);
    }

    #[test]
    fn init_quotes_values_that_need_escaping() {
        let fixture = Fixture::new();
        let answers = InitAnswers {
            path: "/srv/it's \"here\"\\app".into(),
            ..answers()
        };
        InitPlan::new(&fixture.root)
            .unwrap()
            .write(&answers)
            .unwrap();
        let target = fixture.load().unwrap().target("staging").unwrap();
        assert_eq!(target.path, answers.path);
    }

    #[test]
    fn init_refuses_when_a_config_already_exists() {
        let fixture = Fixture::new();
        let sub = fixture.root.join("app/Http");
        fs::create_dir_all(&sub).unwrap();
        fixture.write_config("", "staging-host", "echo ok");
        for start in [&fixture.root, &sub] {
            assert!(matches!(
                InitPlan::new(start),
                Err(ConfigError::AlreadyExists(path)) if path.ends_with(".shipslip.toml")
            ));
        }

        // Created between the check and the write.
        let fresh = Fixture::new();
        let plan = InitPlan::new(&fresh.root).unwrap();
        fresh.write_config("", "staging-host", "echo ok");
        assert!(matches!(
            plan.write(&answers()),
            Err(ConfigError::AlreadyExists(_))
        ));
        assert!(fresh.load().is_ok());
    }

    #[test]
    fn init_writes_at_the_git_root_and_rejects_invalid_answers() {
        let fixture = Fixture::new();
        let sub = fixture.root.join("app");
        fs::create_dir_all(&sub).unwrap();
        let plan = InitPlan::new(&sub).unwrap();
        assert_eq!(
            plan.path(),
            fs::canonicalize(&fixture.root)
                .unwrap()
                .join(".shipslip.toml")
        );
        for bad in [
            InitAnswers {
                env: "stag ing".into(),
                ..answers()
            },
            InitAnswers {
                ssh_alias: "-oProxyCommand=x".into(),
                ..answers()
            },
            InitAnswers {
                path: "var/www".into(),
                ..answers()
            },
            InitAnswers {
                branch: "-main".into(),
                ..answers()
            },
            InitAnswers {
                smoke_url: Some("ftp://example.com".into()),
                ..answers()
            },
        ] {
            assert!(
                matches!(plan.render(&bad), Err(ConfigError::Invalid(_))),
                "{bad:?}"
            );
            assert!(plan.write(&bad).is_err());
        }
        assert!(!plan.path().exists());
    }

    #[test]
    fn answer_checks_match_the_config_rules() {
        assert!(is_ssh_alias("app-staging.example_1"));
        assert!(!is_ssh_alias(""));
        assert!(!is_ssh_alias("-oProxyCommand=x"));
        assert!(!is_ssh_alias("user@host"));
        assert!(is_smoke_url("http://16.171.70.231/"));
        assert!(is_smoke_url("https://example.com/health?x=1"));
        assert!(!is_smoke_url("https://"));
        assert!(!is_smoke_url("example.com"));
        assert!(!is_smoke_url("https://example.com/a b"));
        assert!(is_branch_name("release/1.2"));
        assert!(!is_branch_name("-main"));
        assert!(is_env_name("staging_2-eu"));
        assert!(!is_env_name("env.staging"));
        assert!(!is_env_name(""));
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

        // Valid on the server's bash even where local bash is 3.2.
        fixture.write_config("", "staging-host", "echo ok |& cat");
        assert!(fixture.load().is_ok());
    }

    /// Approvals are stored as this hash; settings a config never used must
    /// not change it, or every user would have to re-trust after upgrading.
    #[test]
    fn trust_hash_is_unchanged_without_log_settings() {
        let snapshot = TrustSnapshot {
            ssh_alias: "app-production".into(),
            path: "/srv/app".into(),
            branch: "main".into(),
            production: true,
            maintenance: true,
            log: Some("storage/logs/laravel".into()),
            log_daily: true,
            smoke_url: Some("https://example.com/health".into()),
            timezone: None,
            logs: BTreeMap::new(),
            steps: vec!["php artisan migrate --force".into()],
        };
        assert_eq!(
            snapshot_hash(&snapshot).unwrap(),
            "8fe0bb8f3eede4f205d430b192c1ab9777ea4ba639c849e55fa7b19d32a43779"
        );

        let mut with_timezone = snapshot.clone();
        with_timezone.timezone = Some("Asia/Kolkata".into());
        let mut with_logs = snapshot.clone();
        with_logs.logs.insert(
            "worker".into(),
            LogChannel {
                hide: true,
                ..LogChannel::default()
            },
        );
        for changed in [with_timezone, with_logs] {
            assert_ne!(
                snapshot_hash(&changed).unwrap(),
                snapshot_hash(&snapshot).unwrap()
            );
        }
    }

    #[test]
    fn log_settings_load_into_the_target_and_snapshot() {
        let fixture = Fixture::new();
        fixture.write_config(
            "timezone = \"Asia/Kolkata\"\n\
             [env.staging.logs.worker]\nhide = true\n\
             [env.staging.logs.payments]\nrename = \"billing\"\n\
             [env.staging.logs.queue]\npath = \"/var/log/queue.log\"\n",
            "staging-host",
            "echo ok",
        );
        let config = fixture.load().unwrap();
        let target = config.target("staging").unwrap();
        assert_eq!(target.timezone.as_deref(), Some("Asia/Kolkata"));
        assert_eq!(
            target.logs.keys().collect::<Vec<_>>(),
            ["payments", "queue", "worker"]
        );
        assert!(target.logs["worker"].hide);
        assert_eq!(target.logs["payments"].rename.as_deref(), Some("billing"));
        assert_eq!(
            target.logs["queue"].path.as_deref(),
            Some("/var/log/queue.log")
        );
        let snapshot = config.trust_snapshot("staging").unwrap();
        assert_eq!(snapshot.timezone, target.timezone);
        assert_eq!(snapshot.logs, target.logs);
    }

    #[test]
    fn invalid_log_settings_are_rejected() {
        let fixture = Fixture::new();
        let invalid = [
            (
                "timezone = \"Asia/Kolkatta\"\n",
                "not a known IANA timezone",
            ),
            (
                "timezone = \"../../etc/passwd\"\n",
                "not a known IANA timezone",
            ),
            ("timezone = \"\"\n", "not a known IANA timezone"),
            (
                "[env.staging.logs.worker]\n",
                "must set hide, rename or path",
            ),
            (
                "[env.staging.logs.worker]\nhide = true\nrename = \"jobs\"\n",
                "cannot both hide",
            ),
            (
                "[env.staging.logs.worker]\nrename = \"a b\"\n",
                "rename must use",
            ),
            (
                "[env.staging.logs.worker]\npath = \" \"\n",
                "path must not be empty",
            ),
            (
                "[env.staging.logs.\".hidden\"]\nhide = true\n",
                "must be named",
            ),
        ];
        for (extra, message) in invalid {
            fixture.write_config(extra, "staging-host", "echo ok");
            let error = fixture.load().unwrap_err().to_string();
            assert!(error.contains(message), "{extra:?}: {error}");
        }

        fixture.write_config(
            "[env.staging.logs.worker]\nhide = true\nlevel = \"error\"\n",
            "staging-host",
            "echo ok",
        );
        assert!(matches!(fixture.load(), Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn trust_store_written_by_another_version_still_loads() {
        let fixture = Fixture::new();
        let store = fixture.root.join("trust.json");
        fixture.write_config("", "staging-host", "echo ok");
        let config = fixture.load().unwrap();
        let snapshot = config.trust_snapshot("staging").unwrap();
        let mut stored = serde_json::to_value(&snapshot).unwrap();
        stored.as_object_mut().unwrap().remove("smoke_url");
        stored["future_setting"] = serde_json::json!(true);
        let repo = config.repo_root().to_string_lossy().into_owned();
        let record = serde_json::json!({
            "hash": snapshot_hash(&snapshot).unwrap(),
            "snapshot": stored,
            "approved_by": "a later version",
        });
        let json = serde_json::json!({
            "version": 1,
            "repositories": { repo: { "staging": record } },
        });
        fs::write(&store, serde_json::to_vec(&json).unwrap()).unwrap();

        let status = |snapshot: &TrustSnapshot| {
            trust_status(&store, config.repo_root(), "staging", snapshot).unwrap()
        };
        assert_eq!(status(&snapshot), TrustStatus::Trusted);
        let mut changed = snapshot.clone();
        changed.branch = "release".into();
        let TrustStatus::Untrusted {
            previous: Some(previous),
        } = status(&changed)
        else {
            panic!("changed branch should need approval");
        };
        assert_eq!(previous.branch, snapshot.branch);
        assert_eq!(previous.smoke_url, None);
        approve_trust(&store, config.repo_root(), "staging", changed.clone()).unwrap();
        assert_eq!(status(&changed), TrustStatus::Trusted);
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
