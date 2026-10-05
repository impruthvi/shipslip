//! Read-only local toolchain discovery and health checks.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::create::{CreateError, Tools};
use crate::laravel::{Database, InstallerOptions};
use crate::publish::PublishTools;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Create,
    Publish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementId {
    Tool(&'static str),
    Extension(&'static str),
    Identity(&'static str),
    GithubAuth,
}
impl std::fmt::Display for RequirementId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tool(name) => f.write_str(name),
            Self::Extension(name) => write!(f, "php.{name}"),
            Self::Identity(name) => write!(f, "git.{name}"),
            Self::GithubAuth => f.write_str("github.auth"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub id: RequirementId,
    pub purposes: Vec<Purpose>,
    pub blocking: bool,
}

pub const PHP_EXTENSIONS: &[&str] = &[
    "ctype",
    "curl",
    "dom",
    "fileinfo",
    "filter",
    "hash",
    "mbstring",
    "openssl",
    "pcre",
    "pdo",
    "session",
    "tokenizer",
    "xml",
    "pdo_sqlite",
];
const CREATE_TOOLS: &[&str] = &["php", "composer", "laravel", "node", "npm", "git"];

pub fn requirements(purposes: &[Purpose], database: Database) -> Vec<Requirement> {
    let create = purposes.contains(&Purpose::Create);
    let publish = purposes.contains(&Purpose::Publish);
    let mut requirements = Vec::new();
    if create {
        for name in CREATE_TOOLS {
            requirements.push(Requirement {
                id: RequirementId::Tool(name),
                purposes: if *name == "git" && publish {
                    vec![Purpose::Create, Purpose::Publish]
                } else {
                    vec![Purpose::Create]
                },
                blocking: true,
            });
        }
        for extension in PHP_EXTENSIONS.iter().copied().chain(match database {
            Database::Sqlite => None,
            Database::Mysql | Database::Mariadb => Some("pdo_mysql"),
            Database::Pgsql => Some("pdo_pgsql"),
            Database::Sqlsrv => Some("pdo_sqlsrv"),
        }) {
            requirements.push(Requirement {
                id: RequirementId::Extension(extension),
                purposes: vec![Purpose::Create],
                blocking: PHP_EXTENSIONS.contains(&extension),
            });
        }
        for name in ["user.name", "user.email"] {
            requirements.push(Requirement {
                id: RequirementId::Identity(name),
                purposes: vec![Purpose::Create],
                blocking: true,
            });
        }
    }
    if publish {
        for name in if create {
            &["gh"][..]
        } else {
            &["git", "gh"][..]
        } {
            requirements.push(Requirement {
                id: RequirementId::Tool(name),
                purposes: vec![Purpose::Publish],
                blocking: true,
            });
        }
        requirements.push(Requirement {
            id: RequirementId::GithubAuth,
            purposes: vec![Purpose::Publish],
            blocking: true,
        });
    }
    requirements
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingState {
    Ok { path: PathBuf, version: String },
    Missing,
    TooOld { found: String, need: String },
    Broken { path: PathBuf, error: String },
    OffPath { path: PathBuf, dir: PathBuf },
    Unverified { reason: String },
}
impl FindingState {
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::Missing | Self::TooOld { .. } | Self::Broken { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Homebrew,
    System,
    Herd,
    HerdLite,
    Asdf,
    Mise,
    Nvm,
    Fnm,
    ComposerGlobal,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Manager {
    Herd,
    HerdLite,
    Asdf,
    Mise,
    Nvm,
    Fnm,
}

#[derive(Debug, Clone)]
pub struct DetectionOptions {
    pub purposes: Vec<Purpose>,
    pub root: PathBuf,
    pub installer_options: Option<InstallerOptions>,
}

#[derive(Debug, Clone)]
pub struct DetectionContext {
    pub environment: BTreeMap<OsString, OsString>,
    pub macos: bool,
    pub system_bin: PathBuf,
    pub xdg_system: PathBuf,
    pub herd_app: PathBuf,
    pub brew_candidates: Vec<PathBuf>,
}
impl DetectionContext {
    pub fn from_environment() -> Self {
        Self {
            environment: std::env::vars_os().collect(),
            macos: cfg!(target_os = "macos"),
            system_bin: "/usr/bin".into(),
            xdg_system: "/etc/xdg".into(),
            herd_app: "/Applications/Herd.app".into(),
            brew_candidates: vec![
                "/opt/homebrew/bin/brew".into(),
                "/usr/local/bin/brew".into(),
            ],
        }
    }
    fn env(&self, name: &str) -> Option<PathBuf> {
        self.environment
            .get(std::ffi::OsStr::new(name))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
    fn path_dirs(&self) -> Vec<PathBuf> {
        self.environment
            .get(std::ffi::OsStr::new("PATH"))
            .map(|value| std::env::split_paths(value).collect())
            .unwrap_or_default()
    }
    fn home_path(&self, path: &str) -> Option<PathBuf> {
        self.env("HOME").map(|home| home.join(path))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLocation {
    pub entry: PathBuf,
    pub path: PathBuf,
    pub off_path: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryWarning {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Homebrew {
    pub binary: PathBuf,
    pub prefix: PathBuf,
    pub bin: PathBuf,
    pub writable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MachineFacts {
    pub homebrew: Option<Homebrew>,
    pub composer_home: Option<PathBuf>,
    pub composer_bin: Option<PathBuf>,
    pub managers: Vec<Manager>,
    pub providers: BTreeMap<String, Vec<Manager>>,
    pub command_line_tools: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct LookupReport {
    pub tools: BTreeMap<String, ToolLocation>,
    pub failures: BTreeMap<String, String>,
    pub facts: MachineFacts,
    pub warnings: Vec<DiscoveryWarning>,
}
impl LookupReport {
    pub fn diagnostic(&self) -> String {
        self.failures
            .iter()
            .map(|(name, reason)| format!("{name}: {reason}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
    fn path(&self, name: &str) -> Result<PathBuf, CreateError> {
        if !self.failures.is_empty() {
            return Err(CreateError::Invalid(self.diagnostic()));
        }
        self.tools
            .get(name)
            .map(|tool| tool.path.clone())
            .ok_or_else(|| CreateError::Invalid(format!("{name}: not found")))
    }
    pub fn creation_tools(&self) -> Result<Tools, CreateError> {
        Ok(Tools {
            php: self.path("php")?,
            composer: self.path("composer")?,
            laravel: self.path("laravel")?,
            node: self.path("node")?,
            npm: self.path("npm")?,
            git: self.path("git")?,
        })
    }
    pub fn publish_tools(&self) -> Result<PublishTools, CreateError> {
        Ok(PublishTools::new(self.path("git")?, self.path("gh")?))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("could not inspect {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Create(#[from] CreateError),
}

fn executable(path: &Path) -> bool {
    path.is_file() && {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
}

pub fn composer_uses_xdg(context: &DetectionContext) -> bool {
    context
        .environment
        .keys()
        .any(|key| key.to_string_lossy().starts_with("XDG_"))
        || context.xdg_system.is_dir()
}

pub fn composer_home(context: &DetectionContext) -> Option<PathBuf> {
    if let Some(home) = context.env("COMPOSER_HOME") {
        return Some(home);
    }
    let home = context.env("HOME")?;
    let mut candidates = Vec::new();
    if composer_uses_xdg(context) {
        candidates.push(
            context
                .env("XDG_CONFIG_HOME")
                .unwrap_or_else(|| home.join(".config"))
                .join("composer"),
        );
    }
    candidates.push(home.join(".composer"));
    candidates
        .iter()
        .find(|path| path.is_dir())
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

pub fn composer_bin_dir(home: &Path) -> (PathBuf, Option<DiscoveryWarning>) {
    let default = home.join("vendor/bin");
    let path = home.join("config.json");
    let parsed = match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| error.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (default, None),
        Err(error) => Err(error.to_string()),
    };
    let parsed = parsed.and_then(|value| {
        if !value.is_object()
            || value
                .get("config")
                .is_some_and(|config| !config.is_object())
        {
            Err("expected a JSON object with an object-valued config".into())
        } else {
            Ok(value)
        }
    });
    match parsed {
        Ok(value) => match value.get("config").and_then(|config| config.get("bin-dir")) {
            None => (default, None),
            Some(serde_json::Value::String(bin)) if !bin.is_empty() => (home.join(bin), None),
            Some(_) => (
                default,
                Some(DiscoveryWarning {
                    path,
                    message: "config.bin-dir must be a non-empty string; using vendor/bin".into(),
                }),
            ),
        },
        Err(error) => (
            default,
            Some(DiscoveryWarning {
                path,
                message: format!("{error}; using vendor/bin"),
            }),
        ),
    }
}

fn resolve_tool_in(
    name: &str,
    path_dirs: &[PathBuf],
    known_dirs: &[PathBuf],
) -> Result<Option<ToolLocation>, std::io::Error> {
    let selected = path_dirs
        .iter()
        .map(|dir| (dir, false))
        .chain(known_dirs.iter().map(|dir| (dir, true)))
        .map(|(dir, off_path)| (dir.join(name), off_path))
        .find(|(entry, _)| executable(entry));
    selected
        .map(|(entry, off_path)| {
            fs::canonicalize(&entry).map(|path| ToolLocation {
                entry,
                path,
                off_path,
            })
        })
        .transpose()
}

fn discover_paths(
    context: &DetectionContext,
) -> (MachineFacts, Vec<DiscoveryWarning>, Vec<PathBuf>) {
    let path_dirs = context.path_dirs();
    let brew = path_dirs
        .iter()
        .map(|dir| dir.join("brew"))
        .chain(context.brew_candidates.iter().cloned())
        .find(|path| executable(path));
    let mut facts = MachineFacts::default();
    let mut known = Vec::new();
    if let Some(binary) = brew {
        let target = fs::canonicalize(&binary).unwrap_or_else(|_| binary.clone());
        let directory = target.parent().unwrap_or(Path::new("."));
        let mut prefix = directory.parent().unwrap_or(directory).to_path_buf();
        if prefix.file_name() == Some(std::ffi::OsStr::new("Homebrew")) {
            prefix = prefix.parent().unwrap_or(&prefix).to_path_buf();
        } else if directory.ends_with("Library/Homebrew") {
            prefix = directory
                .parent()
                .and_then(Path::parent)
                .unwrap_or(directory)
                .to_path_buf();
        }
        let bin = prefix.join("bin");
        let writable = crate::create::writable(&prefix).is_ok();
        known.push(bin.clone());
        facts.homebrew = Some(Homebrew {
            binary,
            prefix,
            bin,
            writable,
        });
    }
    let mut warnings = Vec::new();
    facts.composer_home = composer_home(context);
    if let Some(home) = &facts.composer_home {
        let (bin, warning) = composer_bin_dir(home);
        known.push(bin.clone());
        facts.composer_bin = Some(bin);
        warnings.extend(warning);
    }
    manager_providers(context, &mut facts);
    (facts, warnings, known)
}

pub fn detect_tools(context: &DetectionContext, names: &[&str]) -> LookupReport {
    let (facts, warnings, known) = discover_paths(context);
    let mut report = LookupReport {
        tools: BTreeMap::new(),
        failures: BTreeMap::new(),
        facts,
        warnings,
    };
    for name in names {
        match resolve_tool_in(name, &context.path_dirs(), &known) {
            Ok(Some(tool)) => {
                report.tools.insert((*name).into(), tool);
            }
            Ok(None) => {
                report.failures.insert((*name).into(), "not found".into());
            }
            Err(error) => {
                report.failures.insert((*name).into(), error.to_string());
            }
        }
    }
    report
}

pub fn resolve_tool(name: &str) -> Result<PathBuf, CreateError> {
    detect_tools(&DetectionContext::from_environment(), &[name]).path(name)
}

fn fnm_dirs(context: &DetectionContext) -> Vec<PathBuf> {
    let data = context
        .env("XDG_DATA_HOME")
        .or_else(|| context.home_path(".local/share"));
    context
        .env("FNM_DIR")
        .into_iter()
        .chain(data.map(|data| data.join("fnm")))
        .chain(context.home_path(".fnm"))
        .chain(
            context
                .macos
                .then(|| context.home_path("Library/Application Support/fnm"))
                .flatten(),
        )
        .collect()
}

fn manager_providers(context: &DetectionContext, facts: &mut MachineFacts) {
    let exists = |path: Option<PathBuf>| path.is_some_and(|path| path.is_dir());
    let on_path = |name| {
        resolve_tool_in(name, &context.path_dirs(), &[])
            .ok()
            .flatten()
            .is_some()
    };
    let asdf = context
        .env("ASDF_DATA_DIR")
        .or_else(|| context.home_path(".asdf"));
    let mise = context
        .env("MISE_DATA_DIR")
        .or_else(|| context.home_path(".local/share/mise"));
    let installed = [
        (
            Manager::Nvm,
            exists(context.env("NVM_DIR")) || exists(context.home_path(".nvm")),
        ),
        (
            Manager::Fnm,
            on_path("fnm") || fnm_dirs(context).iter().any(|path| path.is_dir()),
        ),
        (Manager::Asdf, on_path("asdf") || exists(asdf.clone())),
        (
            Manager::Mise,
            on_path("mise")
                || exists(mise.clone())
                || context
                    .home_path(".local/bin/mise")
                    .is_some_and(|path| executable(&path)),
        ),
        (Manager::Herd, context.herd_app.is_dir()),
        (
            Manager::HerdLite,
            exists(context.home_path(".config/herd-lite")),
        ),
    ];
    facts.managers = installed
        .into_iter()
        .filter_map(|(manager, present)| present.then_some(manager))
        .collect();
    for name in ["php", "composer", "laravel", "node", "npm", "git", "gh"] {
        let mut providers = Vec::new();
        for manager in &facts.managers {
            let provides = match manager {
                Manager::Nvm | Manager::Fnm => matches!(name, "node" | "npm"),
                Manager::HerdLite => matches!(name, "php" | "composer" | "laravel"),
                Manager::Herd => {
                    matches!(name, "php" | "composer" | "laravel")
                        || (name == "node"
                            && exists(
                                context.home_path("Library/Application Support/Herd/config/nvm"),
                            ))
                }
                Manager::Asdf => exists(asdf.as_ref().map(|root| {
                    root.join("plugins")
                        .join(if matches!(name, "node" | "npm") {
                            "nodejs"
                        } else {
                            name
                        })
                })),
                Manager::Mise => {
                    let provider = if matches!(name, "node" | "npm") {
                        "node"
                    } else {
                        name
                    };
                    exists(
                        mise.as_ref()
                            .map(|root| root.join("installs").join(provider)),
                    ) || (provider == "node"
                        && exists(mise.as_ref().map(|root| root.join("installs/nodejs"))))
                }
            };
            if provides {
                providers.push(*manager);
            }
        }
        facts.providers.insert(name.into(), providers);
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub requirement: Requirement,
    pub state: FindingState,
    pub location: Option<ToolLocation>,
    pub owner: Owner,
    pub managers: Vec<Manager>,
    pub shadowing: Option<PathBuf>,
    pub detail: Option<String>,
    pub version: Option<String>,
}
impl Finding {
    pub fn ready(&self) -> bool {
        !self.requirement.blocking || !self.state.is_failure()
    }
}

#[derive(Debug, Clone)]
pub struct DetectionReport {
    pub findings: Vec<Finding>,
    pub facts: MachineFacts,
    pub warnings: Vec<DiscoveryWarning>,
}
impl DetectionReport {
    pub fn ready(&self) -> bool {
        self.findings.iter().all(Finding::ready)
    }
}

fn classify_owner(path: &Path, context: &DetectionContext, facts: &MachineFacts) -> Owner {
    let under = |root: Option<PathBuf>| {
        root.is_some_and(|root| {
            let root = fs::canonicalize(&root).unwrap_or(root);
            path.starts_with(root)
        })
    };
    if path.starts_with(&context.herd_app)
        || under(context.home_path("Library/Application Support/Herd"))
    {
        Owner::Herd
    } else if under(context.home_path(".config/herd-lite")) {
        Owner::HerdLite
    } else if under(
        context
            .env("ASDF_DATA_DIR")
            .or_else(|| context.home_path(".asdf")),
    ) {
        Owner::Asdf
    } else if under(
        context
            .env("MISE_DATA_DIR")
            .or_else(|| context.home_path(".local/share/mise")),
    ) {
        Owner::Mise
    } else if under(context.env("NVM_DIR")) || under(context.home_path(".nvm")) {
        Owner::Nvm
    } else if fnm_dirs(context).into_iter().any(|path| under(Some(path))) {
        Owner::Fnm
    } else if under(facts.composer_bin.clone()) {
        Owner::ComposerGlobal
    } else if facts
        .homebrew
        .as_ref()
        .is_some_and(|brew| under(Some(brew.prefix.clone())))
    {
        Owner::Homebrew
    } else if under(Some(context.system_bin.clone())) {
        Owner::System
    } else {
        Owner::Unknown
    }
}

fn is_system_shim(context: &DetectionContext, location: &ToolLocation) -> bool {
    context.macos
        && (location.entry.parent() == Some(context.system_bin.as_path())
            || location.path.parent() == Some(context.system_bin.as_path()))
}

async fn detect_machine(context: &DetectionContext, root: &Path, facts: &mut MachineFacts) {
    if context.macos {
        let mut command = tokio::process::Command::new(context.system_bin.join("xcode-select"));
        crate::create::clean(&mut command);
        facts.command_line_tools = Some(
            command
                .args(["-p"])
                .current_dir(root)
                .output()
                .await
                .is_ok_and(|output| output.status.success()),
        );
    }
}

fn find_shadowing(name: &str, location: &ToolLocation, facts: &MachineFacts) -> Option<PathBuf> {
    if location.off_path {
        return None;
    }
    let destination = if name == "laravel" {
        facts.composer_bin.as_ref().map(|bin| bin.join(name))
    } else {
        facts.homebrew.as_ref().map(|brew| brew.bin.join(name))
    }?;
    let same = destination == location.entry
        || fs::canonicalize(&destination).is_ok_and(|path| path == location.path);
    (!same).then(|| location.entry.clone())
}

struct Probes<'a> {
    context: &'a DetectionContext,
    root: &'a Path,
    binding: crate::create::ToolPath,
    known: Vec<PathBuf>,
}
impl Probes<'_> {
    async fn output(&self, binary: &Path, args: &[&str]) -> Result<std::process::Output, String> {
        let mut command = tokio::process::Command::new(binary);
        command.envs(&self.context.environment);
        crate::create::clean(&mut command);
        let dirs = std::iter::once(self.binding.path.clone())
            .chain(self.context.path_dirs())
            .chain(self.known.iter().cloned());
        let path = std::env::join_paths(dirs).map_err(|error| error.to_string())?;
        command.env("PATH", path).current_dir(self.root).args(args);
        command.output().await.map_err(|error| error.to_string())
    }
    async fn text(&self, binary: &Path, args: &[&str]) -> Result<String, String> {
        let output = self.output(binary, args).await?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(if message.is_empty() {
                format!(
                    "{} {} failed ({})",
                    binary.display(),
                    args.join(" "),
                    output.status
                )
            } else {
                message
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let text = if stdout.trim().is_empty() {
            String::from_utf8_lossy(&output.stderr)
        } else {
            stdout
        };
        if text.trim().is_empty() {
            return Err(format!(
                "{} {} returned no output",
                binary.display(),
                args.join(" ")
            ));
        }
        Ok(text.into_owned())
    }
}

fn broken(path: &Path, error: impl Into<String>) -> FindingState {
    FindingState::Broken {
        path: path.into(),
        error: error.into(),
    }
}

fn successful(location: &ToolLocation, version: String) -> FindingState {
    if location.off_path {
        FindingState::OffPath {
            path: location.path.clone(),
            dir: location.entry.parent().unwrap_or(Path::new(".")).into(),
        }
    } else {
        FindingState::Ok {
            path: location.path.clone(),
            version,
        }
    }
}

fn probe_installer(
    version: &str,
    help: &str,
    options: Option<&InstallerOptions>,
) -> Result<String, String> {
    use crate::laravel::{Auth, StarterKit, Testing};
    if let Some(options) = options {
        return options
            .probe(version, help)
            .map_err(|error| error.to_string());
    }
    let mut found = String::new();
    for kit in [
        StarterKit::None,
        StarterKit::React,
        StarterKit::Vue,
        StarterKit::Svelte,
        StarterKit::Livewire,
    ] {
        for auth in [Auth::None, Auth::Laravel] {
            if kit == StarterKit::None && auth == Auth::Laravel {
                continue;
            }
            for testing in [Testing::Pest, Testing::Phpunit] {
                for boost in [false, true] {
                    let options = InstallerOptions {
                        starter_kit: kit,
                        auth,
                        database: Database::Sqlite,
                        testing,
                        boost,
                    };
                    found = options
                        .probe(version, help)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
    }
    Ok(found)
}

async fn probe_tool(
    name: &str,
    location: &ToolLocation,
    options: &DetectionOptions,
    probes: &Probes<'_>,
) -> (FindingState, Option<String>) {
    let text = match probes.text(&location.path, &["--version"]).await {
        Ok(text) => text,
        Err(error) => return (broken(&location.path, error), None),
    };
    let mut version = text.lines().next().unwrap_or("unknown").trim().to_string();
    if name == "laravel" {
        let found = text
            .split_whitespace()
            .find(|word| word.as_bytes().first().is_some_and(u8::is_ascii_digit));
        let parsed = found.and_then(|version| crate::laravel::parse_version(version).ok());
        let Some(parsed) = parsed else {
            return (
                broken(&location.path, "could not parse Laravel installer version"),
                Some(version),
            );
        };
        if parsed
            < crate::laravel::parse_version(crate::laravel::MIN_INSTALLER_VERSION)
                .expect("valid minimum")
        {
            return (
                FindingState::TooOld {
                    found: found.unwrap().into(),
                    need: crate::laravel::MIN_INSTALLER_VERSION.into(),
                },
                Some(version),
            );
        }
        let help = match probes.text(&location.path, &["new", "--help"]).await {
            Ok(help) => help,
            Err(error) => return (broken(&location.path, error), Some(version)),
        };
        match probe_installer(&text, &help, options.installer_options.as_ref()) {
            Ok(found) => version = found,
            Err(error) => return (broken(&location.path, error), Some(version)),
        }
    }
    (successful(location, version.clone()), Some(version))
}

async fn probe_php_extensions(
    location: Option<&ToolLocation>,
    probes: &Probes<'_>,
) -> Result<std::collections::BTreeSet<String>, String> {
    let location = location.ok_or("PHP is unavailable; extensions could not be checked")?;
    let text = probes.text(&location.path, &["-m"]).await?;
    Ok(text
        .lines()
        .map(|line| line.trim().to_ascii_lowercase())
        .collect())
}

fn probe_git_identity(
    name: &str,
    location: Option<&ToolLocation>,
    root: &Path,
) -> (FindingState, Option<String>) {
    let Some(location) = location else {
        return (
            FindingState::Unverified {
                reason: "Git is unavailable; identity could not be checked".into(),
            },
            None,
        );
    };
    match crate::git::Git::new(location.path.clone()).run(root, &["config", "--get", name]) {
        Ok(value) if !value.trim().is_empty() => (
            FindingState::Ok {
                path: location.path.clone(),
                version: "configured".into(),
            },
            None,
        ),
        Ok(_) => (
            FindingState::Missing,
            Some(format!("run git config --global {name} <value>")),
        ),
        Err(crate::git::GitError::Command { message, .. }) if message.is_empty() => (
            FindingState::Missing,
            Some(format!("run git config --global {name} <value>")),
        ),
        Err(error) => (broken(&location.path, error.to_string()), None),
    }
}

async fn probe_github_auth(
    context: &DetectionContext,
    location: Option<&ToolLocation>,
    root: &Path,
) -> (FindingState, Option<String>) {
    let Some(location) = location else {
        return (
            FindingState::Unverified {
                reason: "GitHub CLI is unavailable; authentication could not be checked".into(),
            },
            None,
        );
    };
    let credentials = crate::git::GhCredentials::new(
        ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN"]
            .into_iter()
            .filter_map(|name| {
                context
                    .environment
                    .get(std::ffi::OsStr::new(name))
                    .and_then(|value| value.to_str())
                    .map(|value| (name.into(), value.into()))
            }),
    );
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        PublishTools::auth_status(&location.path, &credentials, root),
    )
    .await;
    match output {
        Err(_) => (
            FindingState::Unverified {
                reason: "could not reach GitHub within 10 s".into(),
            },
            None,
        ),
        Ok(Err(error)) => (broken(&location.path, error.to_string()), None),
        Ok(Ok(output)) if output.status.success() => (
            FindingState::Ok {
                path: location.path.clone(),
                version: "authenticated".into(),
            },
            None,
        ),
        Ok(Ok(output)) => {
            let message = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let message = credentials.redact(message.trim());
            (FindingState::Missing, Some(format!("{message}\nRun gh auth login --hostname github.com, or set GH_TOKEN. gh may offer to change Git's credential helper.")))
        }
    }
}

pub async fn detect(
    context: &DetectionContext,
    options: &DetectionOptions,
) -> Result<DetectionReport, SetupError> {
    fs::read_dir(&options.root).map_err(|source| SetupError::Io {
        path: options.root.clone(),
        source,
    })?;
    let database = options
        .installer_options
        .as_ref()
        .map_or(Database::Sqlite, |options| options.database);
    let requirements = requirements(&options.purposes, database);
    let names = requirements
        .iter()
        .filter_map(|requirement| match requirement.id {
            RequirementId::Tool(name) => Some(name),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut lookup = detect_tools(context, &names);
    detect_machine(context, &options.root, &mut lookup.facts).await;
    let blocked = |location: &ToolLocation| {
        lookup.facts.command_line_tools == Some(false) && is_system_shim(context, location)
    };
    let binding = crate::create::ToolPath::from_paths(
        lookup
            .tools
            .iter()
            .filter(|(_, tool)| !blocked(tool))
            .map(|(name, tool)| (name.as_str(), tool.path.as_path())),
    )?;
    let known = lookup
        .facts
        .homebrew
        .iter()
        .map(|brew| brew.bin.clone())
        .chain(lookup.facts.composer_bin.iter().cloned())
        .collect();
    let probes = Probes {
        context,
        root: &options.root,
        binding,
        known,
    };
    let php = lookup.tools.get("php").filter(|tool| !blocked(tool));
    let extensions = if options.purposes.contains(&Purpose::Create) {
        probe_php_extensions(php, &probes).await
    } else {
        Err("not requested".into())
    };
    let mut findings = Vec::new();
    for requirement in requirements {
        let tool_name = match requirement.id {
            RequirementId::Tool(name) => name,
            RequirementId::Extension(_) => "php",
            RequirementId::Identity(_) => "git",
            RequirementId::GithubAuth => "gh",
        };
        let location = lookup.tools.get(tool_name).cloned();
        let owner = location.as_ref().map_or(Owner::Unknown, |tool| {
            classify_owner(&tool.path, context, &lookup.facts)
        });
        let managers = lookup
            .facts
            .providers
            .get(tool_name)
            .cloned()
            .unwrap_or_default();
        let mut detail = None;
        let mut version = None;
        let state = match requirement.id {
            RequirementId::Tool(name) => match &location {
                None => {
                    detail = lookup.failures.get(name).cloned();
                    FindingState::Missing
                }
                Some(tool) if blocked(tool) => {
                    detail =
                        Some("Command Line Tools not installed; run xcode-select --install".into());
                    FindingState::Missing
                }
                Some(tool) => {
                    let (state, found) = probe_tool(name, tool, options, &probes).await;
                    version = found;
                    state
                }
            },
            RequirementId::Extension(extension) => {
                match &extensions {
                    Ok(modules) if modules.contains(extension) => FindingState::Ok {
                        path: php.unwrap().path.clone(),
                        version: "loaded".into(),
                    },
                    Ok(_) if !requirement.blocking => {
                        let reason = if extension == "pdo_sqlsrv" {
                            format!("{extension} is not loaded; see https://learn.microsoft.com/en-us/sql/connect/php/installation-tutorial-linux-mac for Microsoft's PHP driver installation instructions")
                        } else {
                            format!("{extension} is not loaded; selected database support is unverified")
                        };
                        FindingState::Unverified { reason }
                    }
                    Ok(_) => {
                        detail = Some(if owner == Owner::Homebrew {
                            "Homebrew PHP bundles this extension; run brew reinstall php".into()
                        } else {
                            "enable this extension in the selected PHP installation".into()
                        });
                        broken(&php.unwrap().path, format!("{extension} is not loaded"))
                    }
                    Err(reason) => {
                        if let Some(php) = php {
                            if requirement.blocking {
                                broken(&php.path, reason.clone())
                            } else {
                                FindingState::Unverified {
                                    reason: reason.clone(),
                                }
                            }
                        } else {
                            FindingState::Unverified {
                                reason: reason.clone(),
                            }
                        }
                    }
                }
            }
            RequirementId::Identity(name) => {
                let (state, guidance) = probe_git_identity(
                    name,
                    location.as_ref().filter(|tool| !blocked(tool)),
                    &options.root,
                );
                detail = guidance;
                state
            }
            RequirementId::GithubAuth => {
                let (state, guidance) = probe_github_auth(
                    context,
                    location.as_ref().filter(|tool| !blocked(tool)),
                    &options.root,
                )
                .await;
                detail = guidance;
                state
            }
        };
        let shadowing = if matches!(requirement.id, RequirementId::Tool(_)) && state.is_failure() {
            location
                .as_ref()
                .and_then(|tool| find_shadowing(tool_name, tool, &lookup.facts))
        } else {
            None
        };
        findings.push(Finding {
            requirement,
            state,
            location,
            owner,
            managers,
            shadowing,
            detail,
            version,
        });
    }
    Ok(DetectionReport {
        findings,
        facts: lookup.facts,
        warnings: lookup.warnings,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    use crate::laravel::{Auth, StarterKit, Testing};
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        context: DetectionContext,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "shipslip-setup-tests-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let root = fs::canonicalize(root).unwrap();
            let mut context = DetectionContext::from_environment();
            context.environment = BTreeMap::from([
                ("HOME".into(), root.join("home").into_os_string()),
                ("PATH".into(), root.join("bin").into_os_string()),
            ]);
            context.macos = false;
            context.system_bin = root.join("system");
            context.xdg_system = root.join("etc-xdg");
            context.herd_app = root.join("Herd.app");
            context.brew_candidates = vec![root.join("brew/bin/brew")];
            fs::create_dir(root.join("home")).unwrap();
            fs::create_dir(root.join("bin")).unwrap();
            Self { root, context }
        }
        fn script(&self, path: &Path, body: &str) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fn tool(&self, name: &str, body: &str) {
            self.script(&self.root.join("bin").join(name), body);
        }
        fn brew(&self) {
            self.script(&self.context.brew_candidates[0], "exit 99");
        }
        fn complete(&self) {
            self.php(PHP_EXTENSIONS);
            self.tool("laravel", &format!("if [ \"$1\" = --version ]; then echo 'Laravel Installer 5.31.1'; else printf '%s\\n' {}; fi", crate::script::shell_quote(include_str!("fixtures/laravel-installer-5.31.1-help.txt"))));
            for name in ["composer", "node", "npm", "gh"] {
                self.tool(name, "echo 'fake 1.0'");
            }
            self.identity(true, true);
        }
        fn php(&self, extensions: &[&str]) {
            self.tool(
                "php",
                &format!(
                    "if [ \"$1\" = -m ]; then printf '%s\\n' {}; else echo 'PHP 1.0.0'; fi",
                    crate::script::shell_quote(&extensions.join("\n"))
                ),
            );
        }
        fn identity(&self, name: bool, email: bool) {
            self.tool(
                "git",
                &format!(
                    r#"case "$*" in
  '--version') echo 'git 1.0' ;;
  *'config --get user.name') {} ;;
  *'config --get user.email') {} ;;
  *) exit 99 ;;
esac"#,
                    if name { "echo Tester" } else { "exit 1" },
                    if email {
                        "echo tester@example.com"
                    } else {
                        "exit 1"
                    }
                ),
            );
        }
        fn options(&self, purposes: Vec<Purpose>) -> DetectionOptions {
            DetectionOptions {
                purposes,
                root: self.root.clone(),
                installer_options: None,
            }
        }
        async fn detect(&self, purposes: Vec<Purpose>) -> DetectionReport {
            detect(&self.context, &self.options(purposes))
                .await
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.root.join("brew"), fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn finding(report: &DetectionReport, id: RequirementId) -> &Finding {
        report
            .findings
            .iter()
            .find(|finding| finding.requirement.id == id)
            .unwrap()
    }
    fn actual_options(database: Database) -> InstallerOptions {
        InstallerOptions {
            starter_kit: StarterKit::React,
            auth: Auth::Laravel,
            database,
            testing: Testing::Pest,
            boost: false,
        }
    }

    #[test]
    fn finding_states_have_expected_readiness() {
        for (state, ready) in [
            (
                FindingState::Ok {
                    path: "/tool".into(),
                    version: "1".into(),
                },
                true,
            ),
            (FindingState::Missing, false),
            (
                FindingState::TooOld {
                    found: "1".into(),
                    need: "2".into(),
                },
                false,
            ),
            (broken(Path::new("/tool"), "failed"), false),
            (
                FindingState::OffPath {
                    path: "/tool".into(),
                    dir: "/bin".into(),
                },
                true,
            ),
            (
                FindingState::Unverified {
                    reason: "offline".into(),
                },
                true,
            ),
        ] {
            let mut finding = Finding {
                requirement: Requirement {
                    id: RequirementId::Tool("php"),
                    purposes: vec![Purpose::Create],
                    blocking: true,
                },
                state,
                location: None,
                owner: Owner::Unknown,
                managers: vec![],
                shadowing: None,
                detail: None,
                version: None,
            };
            assert_eq!(finding.ready(), ready);
            finding.requirement.blocking = false;
            assert!(finding.ready());
        }
    }

    #[test]
    fn path_wins_over_known_directories_table() {
        let mut fixture = Fixture::new();
        fixture.brew();
        let first = fixture.root.join("first");
        fs::create_dir(&first).unwrap();
        fixture.context.environment.insert(
            "PATH".into(),
            std::env::join_paths([first.clone(), fixture.root.join("bin")]).unwrap(),
        );
        for name in ["php", "composer", "laravel", "git", "node", "npm", "gh"] {
            fixture.tool(name, "exit 97");
            fixture.script(&fixture.root.join("brew/bin").join(name), "exit 98");
            let composer = composer_home(&fixture.context)
                .unwrap()
                .join("vendor/bin")
                .join(name);
            fixture.script(&composer, "exit 99");
            fixture.script(&first.join(name), "exit 96");
            fs::set_permissions(first.join(name), fs::Permissions::from_mode(0o644)).unwrap();
            let original = |dirs: &[PathBuf]| {
                dirs.iter()
                    .map(|dir| dir.join(name))
                    .find(|path| {
                        path.is_file()
                            && fs::metadata(path).unwrap().permissions().mode() & 0o111 != 0
                    })
                    .map(|path| fs::canonicalize(path).unwrap())
                    .unwrap()
            };
            let report = detect_tools(&fixture.context, &[name]);
            assert_eq!(
                report.tools[name].path,
                original(&fixture.context.path_dirs())
            );
            assert!(!report.tools[name].off_path);
            fs::remove_file(first.join(name)).unwrap();
            let target = fixture.root.join(format!("target-{name}"));
            fixture.script(&target, "exit 95");
            symlink(&target, first.join(name)).unwrap();
            let report = detect_tools(&fixture.context, &[name]);
            assert_eq!(
                report.tools[name].path,
                original(&fixture.context.path_dirs())
            );
            assert_eq!(report.tools[name].path, target);
        }
    }

    #[test]
    fn known_directories_follow_path_in_order() {
        let fixture = Fixture::new();
        fixture.brew();
        let composer = composer_home(&fixture.context)
            .unwrap()
            .join("vendor/bin/laravel");
        fixture.script(&composer, "exit 1");
        fixture.script(&fixture.root.join("brew/bin/laravel"), "exit 2");
        let report = detect_tools(&fixture.context, &["laravel"]);
        assert_eq!(
            report.tools["laravel"].path,
            fixture.root.join("brew/bin/laravel")
        );
        assert!(report.tools["laravel"].off_path);
        fs::remove_file(fixture.root.join("brew/bin/laravel")).unwrap();
        assert_eq!(
            detect_tools(&fixture.context, &["laravel"]).tools["laravel"].path,
            composer
        );
    }

    #[test]
    fn homebrew_on_path_symlink_uses_installation_prefix() {
        let fixture = Fixture::new();
        fixture.brew();
        symlink(
            &fixture.context.brew_candidates[0],
            fixture.root.join("bin/brew"),
        )
        .unwrap();
        fixture.script(&fixture.root.join("brew/bin/node"), "exit 99");
        let lookup = detect_tools(&fixture.context, &["node"]);
        assert_eq!(
            lookup.facts.homebrew.as_ref().unwrap().prefix,
            fixture.root.join("brew")
        );
        assert_eq!(
            lookup.tools["node"].path,
            fixture.root.join("brew/bin/node")
        );
        fs::remove_file(fixture.root.join("bin/brew")).unwrap();
        fixture.script(&fixture.root.join("intel/Homebrew/bin/brew"), "exit 99");
        symlink(
            fixture.root.join("intel/Homebrew/bin/brew"),
            fixture.root.join("bin/brew"),
        )
        .unwrap();
        assert_eq!(
            detect_tools(&fixture.context, &[])
                .facts
                .homebrew
                .unwrap()
                .prefix,
            fixture.root.join("intel")
        );
    }

    #[test]
    fn fnm_custom_and_legacy_directories_are_detected() {
        let mut fixture = Fixture::new();
        fixture.context.environment.insert(
            "XDG_DATA_HOME".into(),
            fixture.root.join("data").into_os_string(),
        );
        let node = fixture.root.join("data/fnm/node-versions/node");
        fixture.script(&node, "exit 99");
        let facts = detect_tools(&fixture.context, &[]).facts;
        assert!(facts.providers["node"].contains(&Manager::Fnm));
        assert_eq!(classify_owner(&node, &fixture.context, &facts), Owner::Fnm);
        fs::remove_dir_all(fixture.root.join("data")).unwrap();
        fixture.context.macos = true;
        let node = fixture
            .root
            .join("home/Library/Application Support/fnm/node-versions/node");
        fixture.script(&node, "exit 99");
        let facts = detect_tools(&fixture.context, &[]).facts;
        assert!(facts.providers["npm"].contains(&Manager::Fnm));
        assert_eq!(classify_owner(&node, &fixture.context, &facts), Owner::Fnm);
    }

    #[test]
    fn composer_xdg_eligibility_matches_factory() {
        let mut fixture = Fixture::new();
        assert!(!composer_uses_xdg(&fixture.context));
        fixture
            .context
            .environment
            .insert("XDG_DATA_HOME".into(), "".into());
        assert!(composer_uses_xdg(&fixture.context));
        fixture
            .context
            .environment
            .remove(std::ffi::OsStr::new("XDG_DATA_HOME"));
        fs::create_dir(&fixture.context.xdg_system).unwrap();
        assert!(composer_uses_xdg(&fixture.context));
    }

    #[test]
    fn composer_home_selection_matches_factory() {
        let mut fixture = Fixture::new();
        let legacy = fixture.root.join("home/.composer");
        assert_eq!(composer_home(&fixture.context), Some(legacy.clone()));
        let xdg = fixture.root.join("xdg");
        fixture
            .context
            .environment
            .insert("XDG_CONFIG_HOME".into(), xdg.clone().into_os_string());
        assert_eq!(composer_home(&fixture.context), Some(xdg.join("composer")));
        fs::create_dir_all(&legacy).unwrap();
        assert_eq!(composer_home(&fixture.context), Some(legacy));
        fs::create_dir_all(xdg.join("composer")).unwrap();
        assert_eq!(composer_home(&fixture.context), Some(xdg.join("composer")));
        let explicit = fixture.root.join("not-created");
        fixture
            .context
            .environment
            .insert("COMPOSER_HOME".into(), explicit.clone().into_os_string());
        assert_eq!(composer_home(&fixture.context), Some(explicit));
        fixture
            .context
            .environment
            .remove(std::ffi::OsStr::new("COMPOSER_HOME"));
        fixture
            .context
            .environment
            .remove(std::ffi::OsStr::new("XDG_CONFIG_HOME"));
        fs::create_dir(&fixture.context.xdg_system).unwrap();
        fs::create_dir_all(fixture.root.join("home/.config/composer")).unwrap();
        assert_eq!(
            composer_home(&fixture.context),
            Some(fixture.root.join("home/.config/composer"))
        );
    }

    #[test]
    fn composer_discovery_never_executes_composer() {
        let fixture = Fixture::new();
        fixture.tool(
            "composer",
            &format!(
                "printf called > {}",
                crate::script::shell_quote(&fixture.root.join("called").display().to_string())
            ),
        );
        let _ = detect_tools(&fixture.context, &["composer", "laravel"]);
        assert!(!fixture.root.join("called").exists());
    }

    #[test]
    fn composer_selected_home_does_not_fall_through() {
        let mut fixture = Fixture::new();
        let selected = fixture.root.join("selected");
        fs::create_dir(&selected).unwrap();
        fixture
            .context
            .environment
            .insert("COMPOSER_HOME".into(), selected.clone().into_os_string());
        fixture.script(
            &fixture.root.join("home/.composer/vendor/bin/laravel"),
            "echo later",
        );
        let report = detect_tools(&fixture.context, &["laravel"]);
        assert!(report.failures.contains_key("laravel"));
        assert_eq!(report.facts.composer_bin, Some(selected.join("vendor/bin")));
    }

    #[test]
    fn composer_custom_bin_dir_is_resolved_relative_to_home() {
        let fixture = Fixture::new();
        let home = fixture.root.join("home");
        for bin in [
            "custom/bin".to_string(),
            fixture.root.join("absolute bin").display().to_string(),
        ] {
            fs::write(
                home.join("config.json"),
                serde_json::json!({"config": {"bin-dir": bin}}).to_string(),
            )
            .unwrap();
            let (dir, warning) = composer_bin_dir(&home);
            assert_eq!(dir, home.join(&bin));
            assert!(warning.is_none());
        }
    }

    #[test]
    fn composer_bad_config_warns_and_uses_same_home_default() {
        let fixture = Fixture::new();
        let home = fixture.root.join("home");
        let config = home.join("config.json");
        fs::write(&config, "invalid json").unwrap();
        let (dir, warning) = composer_bin_dir(&home);
        assert_eq!(dir, home.join("vendor/bin"));
        assert_eq!(warning.unwrap().path, config);
        for invalid in ["[]", r#"{"config":false}"#, r#"{"config":{"bin-dir":23}}"#] {
            fs::write(&config, invalid).unwrap();
            let (dir, warning) = composer_bin_dir(&home);
            assert_eq!(dir, home.join("vendor/bin"));
            assert_eq!(warning.unwrap().path, config);
        }
        fs::remove_file(&config).unwrap();
        fs::create_dir(&config).unwrap();
        let (dir, warning) = composer_bin_dir(&home);
        assert_eq!(dir, home.join("vendor/bin"));
        assert_eq!(warning.unwrap().path, config);
    }

    #[test]
    fn owners_are_classified_from_canonical_paths_table() {
        let fixture = Fixture::new();
        fixture.brew();
        let facts = detect_tools(&fixture.context, &[]).facts;
        for (path, owner) in [
            (fixture.root.join("brew/Cellar/php/php"), Owner::Homebrew),
            (fixture.context.system_bin.join("git"), Owner::System),
            (fixture.context.herd_app.join("Contents/php"), Owner::Herd),
            (
                fixture
                    .root
                    .join("home/Library/Application Support/Herd/bin/php"),
                Owner::Herd,
            ),
            (
                fixture.root.join("home/.config/herd-lite/bin/php"),
                Owner::HerdLite,
            ),
            (fixture.root.join("home/.asdf/shims/php"), Owner::Asdf),
            (
                fixture.root.join("home/.local/share/mise/installs/php/php"),
                Owner::Mise,
            ),
            (
                fixture.root.join("home/.nvm/versions/node/node"),
                Owner::Nvm,
            ),
            (
                fixture
                    .root
                    .join("home/.local/share/fnm/node-versions/node"),
                Owner::Fnm,
            ),
            (
                fixture.root.join("home/.composer/vendor/bin/laravel"),
                Owner::ComposerGlobal,
            ),
            (fixture.root.join("other/php"), Owner::Unknown),
        ] {
            fixture.script(&path, "exit 0");
            let entry = fixture.root.join("bin/tool");
            symlink(&path, &entry).unwrap();
            let report = detect_tools(&fixture.context, &["tool"]);
            assert_eq!(
                classify_owner(&report.tools["tool"].path, &fixture.context, &facts),
                owner
            );
            fs::remove_file(entry).unwrap();
        }
    }

    #[test]
    fn managers_are_detected_without_provided_tools() {
        let mut fixture = Fixture::new();
        for path in [
            "home/.nvm",
            "home/.local/share/fnm",
            "home/.asdf",
            "home/.local/share/mise",
            "home/.config/herd-lite",
            "Herd.app",
        ] {
            fs::create_dir_all(fixture.root.join(path)).unwrap();
        }
        let report = detect_tools(&fixture.context, &["node"]);
        assert_eq!(report.facts.managers.len(), 6);
        assert!(report.failures.contains_key("node"));
        fixture.context.environment.insert(
            "NVM_DIR".into(),
            fixture.root.join("custom-nvm").into_os_string(),
        );
        fs::remove_dir(fixture.root.join("home/.nvm")).unwrap();
        fs::create_dir(fixture.root.join("custom-nvm")).unwrap();
        assert!(detect_tools(&fixture.context, &[])
            .facts
            .managers
            .contains(&Manager::Nvm));
    }

    #[test]
    fn manager_provider_map_is_tool_specific() {
        let fixture = Fixture::new();
        for path in [
            "home/.asdf/plugins/nodejs",
            "home/.asdf/plugins/php",
            "home/.local/share/mise/installs/nodejs",
            "home/.local/share/mise/installs/php",
            "Herd.app",
        ] {
            fs::create_dir_all(fixture.root.join(path)).unwrap();
        }
        let facts = detect_tools(&fixture.context, &[]).facts;
        for tool in ["node", "npm", "php"] {
            assert!(facts.providers[tool].contains(&Manager::Asdf));
            assert!(facts.providers[tool].contains(&Manager::Mise));
        }
        for tool in ["composer", "laravel"] {
            assert!(!facts.providers[tool].contains(&Manager::Asdf));
            assert!(!facts.providers[tool].contains(&Manager::Mise));
            fs::create_dir_all(fixture.root.join("home/.asdf/plugins").join(tool)).unwrap();
            fs::create_dir_all(
                fixture
                    .root
                    .join("home/.local/share/mise/installs")
                    .join(tool),
            )
            .unwrap();
        }
        assert!(!facts.providers["node"].contains(&Manager::Herd));
        fs::create_dir_all(
            fixture
                .root
                .join("home/Library/Application Support/Herd/config/nvm"),
        )
        .unwrap();
        fs::rename(
            fixture.root.join("home/.local/share/mise/installs/nodejs"),
            fixture.root.join("home/.local/share/mise/installs/node"),
        )
        .unwrap();
        let facts = detect_tools(&fixture.context, &[]).facts;
        assert!(facts.providers["node"].contains(&Manager::Herd));
        for tool in ["composer", "laravel", "node", "npm"] {
            assert!(facts.providers[tool].contains(&Manager::Mise));
            assert!(facts.providers[tool].contains(&Manager::Asdf));
        }
    }

    #[tokio::test]
    async fn php_new_does_not_claim_missing_node() {
        let fixture = Fixture::new();
        fixture.brew();
        fs::create_dir_all(fixture.root.join("home/.config/herd-lite")).unwrap();
        let report = fixture.detect(vec![Purpose::Create]).await;
        assert_eq!(
            finding(&report, RequirementId::Tool("node")).state,
            FindingState::Missing
        );
        for tool in ["node", "npm"] {
            assert!(!report.facts.providers[tool].contains(&Manager::HerdLite));
        }
        assert!(report.facts.homebrew.unwrap().writable);
    }

    #[test]
    fn homebrew_prefix_writability_is_detected_without_writes() {
        let fixture = Fixture::new();
        fixture.brew();
        assert!(
            detect_tools(&fixture.context, &[])
                .facts
                .homebrew
                .unwrap()
                .writable
        );
        fs::set_permissions(fixture.root.join("brew"), fs::Permissions::from_mode(0o555)).unwrap();
        let facts = detect_tools(&fixture.context, &[]).facts;
        assert!(!facts.homebrew.unwrap().writable);
        assert_eq!(fs::read_dir(fixture.root.join("brew")).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn detection_reports_every_requirement_after_failures() {
        let fixture = Fixture::new();
        fixture.tool("node", "echo still-probed");
        fixture.tool("gh", "echo gh-ready");
        let report = fixture
            .detect(vec![Purpose::Create, Purpose::Publish])
            .await;
        assert_eq!(
            report.findings.len(),
            requirements(&[Purpose::Create, Purpose::Publish], Database::Sqlite).len()
        );
        assert!(matches!(
            finding(&report, RequirementId::Tool("node")).state,
            FindingState::Ok { .. }
        ));
        assert!(matches!(
            finding(&report, RequirementId::GithubAuth).state,
            FindingState::Ok { .. }
        ));
        assert!(!report.ready());
    }

    #[tokio::test]
    async fn broken_path_tool_does_not_fall_back() {
        let fixture = Fixture::new();
        fixture.brew();
        fixture.tool("node", "echo broken-node >&2; exit 1");
        fixture.script(&fixture.root.join("brew/bin/node"), "echo working-node");
        let report = fixture.detect(vec![Purpose::Create]).await;
        let node = finding(&report, RequirementId::Tool("node"));
        assert!(
            matches!(&node.state, FindingState::Broken { path, error } if path == &fixture.root.join("bin/node") && error == "broken-node")
        );
        assert_eq!(node.shadowing, Some(fixture.root.join("bin/node")));
    }

    #[tokio::test]
    async fn failing_path_tool_records_shadowing() {
        let fixture = Fixture::new();
        fixture.brew();
        fixture.tool("node", "exit 1");
        let report = fixture.detect(vec![Purpose::Create]).await;
        assert_eq!(
            finding(&report, RequirementId::Tool("node")).shadowing,
            Some(fixture.root.join("bin/node"))
        );
    }

    #[tokio::test]
    async fn off_path_tool_is_usable() {
        let fixture = Fixture::new();
        fixture.complete();
        let bin = composer_home(&fixture.context).unwrap().join("vendor/bin");
        fs::create_dir_all(&bin).unwrap();
        fs::rename(fixture.root.join("bin/laravel"), bin.join("laravel")).unwrap();
        let report = fixture.detect(vec![Purpose::Create]).await;
        assert!(report.ready());
        assert!(
            matches!(&finding(&report, RequirementId::Tool("laravel")).state, FindingState::OffPath { path, dir } if path == &bin.join("laravel") && dir == &bin)
        );
        assert_eq!(
            detect_tools(&fixture.context, CREATE_TOOLS)
                .creation_tools()
                .unwrap()
                .laravel,
            bin.join("laravel")
        );
    }

    #[tokio::test]
    async fn missing_clt_does_not_execute_git_shim() {
        let mut fixture = Fixture::new();
        fixture.context.macos = true;
        fixture.context.environment.insert(
            "PATH".into(),
            fixture.context.system_bin.clone().into_os_string(),
        );
        fixture.script(
            &fixture.context.system_bin.join("xcode-select"),
            "[ \"$*\" = -p ]; exit 1",
        );
        fixture.script(
            &fixture.context.system_bin.join("git"),
            &format!(
                "echo called > {}",
                crate::script::shell_quote(&fixture.root.join("called").display().to_string())
            ),
        );
        let report = fixture
            .detect(vec![Purpose::Create, Purpose::Publish])
            .await;
        let git = finding(&report, RequirementId::Tool("git"));
        assert_eq!(git.state, FindingState::Missing);
        assert!(git
            .detail
            .as_ref()
            .unwrap()
            .contains("Command Line Tools not installed"));
        assert!(!fixture.root.join("called").exists());
    }

    #[tokio::test]
    async fn available_clt_allows_git_probe() {
        let mut fixture = Fixture::new();
        fixture.context.macos = true;
        fixture.context.environment.insert(
            "PATH".into(),
            fixture.context.system_bin.clone().into_os_string(),
        );
        fixture.script(
            &fixture.context.system_bin.join("xcode-select"),
            "[ \"$*\" = -p ]; exit 0",
        );
        fixture.script(&fixture.context.system_bin.join("git"), "echo git-ready");
        let report = fixture.detect(vec![Purpose::Publish]).await;
        assert!(matches!(
            finding(&report, RequirementId::Tool("git")).state,
            FindingState::Ok { .. }
        ));
    }

    #[tokio::test]
    async fn failed_version_probe_is_broken() {
        let fixture = Fixture::new();
        fixture.tool("node", "echo version-failed >&2; exit 1");
        fixture.tool("npm", "echo empty >&2; exit 1");
        let report = fixture.detect(vec![Purpose::Create]).await;
        for name in ["node", "npm"] {
            assert!(
                matches!(&finding(&report, RequirementId::Tool(name)).state, FindingState::Broken { path, .. } if path == &fixture.root.join("bin").join(name))
            );
        }
        fixture.script(&fixture.root.join("bin/node"), "");
        assert!(matches!(
            finding(
                &fixture.detect(vec![Purpose::Create]).await,
                RequirementId::Tool("node")
            )
            .state,
            FindingState::Broken { .. }
        ));
    }

    #[tokio::test]
    async fn php_and_node_have_no_minimum_version() {
        let fixture = Fixture::new();
        fixture.complete();
        let report = fixture.detect(vec![Purpose::Create]).await;
        assert!(report.ready());
        for name in ["php", "node"] {
            assert!(matches!(
                finding(&report, RequirementId::Tool(name)).state,
                FindingState::Ok { .. }
            ));
        }
    }

    #[tokio::test]
    async fn installer_minimum_is_enforced() {
        let fixture = Fixture::new();
        fixture.complete();
        fixture.tool("laravel", "echo 'Laravel Installer 5.30.0'");
        let report = fixture.detect(vec![Purpose::Create]).await;
        assert!(
            matches!(&finding(&report, RequirementId::Tool("laravel")).state, FindingState::TooOld { found, need } if found == "5.30.0" && need == "5.31.1")
        );
        fixture.complete();
        assert!(fixture.detect(vec![Purpose::Create]).await.ready());
    }

    #[test]
    fn doctor_requires_installer_flag_union() {
        let help = include_str!("fixtures/laravel-installer-5.31.1-help.txt");
        assert_eq!(
            probe_installer("Laravel Installer 5.31.1", help, None).unwrap(),
            "5.31.1"
        );
        for flag in [
            "--react",
            "--vue",
            "--svelte",
            "--livewire",
            "--no-authentication",
            "--database",
            "--no-interaction",
            "--npm",
            "--pest",
            "--phpunit",
            "--boost",
            "--no-boost",
        ] {
            assert!(
                probe_installer(
                    "Laravel Installer 5.31.1",
                    &help.replace(flag, "--removed"),
                    None
                )
                .is_err(),
                "{flag}"
            );
        }
    }

    #[test]
    fn installer_actual_options_require_only_selected_flags() {
        let help = include_str!("fixtures/laravel-installer-5.31.1-help.txt")
            .replace("--vue", "--removed");
        assert!(probe_installer("Laravel Installer 5.31.1", &help, None).is_err());
        assert!(probe_installer(
            "Laravel Installer 5.31.1",
            &help,
            Some(&actual_options(Database::Sqlite))
        )
        .is_ok());
    }

    #[tokio::test]
    async fn php_blocking_extensions_are_complete() {
        let fixture = Fixture::new();
        fixture.complete();
        for missing in PHP_EXTENSIONS {
            let extensions = PHP_EXTENSIONS
                .iter()
                .copied()
                .filter(|extension| extension != missing)
                .collect::<Vec<_>>();
            fixture.php(&extensions);
            let report = fixture.detect(vec![Purpose::Create]).await;
            assert!(!report.ready(), "{missing}");
            assert!(matches!(
                finding(&report, RequirementId::Extension(missing)).state,
                FindingState::Broken { .. }
            ));
        }
    }

    #[tokio::test]
    async fn pdo_sqlite_blocks_for_every_database() {
        let fixture = Fixture::new();
        fixture.complete();
        fixture.php(
            &PHP_EXTENSIONS
                .iter()
                .copied()
                .filter(|extension| *extension != "pdo_sqlite")
                .collect::<Vec<_>>(),
        );
        for database in [
            Database::Sqlite,
            Database::Mysql,
            Database::Mariadb,
            Database::Pgsql,
            Database::Sqlsrv,
        ] {
            let mut options = fixture.options(vec![Purpose::Create]);
            options.installer_options = Some(actual_options(database));
            let report = detect(&fixture.context, &options).await.unwrap();
            assert!(!report.ready());
            assert!(matches!(
                finding(&report, RequirementId::Extension("pdo_sqlite")).state,
                FindingState::Broken { .. }
            ));
        }
    }

    #[tokio::test]
    async fn selected_database_driver_only_warns_table() {
        let fixture = Fixture::new();
        fixture.complete();
        for (database, driver) in [
            (Database::Mysql, "pdo_mysql"),
            (Database::Mariadb, "pdo_mysql"),
            (Database::Pgsql, "pdo_pgsql"),
            (Database::Sqlsrv, "pdo_sqlsrv"),
        ] {
            let mut options = fixture.options(vec![Purpose::Create]);
            options.installer_options = Some(actual_options(database));
            let report = detect(&fixture.context, &options).await.unwrap();
            assert!(report.ready());
            let state = &finding(&report, RequirementId::Extension(driver)).state;
            assert!(matches!(state, FindingState::Unverified { .. }));
            if driver == "pdo_sqlsrv" {
                assert!(
                    matches!(state, FindingState::Unverified { reason } if reason.contains("learn.microsoft.com"))
                );
            }
        }
    }

    #[tokio::test]
    async fn git_name_and_email_are_detected_separately() {
        let fixture = Fixture::new();
        fixture.complete();
        for name in [false, true] {
            for email in [false, true] {
                fixture.identity(name, email);
                let report = fixture.detect(vec![Purpose::Create]).await;
                assert_eq!(
                    finding(&report, RequirementId::Identity("user.name")).ready(),
                    name
                );
                assert_eq!(
                    finding(&report, RequirementId::Identity("user.email")).ready(),
                    email
                );
                assert_eq!(report.ready(), name && email);
            }
        }
    }

    #[tokio::test]
    async fn publish_auth_uses_exact_argv_and_environment() {
        if std::env::var("SHIPSLIP_SETUP_AUTH_TEST").as_deref() != Ok("child") {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "setup::tests::publish_auth_uses_exact_argv_and_environment",
                    "--nocapture",
                ])
                .env("SHIPSLIP_SETUP_AUTH_TEST", "child")
                .env("GH_HOST", "wrong.example")
                .env("GH_PROMPT_DISABLED", "0")
                .env("SHIPSLIP_GITHUB_TOKEN", "ambient-private-token")
                .env("GITHUB_ENTERPRISE_TOKEN", "ambient-enterprise-token");
            for name in crate::git::REPOSITORY_ENV {
                child.env(name, "unrelated-repository");
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }
        let mut fixture = Fixture::new();
        fixture.complete();
        for (name, value) in [
            ("GH_TOKEN", "first"),
            ("GITHUB_TOKEN", "second"),
            ("GH_ENTERPRISE_TOKEN", "third"),
        ] {
            fixture
                .context
                .environment
                .insert(name.into(), value.into());
        }
        fixture.tool("gh", r#"if [ "$1" = --version ]; then echo gh-test; exit 0; fi
[ "$*" = 'auth status --hostname github.com --active' ]
[ "$GH_HOST" = github.com ] && [ "$GH_PROMPT_DISABLED" = 1 ]
[ "$GH_TOKEN" = first ] && [ "$GITHUB_TOKEN" = second ] && [ "$GH_ENTERPRISE_TOKEN" = third ]
[ -z "${GIT_DIR+x}" ] && [ -z "${GIT_WORK_TREE+x}" ] && [ -z "${GIT_INDEX_FILE+x}" ] && [ -z "${GIT_COMMON_DIR+x}" ] && [ -z "${GIT_OBJECT_DIRECTORY+x}" ] && [ -z "${GIT_ALTERNATE_OBJECT_DIRECTORIES+x}" ] && [ -z "${GIT_NAMESPACE+x}" ]
[ -z "${SHIPSLIP_GITHUB_TOKEN+x}" ] && [ -z "${GITHUB_ENTERPRISE_TOKEN+x}" ]
if IFS= read -r input; then exit 92; fi"#);
        let report = fixture.detect(vec![Purpose::Publish]).await;
        assert!(report.ready(), "{report:?}");
    }

    #[tokio::test]
    async fn publish_auth_failure_reports_redacted_gh_message() {
        let mut fixture = Fixture::new();
        fixture.complete();
        fixture
            .context
            .environment
            .insert("GH_TOKEN".into(), "invalid-token".into());
        fixture.tool("gh", "if [ \"$1\" = --version ]; then echo gh-test; exit 0; fi\necho \"rejected $GH_TOKEN\" >&2; exit 1");
        let report = fixture.detect(vec![Purpose::Publish]).await;
        assert!(!report.ready());
        let auth = finding(&report, RequirementId::GithubAuth);
        assert_eq!(auth.state, FindingState::Missing);
        assert!(auth
            .detail
            .as_ref()
            .unwrap()
            .contains("rejected [REDACTED]"));
        assert!(!format!("{report:?}").contains("invalid-token"));
    }

    #[tokio::test]
    async fn publish_auth_timeout_warns_and_kills_child() {
        let fixture = Fixture::new();
        fixture.complete();
        fixture.tool("gh", &format!("if [ \"$1\" = --version ]; then echo gh-test; exit 0; fi\necho $$ > {}\nexec /bin/sleep 30", crate::script::shell_quote(&fixture.root.join("pid").display().to_string())));
        let started = std::time::Instant::now();
        let report = fixture.detect(vec![Purpose::Publish]).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert!(report.ready());
        assert!(
            matches!(&finding(&report, RequirementId::GithubAuth).state, FindingState::Unverified { reason } if reason == "could not reach GitHub within 10 s")
        );
        let pid: i32 = fs::read_to_string(fixture.root.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                // SAFETY: signal zero checks this fixture child's existence without sending a signal.
                if unsafe { libc::kill(pid, 0) } != 0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed-out gh child must terminate");
    }

    #[test]
    fn creation_lookup_report_converts_to_tools() {
        let fixture = Fixture::new();
        fixture.complete();
        let lookup = detect_tools(&fixture.context, CREATE_TOOLS);
        let tools = lookup.creation_tools().unwrap();
        assert_eq!(tools.php, lookup.tools["php"].path);
        assert_eq!(tools.composer, lookup.tools["composer"].path);
        assert_eq!(tools.laravel, lookup.tools["laravel"].path);
        assert_eq!(tools.node, lookup.tools["node"].path);
        assert_eq!(tools.npm, lookup.tools["npm"].path);
        assert_eq!(tools.git, lookup.tools["git"].path);
        fs::remove_file(&tools.php).unwrap();
        fs::remove_file(&tools.node).unwrap();
        let error = detect_tools(&fixture.context, CREATE_TOOLS)
            .creation_tools()
            .unwrap_err()
            .to_string();
        assert!(error.contains("php: not found") && error.contains("node: not found"));
    }

    #[test]
    fn publish_lookup_report_converts_to_tools() {
        let fixture = Fixture::new();
        fixture.brew();
        for name in ["git", "gh"] {
            fixture.script(&fixture.root.join("brew/bin").join(name), "exit 99");
        }
        let tools = detect_tools(&fixture.context, &["git", "gh"])
            .publish_tools()
            .unwrap();
        assert_eq!(tools.git.binary, fixture.root.join("brew/bin/git"));
        assert_eq!(tools.gh, fixture.root.join("brew/bin/gh"));
    }

    #[test]
    fn lookup_adapters_do_not_execute_tools() {
        let fixture = Fixture::new();
        for name in ["php", "composer", "laravel", "git", "node", "npm", "gh"] {
            fixture.tool(
                name,
                &format!(
                    "echo called > {}; exit 99",
                    crate::script::shell_quote(&fixture.root.join("called").display().to_string())
                ),
            );
        }
        assert!(detect_tools(&fixture.context, CREATE_TOOLS)
            .creation_tools()
            .is_ok());
        assert!(detect_tools(&fixture.context, &["git", "gh"])
            .publish_tools()
            .is_ok());
        assert!(!fixture.root.join("called").exists());
    }

    #[tokio::test]
    async fn ordinary_probes_pin_tools_scrub_tokens_and_close_stdin() {
        let mut fixture = Fixture::new();
        fixture.complete();
        for name in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "SHIPSLIP_GITHUB_TOKEN",
        ] {
            fixture
                .context
                .environment
                .insert(name.into(), "test-token".into());
        }
        fixture.tool("node", r#"[ -z "${GH_TOKEN+x}" ] && [ -z "${GITHUB_TOKEN+x}" ] && [ -z "${GH_ENTERPRISE_TOKEN+x}" ] && [ -z "${GITHUB_ENTERPRISE_TOKEN+x}" ] && [ -z "${SHIPSLIP_GITHUB_TOKEN+x}" ]
if IFS= read -r input; then exit 92; fi
echo node-test"#);
        fixture.brew();
        fixture.script(
            &fixture.root.join("brew/bin/composer"),
            "[ \"$(php --version)\" = 'PHP 1.0.0' ]; echo composer-pinned",
        );
        fs::remove_file(fixture.root.join("bin/composer")).unwrap();
        let report = fixture.detect(vec![Purpose::Create]).await;
        assert!(report.ready(), "{report:?}");
    }
    #[test]
    fn requirements_cover_create_and_publish_only() {
        let create = requirements(&[Purpose::Create], Database::Sqlite);
        assert_eq!(create.len(), CREATE_TOOLS.len() + PHP_EXTENSIONS.len() + 2);
        let publish = requirements(&[Purpose::Publish], Database::Sqlite);
        assert_eq!(
            publish.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![
                RequirementId::Tool("git"),
                RequirementId::Tool("gh"),
                RequirementId::GithubAuth
            ]
        );
        let both = requirements(&[Purpose::Create, Purpose::Publish], Database::Sqlite);
        assert_eq!(
            both.iter()
                .filter(|r| r.id == RequirementId::Tool("git"))
                .count(),
            1
        );
        for db in [
            Database::Mysql,
            Database::Mariadb,
            Database::Pgsql,
            Database::Sqlsrv,
        ] {
            let requirements = requirements(&[Purpose::Create], db);
            assert!(requirements
                .iter()
                .any(|r| r.id == RequirementId::Extension("pdo_sqlite") && r.blocking));
            assert_eq!(requirements.iter().filter(|r| !r.blocking).count(), 1);
        }
    }
}
