use std::collections::BTreeMap;
use std::ffi::{CString, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::laravel::{self, InstallerOptions, LaravelError};
use crate::private_file;

static OPERATIONS: AtomicU64 = AtomicU64::new(0);
const TOKEN_ENV: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "SHIPSLIP_GITHUB_TOKEN",
];

#[derive(Debug, Error)]
pub enum CreateError {
    #[error("{0}")]
    Invalid(String),
    #[error("another ShipSlip operation is working on this folder: {0}")]
    Busy(PathBuf),
    #[error("could not access {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Laravel(#[from] LaravelError),
    #[error("{stage} failed; scaffold retained at {staging}: {reason}")]
    ScaffoldFailed {
        stage: String,
        staging: PathBuf,
        reason: String,
    },
    #[error("creation cancelled; scaffold retained at {0}")]
    Cancelled(PathBuf),
}

fn io_error(path: &Path, source: io::Error) -> CreateError {
    CreateError::Io {
        path: path.into(),
        source,
    }
}

pub fn default_operations_root() -> Result<PathBuf, CreateError> {
    let receipts = crate::receipt::default_receipts_root()
        .map_err(|error| CreateError::Invalid(error.to_string()))?;
    Ok(receipts
        .parent()
        .expect("receipts root has parent")
        .join("operations"))
}

pub(crate) fn target_key(target: &Path) -> String {
    Sha256::digest(target.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn private_dir(path: &Path) -> Result<(), CreateError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|error| io_error(path, error))
}

#[derive(Debug)]
pub(crate) struct OperationLock {
    _file: File,
}
impl Drop for OperationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self._file);
    }
}

impl OperationLock {
    pub(crate) fn acquire(root: &Path, target: &Path) -> Result<Self, CreateError> {
        let locks = root.join("locks");
        private_dir(&locks)?;
        let path = locks.join(format!("{}.lock", target_key(target)));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(&path)
            .map_err(|error| io_error(&path, error))?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Self { _file: file }),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Err(CreateError::Busy(target.into()))
            }
            Err(error) => Err(io_error(&path, error)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tools {
    pub php: PathBuf,
    pub composer: PathBuf,
    pub laravel: PathBuf,
    pub git: PathBuf,
    pub node: PathBuf,
    pub npm: PathBuf,
}

pub fn resolve_tool(name: &str) -> Result<PathBuf, CreateError> {
    crate::setup::resolve_tool(name)
}

impl Tools {
    pub fn resolve() -> Result<Self, CreateError> {
        crate::setup::detect_tools(
            &crate::setup::DetectionContext::from_environment(),
            &["php", "composer", "laravel", "git", "node", "npm"],
        )
        .creation_tools()
    }
    fn prepend_path(&self, command: &mut Command) -> Result<ToolPath, CreateError> {
        let binding = ToolPath::new(self)?;
        let directories = std::iter::once(binding.path.clone()).chain(
            std::env::var_os("PATH")
                .into_iter()
                .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>()),
        );
        let path = std::env::join_paths(directories)
            .map_err(|error| CreateError::Invalid(error.to_string()))?;
        command.env("PATH", path);
        Ok(binding)
    }
    async fn output(&self, binary: &Path, args: &[&str]) -> Result<String, CreateError> {
        let mut command = Command::new(binary);
        clean(&mut command);
        let _tool_path = self.prepend_path(&mut command)?;
        let output = command
            .args(args)
            .output()
            .await
            .map_err(|error| io_error(binary, error))?;
        if !output.status.success() {
            return Err(CreateError::Invalid(format!(
                "{} {} failed",
                binary.display(),
                args.join(" ")
            )));
        }
        let mut text = String::from_utf8_lossy(&output.stdout).to_string();
        if text.trim().is_empty() {
            text = String::from_utf8_lossy(&output.stderr).to_string();
        }
        Ok(text)
    }
    pub async fn versions(
        &self,
        options: &InstallerOptions,
    ) -> Result<BTreeMap<String, String>, CreateError> {
        let installer = self.output(&self.laravel, &["--version"]).await?;
        let help = self.output(&self.laravel, &["new", "--help"]).await?;
        let version = options.probe(&installer, &help)?;
        let mut versions = BTreeMap::from([("laravel".into(), version)]);
        for (name, binary) in [
            ("php", &self.php),
            ("composer", &self.composer),
            ("git", &self.git),
            ("node", &self.node),
            ("npm", &self.npm),
        ] {
            let output = self.output(binary, &["--version"]).await?;
            versions.insert(
                name.into(),
                output.lines().next().unwrap_or("unknown").to_string(),
            );
        }
        Ok(versions)
    }
}

#[derive(Debug)]
pub(crate) struct ToolPath {
    pub(crate) path: PathBuf,
}
impl ToolPath {
    fn new(tools: &Tools) -> Result<Self, CreateError> {
        Self::from_paths([
            ("php", tools.php.as_path()),
            ("composer", tools.composer.as_path()),
            ("laravel", tools.laravel.as_path()),
            ("git", tools.git.as_path()),
            ("node", tools.node.as_path()),
            ("npm", tools.npm.as_path()),
        ])
    }
    pub(crate) fn from_paths<'a>(
        tools: impl IntoIterator<Item = (&'a str, &'a Path)>,
    ) -> Result<Self, CreateError> {
        let id = format!(
            "{:x}-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id(),
            OPERATIONS.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(format!("shipslip-tools-{id}"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|error| io_error(&path, error))?;
        let binding = Self { path };
        for (name, binary) in tools {
            let target = fs::canonicalize(binary).map_err(|error| io_error(binary, error))?;
            let shim = binding.path.join(name);
            #[cfg(unix)]
            {
                let mut wrapper = b"#!/bin/sh\nexec '".to_vec();
                for byte in target.as_os_str().as_encoded_bytes() {
                    if *byte == b'\'' {
                        wrapper.extend_from_slice(b"'\\''");
                    } else {
                        wrapper.push(*byte);
                    }
                }
                wrapper.extend_from_slice(b"' \"$@\"\n");
                write_tool_wrapper(&shim, &wrapper).map_err(|error| io_error(&shim, error))?;
            }
            #[cfg(not(unix))]
            {
                let _ = (target, shim);
                return Err(CreateError::Invalid(
                    "local creation requires Linux or macOS".into(),
                ));
            }
        }
        Ok(binding)
    }
}

#[cfg(unix)]
pub(crate) fn write_tool_wrapper(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // A concurrent fork can inherit a writable script FD even with CLOEXEC,
    // making Linux exec return ETXTBSY until that child execs. Keep the writable
    // FD in a separate process and wait for it to close before using the script.
    let mut command = std::process::Command::new("/bin/sh");
    command
        .args([
            "-c",
            "set -C; umask 077; /bin/cat > \"$1\" && /bin/chmod 700 \"$1\"",
            "shipslip-wrapper-writer",
        ])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for key in TOKEN_ENV {
        command.env_remove(key);
    }
    let mut writer = command.spawn()?;
    let written = writer.stdin.take().unwrap().write_all(bytes);
    let output = writer.wait_with_output()?;
    written?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "could not write tool wrapper {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

impl Drop for ToolPath {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) fn clean(command: &mut Command) {
    for key in TOKEN_ENV {
        command.env_remove(key);
    }
    command.stdin(Stdio::null()).kill_on_drop(true);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub path: PathBuf,
    pub current_directory: bool,
    parent: PathBuf,
}

impl Destination {
    pub fn check(start: &Path, name: &str) -> Result<Self, CreateError> {
        if name != "."
            && (name.is_empty()
                || Path::new(name).components().count() != 1
                || !matches!(
                    Path::new(name).components().next(),
                    Some(Component::Normal(_))
                ))
        {
            return Err(CreateError::Invalid(
                "project name must be a single directory name or .".into(),
            ));
        }
        if fs::symlink_metadata(start).is_ok_and(|metadata| metadata.file_type().is_symlink())
            && name == "."
        {
            return Err(CreateError::Invalid(
                "destination must not be a symlink".into(),
            ));
        }
        let start = fs::canonicalize(start).map_err(|error| io_error(start, error))?;
        let current_directory = name == ".";
        let path = if current_directory {
            start.clone()
        } else {
            start.join(name)
        };
        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(CreateError::Invalid(
                "destination must not be a symlink".into(),
            ));
        }
        if current_directory {
            require_empty(&path)?;
            writable(&path)?;
        } else {
            require_absent(&path)?;
        }
        let parent = path
            .parent()
            .ok_or_else(|| {
                CreateError::Invalid("cannot create a project at the filesystem root".into())
            })?
            .to_path_buf();
        for ancestor in if current_directory {
            path.as_path()
        } else {
            parent.as_path()
        }
        .ancestors()
        {
            if fs::symlink_metadata(ancestor.join(".git")).is_ok() {
                return Err(CreateError::Invalid(format!(
                    "destination is inside an existing Git repository: {}",
                    ancestor.display()
                )));
            }
        }
        writable(&parent)?;
        Ok(Self {
            path,
            current_directory,
            parent,
        })
    }
    fn recheck(&self) -> Result<(), CreateError> {
        if fs::canonicalize(&self.parent).map_err(|error| io_error(&self.parent, error))?
            != self.parent
        {
            return Err(CreateError::Invalid(
                "destination parent changed after preview".into(),
            ));
        }
        if self.current_directory {
            if fs::symlink_metadata(&self.path)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(CreateError::Invalid("destination became a symlink".into()));
            }
            require_empty(&self.path)?;
        } else {
            require_absent(&self.path)?;
        }
        for ancestor in self.parent.ancestors() {
            if fs::symlink_metadata(ancestor.join(".git")).is_ok() {
                return Err(CreateError::Invalid(
                    "destination is now inside a Git repository".into(),
                ));
            }
        }
        Ok(())
    }
}

fn require_absent(path: &Path) -> Result<(), CreateError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(CreateError::Invalid(format!(
            "destination already exists: {}; nothing will be overwritten",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}
fn require_empty(path: &Path) -> Result<(), CreateError> {
    let mut entries = fs::read_dir(path).map_err(|error| io_error(path, error))?;
    if let Some(entry) = entries.next() {
        entry.map_err(|error| io_error(path, error))?;
        return Err(CreateError::Invalid(format!(
            "destination must be empty, including hidden files: {}",
            path.display()
        )));
    }
    Ok(())
}
pub(crate) fn writable(path: &Path) -> Result<(), CreateError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path).map_err(|error| io_error(path, error))?;
        let bytes = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| CreateError::Invalid("path contains NUL".into()))?;
        // SAFETY: bytes is a valid NUL-terminated pathname and access does not retain it.
        if metadata.permissions().mode() & 0o222 == 0
            || unsafe { libc::access(bytes.as_ptr(), libc::W_OK | libc::X_OK) } != 0
        {
            return Err(CreateError::Invalid(format!(
                "parent directory is not writable: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CreateRequest {
    pub name: String,
    pub options: InstallerOptions,
    pub branch: String,
}
impl CreateRequest {
    pub fn check(&self, start: &Path) -> Result<Destination, CreateError> {
        self.options.validate()?;
        if !crate::config::is_branch_name(&self.branch) {
            return Err(CreateError::Invalid("invalid --branch".into()));
        }
        Destination::check(start, &self.name)
    }
}
#[derive(Debug)]
pub struct CreatePreview {
    pub destination: Destination,
    pub request: CreateRequest,
    pub versions: BTreeMap<String, String>,
    pub staging: PathBuf,
    pub installer_args: Vec<OsString>,
    op_id: String,
    tools: Tools,
    operations_root: PathBuf,
}
pub struct CreateConfirmation {
    fingerprint: String,
}
impl CreatePreview {
    pub fn tools_path(&self) -> &std::ffi::OsStr {
        self.tools.laravel.as_os_str()
    }
    pub fn confirm(&self) -> CreateConfirmation {
        CreateConfirmation {
            fingerprint: self.fingerprint(),
        }
    }
    fn fingerprint(&self) -> String {
        Sha256::digest(format!("{self:?}").as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[cfg(test)]
#[path = "create_tests.rs"]
mod tests;

pub async fn preview(
    request: CreateRequest,
    start: &Path,
    tools: Tools,
    operations_root: PathBuf,
) -> Result<CreatePreview, CreateError> {
    let destination = request.check(start)?;
    crate::git::Git::new(tools.git.clone())
        .validate_branch(start, &request.branch)
        .map_err(|error| CreateError::Invalid(error.to_string()))?;
    let versions = tools.versions(&request.options).await?;
    let op_id = format!(
        "{:x}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        OPERATIONS.fetch_add(1, Ordering::Relaxed)
    );
    let staging = destination.parent.join(format!(".slip-new-{op_id}"));
    let installer_args = request.options.args(&staging.join("project"));
    Ok(CreatePreview {
        destination,
        request,
        versions,
        staging,
        installer_args,
        op_id,
        tools,
        operations_root,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateState {
    Planned,
    ScaffoldFailed,
    Verified,
    Moved,
    Committed,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateJournal {
    pub op_id: String,
    pub destination: PathBuf,
    pub staging: PathBuf,
    pub options: InstallerOptions,
    pub branch: String,
    pub state: CreateState,
    pub head: Option<String>,
}

fn save_journal(path: &Path, journal: &CreateJournal) -> Result<(), CreateError> {
    let bytes = serde_json::to_vec_pretty(journal)
        .map_err(|error| CreateError::Invalid(error.to_string()))?;
    private_file::write_atomic(path, &bytes).map_err(|error| io_error(path, error))
}

#[derive(Debug)]
pub enum CreateEvent {
    Started(String),
    Output(String),
    Verified(String),
    Moved(PathBuf),
    Journal(PathBuf),
}

#[derive(Debug)]
pub struct CreatedProject {
    pub root: PathBuf,
    pub git: PathBuf,
    pub framework_version: String,
    pub journal_path: PathBuf,
    journal: CreateJournal,
    _lock: OperationLock,
}
impl CreatedProject {
    pub fn mark_committed(&mut self, head: String) -> Result<(), CreateError> {
        self.journal.state = CreateState::Committed;
        self.journal.head = Some(head);
        save_journal(&self.journal_path, &self.journal)
    }
}

pub async fn execute_create(
    preview: CreatePreview,
    confirmation: CreateConfirmation,
    mut cancel: watch::Receiver<bool>,
    mut emit: impl FnMut(CreateEvent),
) -> Result<CreatedProject, CreateError> {
    if preview.fingerprint() != confirmation.fingerprint {
        return Err(CreateError::Invalid(
            "confirmation belongs to a different creation preview".into(),
        ));
    }
    let lock = OperationLock::acquire(&preview.operations_root, &preview.destination.path)?;
    preview.destination.recheck()?;
    let journal_dir = preview
        .operations_root
        .join("create")
        .join(target_key(&preview.destination.path));
    private_dir(&journal_dir)?;
    let journal_path = journal_dir.join(format!("{}.json", preview.op_id));
    let mut journal = CreateJournal {
        op_id: preview.op_id.clone(),
        destination: preview.destination.path.clone(),
        staging: preview.staging.clone(),
        options: preview.request.options.clone(),
        branch: preview.request.branch.clone(),
        state: CreateState::Planned,
        head: None,
    };
    save_journal(&journal_path, &journal)?;
    emit(CreateEvent::Journal(journal_path.clone()));
    // An owned container keeps cleanup safe even if the installer never creates its project.
    let mut directory = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory
        .create(&preview.staging)
        .map_err(|error| io_error(&preview.staging, error))?;
    private_file::write_atomic(
        &preview.staging.join(".slip-operation"),
        preview.op_id.as_bytes(),
    )
    .map_err(|error| io_error(&preview.staging, error))?;
    let scaffold_root = preview.staging.join("project");
    let scaffold = async {
        run_process(
            &preview.tools,
            &preview.tools.laravel,
            &preview.installer_args,
            &preview.destination.parent,
            "Laravel installer",
            &mut cancel,
            &mut emit,
        )
        .await?;
        let info = laravel::validate_scaffold(&scaffold_root, &preview.request.options)?;
        run_process(
            &preview.tools,
            &preview.tools.php,
            &["artisan".into(), "test".into()],
            &scaffold_root,
            "Application tests",
            &mut cancel,
            &mut emit,
        )
        .await?;
        let info_after_tests =
            laravel::validate_scaffold(&scaffold_root, &preview.request.options)?;
        if info.framework_version != info_after_tests.framework_version {
            return Err(CreateError::Invalid(
                "framework changed during verification".into(),
            ));
        }
        Ok::<_, CreateError>(info)
    }
    .await;
    let info = match scaffold {
        Ok(info) => info,
        Err(error) => {
            journal.state = CreateState::ScaffoldFailed;
            save_journal(&journal_path, &journal)?;
            if matches!(error, CreateError::Cancelled(_)) {
                return Err(CreateError::Cancelled(preview.staging));
            }
            return Err(CreateError::ScaffoldFailed {
                stage: "scaffold verification".into(),
                staging: preview.staging,
                reason: error.to_string(),
            });
        }
    };
    journal.state = CreateState::Verified;
    save_journal(&journal_path, &journal)?;
    emit(CreateEvent::Verified(info.framework_version.clone()));
    if *cancel.borrow() {
        journal.state = CreateState::ScaffoldFailed;
        save_journal(&journal_path, &journal)?;
        return Err(CreateError::Cancelled(preview.staging));
    }
    move_into_place(&preview.destination, &scaffold_root).map_err(|error| {
        CreateError::ScaffoldFailed {
            stage: "move".into(),
            staging: preview.staging.clone(),
            reason: error.to_string(),
        }
    })?;
    journal.state = CreateState::Moved;
    save_journal(&journal_path, &journal)?;
    remove_owned_staging(&preview.staging)?;
    emit(CreateEvent::Moved(preview.destination.path.clone()));
    Ok(CreatedProject {
        root: preview.destination.path,
        git: preview.tools.git,
        framework_version: info.framework_version,
        journal_path,
        journal,
        _lock: lock,
    })
}

/// Deletes only an operation-owned staging container, never the project destination.
pub fn remove_owned_staging(staging: &Path) -> Result<(), CreateError> {
    let metadata = fs::symlink_metadata(staging).map_err(|error| io_error(staging, error))?;
    let id = staging
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".slip-new-"))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| CreateError::Invalid("not an operation-owned staging directory".into()))?;
    let marker = staging.join(".slip-operation");
    if !metadata.is_dir()
        || fs::symlink_metadata(&marker).is_ok_and(|metadata| !metadata.file_type().is_file())
        || fs::read(&marker).map_err(|error| io_error(&marker, error))? != id.as_bytes()
    {
        return Err(CreateError::Invalid(
            "staging ownership marker does not match; nothing removed".into(),
        ));
    }
    fs::remove_dir_all(staging).map_err(|error| io_error(staging, error))
}

pub fn move_into_place(destination: &Destination, staging: &Path) -> Result<(), CreateError> {
    destination.recheck()?;
    if !fs::symlink_metadata(staging)
        .map_err(|error| io_error(staging, error))?
        .file_type()
        .is_dir()
    {
        return Err(CreateError::Invalid(
            "scaffold must be a real directory, not a symlink".into(),
        ));
    }
    if destination.current_directory {
        let entries = fs::read_dir(staging)
            .map_err(|error| io_error(staging, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(staging, error))?;
        for entry in entries {
            rename_exclusive(&entry.path(), &destination.path.join(entry.file_name()))?;
        }
        fs::remove_dir(staging).map_err(|error| io_error(staging, error))?;
    } else {
        rename_exclusive(staging, &destination.path)?;
    }
    Ok(())
}

fn rename_exclusive(source: &Path, destination: &Path) -> Result<(), CreateError> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let source_c = CString::new(source.as_os_str().as_encoded_bytes())
            .map_err(|_| CreateError::Invalid("path contains NUL".into()))?;
        let destination_c = CString::new(destination.as_os_str().as_encoded_bytes())
            .map_err(|_| CreateError::Invalid("path contains NUL".into()))?;
        #[cfg(target_os = "macos")]
        // SAFETY: both C strings are valid paths; RENAME_EXCL never replaces a destination.
        let result = unsafe {
            libc::renamex_np(source_c.as_ptr(), destination_c.as_ptr(), libc::RENAME_EXCL)
        };
        #[cfg(target_os = "linux")]
        // Call the kernel directly: Rust's bundled musl may lack the libc wrapper.
        // SAFETY: both C strings are valid paths; RENAME_NOREPLACE never replaces a destination.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source_c.as_ptr(),
                libc::AT_FDCWD,
                destination_c.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io_error(destination, io::Error::last_os_error()))
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (source, destination);
        Err(CreateError::Invalid(
            "safe project creation is supported on Linux and macOS".into(),
        ))
    }
}

struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: the negative PID targets only the process group created for this child.
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

#[derive(Default)]
struct ProcessOutput {
    spinner: Option<String>,
}

impl ProcessOutput {
    fn line(&mut self, raw: &str) -> Option<String> {
        static ANSI: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\))")
                .expect("terminal control pattern is valid")
        });
        let plain = ANSI.replace_all(raw, "");
        let line = plain.trim_end();
        if line.trim().is_empty() {
            return None;
        }
        let mut chars = line.trim_start().chars();
        if chars
            .next()
            .is_some_and(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
            && chars.next().is_some_and(char::is_whitespace)
        {
            let message = chars.as_str().trim().to_string();
            if message.is_empty() || self.spinner.as_ref() == Some(&message) {
                return None;
            }
            self.spinner = Some(message.clone());
            Some(message)
        } else {
            self.spinner = None;
            Some(line.to_string())
        }
    }
}

async fn run_process(
    tools: &Tools,
    binary: &Path,
    args: &[OsString],
    directory: &Path,
    stage: &str,
    cancel: &mut watch::Receiver<bool>,
    emit: &mut impl FnMut(CreateEvent),
) -> Result<(), CreateError> {
    if *cancel.borrow() {
        return Err(CreateError::Cancelled(directory.into()));
    }
    emit(CreateEvent::Started(stage.into()));
    let mut command = Command::new(binary);
    clean(&mut command);
    let _tool_path = tools.prepend_path(&mut command)?;
    command
        .args(args)
        .current_dir(directory)
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .env("CLICOLOR", "0")
        .env("TERM", "dumb")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| io_error(binary, error))?;
    let group = ProcessGroup(child.id().expect("new child has PID"));
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    fn reader(
        stream: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        sender: mpsc::UnboundedSender<String>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            let mut output = ProcessOutput::default();
            while let Ok(Some(line)) = lines.next_line().await {
                for frame in line.split('\r') {
                    if let Some(line) = output.line(frame) {
                        if sender.send(line).is_err() {
                            return;
                        }
                    }
                }
            }
        })
    }
    let readers = [reader(stdout, sender.clone()), reader(stderr, sender)];
    let mut status = None;
    let mut output_finished = false;
    let mut cancel_open = true;
    loop {
        if status.is_some() && output_finished {
            break;
        }
        tokio::select! {
            result = child.wait(), if status.is_none() => {
                status = Some(result.map_err(|error| io_error(binary, error))?);
                // Also close any pipes held by orphaned descendants.
                #[cfg(unix)]
                unsafe { libc::kill(-(group.0 as i32), libc::SIGKILL); }
            }
            line = receiver.recv(), if !output_finished => {
                match line { Some(line) => emit(CreateEvent::Output(line)), None => output_finished = true }
            }
            changed = cancel.changed(), if cancel_open => {
                if changed.is_err() { cancel_open = false; }
                else if *cancel.borrow() {
                    drop(group);
                    let _ = child.wait().await;
                    for handle in readers { handle.abort(); }
                    return Err(CreateError::Cancelled(directory.into()));
                }
            }
        }
    }
    for reader in readers {
        let _ = reader.await;
    }
    if !status.expect("child finished").success() {
        return Err(CreateError::Invalid(format!(
            "{stage} exited unsuccessfully"
        )));
    }
    Ok(())
}
