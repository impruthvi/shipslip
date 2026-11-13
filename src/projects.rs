//! The checkouts added in the app, and what a folder holds before it is
//! added. The list keeps each checkout's path and when it was added, nothing
//! else; everything the app shows about a checkout is read from its folder,
//! its approvals and its receipts.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::config::{approve_trust, ConfigError, InitAnswers, InitPlan, LoadedConfig};
use crate::private_file::write_atomic;
use crate::receipt::{Overview, Problem, ProblemKind};

const VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ProjectsError {
    #[error("`{0}` is not inside a git repository")]
    NotGit(PathBuf),
    #[error("could not access `{path}`: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid projects list `{path}`: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("unsupported projects list version {0}")]
    Version(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkout {
    pub path: PathBuf,
    pub added_at_ms: u128,
}

#[derive(Serialize, Deserialize)]
struct List {
    version: u32,
    checkouts: Vec<Checkout>,
}

/// The projects list lives beside the approvals file.
pub fn path_beside(trust_path: &Path) -> PathBuf {
    trust_path.with_file_name("projects.json")
}

/// The added checkouts, oldest first. A missing list is empty.
pub fn list(path: &Path) -> Result<Vec<Checkout>, ProjectsError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ProjectsError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let list: List = serde_json::from_slice(&bytes).map_err(|source| ProjectsError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    if list.version != VERSION {
        return Err(ProjectsError::Version(list.version));
    }
    Ok(list.checkouts)
}

/// Adds the git checkout `folder` belongs to and returns its path. Adding a
/// listed checkout again keeps its first date.
pub fn add(path: &Path, folder: &Path, now_ms: u128) -> Result<PathBuf, ProjectsError> {
    let checkout = checkout_root(folder)?;
    let _lock = lock(path)?;
    let mut checkouts = list(path)?;
    if checkouts.iter().any(|listed| listed.path == checkout) {
        return Ok(checkout);
    }
    checkouts.push(Checkout {
        path: checkout.clone(),
        added_at_ms: now_ms,
    });
    save(path, checkouts)?;
    Ok(checkout)
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error("this Mac has receipts or approvals for `{0}`, so it stays")]
    Recorded(PathBuf),
    /// An unread file could name the checkout, so nothing was removed.
    #[error("could not read what this Mac recorded for `{checkout}`: {reason}")]
    Unreadable { checkout: PathBuf, reason: String },
    #[error(transparent)]
    List(#[from] ProjectsError),
}

/// The checkouts only the projects list names: no receipt or approval does.
/// Empty while an approval, receipt or receipts folder can't be read, since
/// that file could name any checkout.
pub fn nothing_recorded(overview: &Overview) -> BTreeSet<String> {
    if unread(overview).is_some() {
        return BTreeSet::new();
    }
    overview
        .repos
        .iter()
        .filter(|row| row.envs.is_empty())
        .map(|row| row.repo_root.raw().to_string())
        .collect()
}

fn unread(overview: &Overview) -> Option<&Problem> {
    overview
        .problems
        .iter()
        .find(|problem| problem.kind != ProblemKind::Projects)
}

/// Drops `checkout` from the list when this Mac recorded nothing for it (see
/// [`nothing_recorded`]). Nothing in its folder is touched. A checkout not
/// listed is left as it is.
pub fn remove(trust_path: &Path, receipts_root: &Path, checkout: &Path) -> Result<(), RemoveError> {
    let path = path_beside(trust_path);
    let _lock = lock(&path)?;
    let mut checkouts = list(&path)?;
    let Some(at) = checkouts.iter().position(|listed| listed.path == checkout) else {
        return Ok(());
    };
    let unreadable = |reason: String| RemoveError::Unreadable {
        checkout: checkout.to_path_buf(),
        reason,
    };
    let overview = crate::receipt::overview(receipts_root, trust_path)
        .map_err(|error| unreadable(error.to_string()))?;
    if let Some(problem) = unread(&overview) {
        return Err(unreadable(problem.message.raw().to_string()));
    }
    if !nothing_recorded(&overview).contains(checkout.to_string_lossy().as_ref()) {
        return Err(RemoveError::Recorded(checkout.to_path_buf()));
    }
    checkouts.remove(at);
    save(&path, checkouts)?;
    Ok(())
}

/// Blocks until this caller holds `projects.json.lock` beside the list, so
/// one read-change-save can't drop another's change. Released on drop.
fn lock(path: &Path) -> Result<File, ProjectsError> {
    create_parent(path)?;
    let lock_path = path.with_extension("json.lock");
    let io_error = |source| ProjectsError::Io {
        path: lock_path.clone(),
        source,
    };
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(&lock_path).map_err(io_error)?;
    file.lock_exclusive().map_err(io_error)?;
    Ok(file)
}

fn create_parent(path: &Path) -> Result<(), ProjectsError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|source| ProjectsError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|source| {
            ProjectsError::Io {
                path: parent.to_path_buf(),
                source,
            }
        })?;
    }
    Ok(())
}

fn save(path: &Path, checkouts: Vec<Checkout>) -> Result<(), ProjectsError> {
    create_parent(path)?;
    let mut bytes = serde_json::to_vec_pretty(&List {
        version: VERSION,
        checkouts,
    })
    .map_err(|error| ProjectsError::Io {
        path: path.to_path_buf(),
        source: io::Error::other(error),
    })?;
    bytes.push(b'\n');
    write_atomic(path, &bytes).map_err(|source| ProjectsError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// What a folder holds, read before it is added.
#[derive(Debug)]
pub enum Folder {
    /// Not inside a git repository, so nothing can be deployed from it.
    NotGit,
    /// A git checkout without a `.shipslip.toml`: the config `init` would
    /// write there.
    NewConfig(InitPlan),
    /// A git checkout with a config.
    Config(LoadedConfig),
    /// A git checkout whose config can't be read or is invalid.
    Unreadable {
        checkout: PathBuf,
        error: ConfigError,
    },
}

/// Reads the config at the root of the git checkout `folder` belongs to.
pub fn inspect(folder: &Path) -> Folder {
    let Ok(checkout) = checkout_root(folder) else {
        return Folder::NotGit;
    };
    if !checkout.join(".shipslip.toml").exists() {
        return match InitPlan::new(&checkout) {
            Ok(plan) => Folder::NewConfig(plan),
            Err(error) => Folder::Unreadable { checkout, error },
        };
    }
    match LoadedConfig::load(&checkout, None) {
        Ok(config) => Folder::Config(config),
        Err(error) => Folder::Unreadable { checkout, error },
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AddConfigError {
    /// Nothing was written or approved.
    #[error(transparent)]
    List(#[from] ProjectsError),
    /// The checkout is listed; no config was written.
    #[error(transparent)]
    Write(ConfigError),
}

/// A checkout added with the config written for it.
#[derive(Debug)]
pub struct ConfigAdded {
    pub checkout: PathBuf,
    /// Approving failed after the write, so the checkout needs approval.
    pub approval: Result<(), ConfigError>,
}

/// Lists the plan's checkout, writes the config `answers` render to (never
/// replacing a file), then approves each environment as rendered. A later
/// edit to the file is not approved.
pub fn add_with_config(
    trust_path: &Path,
    plan: &InitPlan,
    answers: &InitAnswers,
    now_ms: u128,
) -> Result<ConfigAdded, AddConfigError> {
    let text = plan.render(answers).map_err(AddConfigError::Write)?;
    let checkout = add(&path_beside(trust_path), plan.repo_root(), now_ms)?;
    plan.write(answers).map_err(AddConfigError::Write)?;
    let approval = LoadedConfig::parse(&text, plan.path().to_path_buf(), checkout.clone())
        .and_then(|config| {
            config
                .environment_names()
                .filter_map(|env| Some((env, config.trust_snapshot(env)?)))
                .try_for_each(|(env, snapshot)| approve_trust(trust_path, &checkout, env, snapshot))
        });
    Ok(ConfigAdded { checkout, approval })
}

/// The git root above `folder`, spelled as the approvals and receipts spell
/// checkouts.
fn checkout_root(folder: &Path) -> Result<PathBuf, ProjectsError> {
    let folder = fs::canonicalize(folder).map_err(|source| ProjectsError::Io {
        path: folder.to_path_buf(),
        source,
    })?;
    folder
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
        .ok_or(ProjectsError::NotGit(folder))
}

#[cfg(test)]
mod tests;
