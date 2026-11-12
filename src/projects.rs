//! The checkouts added in the app, and what a folder holds before it is
//! added. The list keeps each checkout's path and when it was added, nothing
//! else; everything the app shows about a checkout is read from its folder,
//! its approvals and its receipts.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ConfigError, LoadedConfig};
use crate::private_file::write_atomic;

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
    let mut checkouts = list(path)?;
    if checkouts.iter().any(|listed| listed.path == checkout) {
        return Ok(checkout);
    }
    checkouts.push(Checkout {
        path: checkout.clone(),
        added_at_ms: now_ms,
    });
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
    })?;
    Ok(checkout)
}

/// What a folder holds, read before it is added.
#[derive(Debug)]
pub enum Folder {
    /// Not inside a git repository, so nothing can be deployed from it.
    NotGit,
    /// A git checkout without a `.shipslip.toml` at its root.
    NoConfig { checkout: PathBuf },
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
        return Folder::NoConfig { checkout };
    }
    match LoadedConfig::load(&checkout, None) {
        Ok(config) => Folder::Config(config),
        Err(error) => Folder::Unreadable { checkout, error },
    }
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
