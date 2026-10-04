//! Local Git operations and narrowly scoped GitHub publication.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::github_auth::GitHubRepository;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("could not run git: {0}")]
    Io(#[from] std::io::Error),
    #[error("git {operation} failed: {message}")]
    Command { operation: String, message: String },
    #[error("Git author identity is missing; run git config --global user.name 'Your Name' and git config --global user.email 'you@example.com'")]
    MissingIdentity,
    #[error("refusing to stage secret file {0}")]
    Secret(String),
    #[error("origin is not the reviewed GitHub repository; remove or correct origin explicitly")]
    WrongOrigin,
    #[error("remote branch SHA differs: expected {expected}, found {actual}")]
    RemoteMismatch { expected: String, actual: String },
    #[error("invalid Git branch name")]
    InvalidBranch,
    #[error("Git state or deployment config changed since review; review it again")]
    ReviewChanged,
}

#[derive(Debug, Clone)]
pub struct Git {
    pub binary: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretPath {
    pub commit: String,
    pub path: String,
}
#[derive(Debug, Clone)]
pub struct HistoryScan {
    pub commit_count: usize,
    pub violations: Vec<SecretPath>,
}

/// Captured credentials are only installed in publication child processes.
#[derive(Clone, Default)]
pub struct GhCredentials(BTreeMap<String, zeroize::Zeroizing<String>>);
impl std::fmt::Debug for GhCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GhCredentials([REDACTED])")
    }
}
impl GhCredentials {
    pub fn new(values: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(
            values
                .into_iter()
                .filter(|(key, _)| TOKEN_NAMES.contains(&key.as_str()))
                .map(|(key, value)| (key, zeroize::Zeroizing::new(value)))
                .collect(),
        )
    }
    pub(crate) fn apply(&self, command: &mut tokio::process::Command) {
        for name in SCRUB_TOKEN_NAMES {
            command.env_remove(name);
        }
        for (name, value) in &self.0 {
            command.env(name, value.as_str());
        }
    }
    pub(crate) fn redact(&self, message: &str) -> String {
        self.0
            .values()
            .filter(|v| !v.is_empty())
            .fold(message.to_owned(), |text, value| {
                text.replace(value.as_str(), "[REDACTED]")
            })
    }
}
const TOKEN_NAMES: [&str; 3] = ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN"];
const SCRUB_TOKEN_NAMES: [&str; 5] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "SHIPSLIP_GITHUB_TOKEN",
];
pub(crate) const REPOSITORY_ENV: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
];

/// Deny secret filenames at every directory depth, including deleted history.
pub fn is_secret_path(path: &str) -> bool {
    path.split('/').any(|name| {
        (name.starts_with(".env") && name != ".env.example")
            || name == "auth.json"
            || name.ends_with(".pem")
            || name.ends_with(".key")
            || name.starts_with("id_rsa")
    })
}
fn excluded_initial(path: &str) -> bool {
    is_secret_path(path)
        || path
            .split('/')
            .any(|p| matches!(p, "CLAUDE.md" | "AUDIT.md"))
}

struct TemporaryIndex {
    directory: PathBuf,
    path: PathBuf,
}
impl TemporaryIndex {
    fn new() -> Result<Self, GitError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "shipslip-commit-index-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        Ok(Self {
            path: directory.join("index"),
            directory,
        })
    }
}
impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn valid_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn collect_output(output: std::process::Output, operation: &str) -> Result<String, GitError> {
    if !output.status.success() {
        return Err(GitError::Command {
            operation: operation.into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

impl Git {
    pub fn new(binary: PathBuf) -> Self {
        Self { binary }
    }
    fn command(&self, root: &Path) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .current_dir(root)
            .stdin(Stdio::null())
            .args(["-c", "core.hooksPath=/dev/null"]);
        for name in SCRUB_TOKEN_NAMES {
            command.env_remove(name);
        }
        for name in REPOSITORY_ENV {
            command.env_remove(name);
        }
        command
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_COUNT")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_GRAFT_FILE", "/dev/null");
        command
    }
    pub fn run(&self, root: &Path, args: &[&str]) -> Result<String, GitError> {
        collect_output(
            self.command(root).args(args).output()?,
            args.first().copied().unwrap_or(""),
        )
    }
    fn run_index(&self, root: &Path, index: &Path, args: &[&str]) -> Result<String, GitError> {
        collect_output(
            self.command(root)
                .env("GIT_INDEX_FILE", index)
                .args(args)
                .output()?,
            args.first().copied().unwrap_or(""),
        )
    }
    fn commit_tree(
        &self,
        root: &Path,
        tree: &str,
        parent: Option<&str>,
        identity: &Identity,
        message: &str,
    ) -> Result<String, GitError> {
        if identity.name.trim().is_empty() || identity.email.trim().is_empty() {
            return Err(GitError::MissingIdentity);
        }
        let mut command = self.command(root);
        command
            .args([
                "-c",
                "commit.gpgsign=false",
                "commit-tree",
                tree,
                "-m",
                message,
            ])
            .env("GIT_AUTHOR_NAME", &identity.name)
            .env("GIT_AUTHOR_EMAIL", &identity.email)
            .env("GIT_COMMITTER_NAME", &identity.name)
            .env("GIT_COMMITTER_EMAIL", &identity.email)
            .env_remove("GIT_AUTHOR_DATE")
            .env_remove("GIT_COMMITTER_DATE");
        if let Some(parent) = parent {
            command.args(["-p", parent]);
        }
        Ok(collect_output(command.output()?, "commit-tree")?
            .trim()
            .into())
    }
    fn initial_head_absent(&self, root: &Path) -> Result<(), GitError> {
        let output = self
            .command(root)
            .args(["rev-parse", "--verify", "--quiet", "HEAD"])
            .output()?;
        if output.status.code() == Some(1) {
            Ok(())
        } else if output.status.success() {
            Err(GitError::ReviewChanged)
        } else {
            collect_output(output, "rev-parse")?;
            Err(GitError::ReviewChanged)
        }
    }
    pub fn validate_branch(&self, root: &Path, branch: &str) -> Result<(), GitError> {
        if branch.starts_with('-')
            || self
                .run(root, &["check-ref-format", "--branch", branch])
                .is_err()
        {
            return Err(GitError::InvalidBranch);
        }
        Ok(())
    }
    pub fn init(&self, root: &Path, branch: &str) -> Result<(), GitError> {
        self.validate_branch(root, branch)?;
        self.run(root, &["init", "-b", branch])?;
        Ok(())
    }
    pub fn identity(&self, root: &Path) -> Result<Identity, GitError> {
        let name = self
            .run(root, &["config", "--get", "user.name"])
            .unwrap_or_default()
            .trim()
            .to_owned();
        let email = self
            .run(root, &["config", "--get", "user.email"])
            .unwrap_or_default()
            .trim()
            .to_owned();
        if name.is_empty() || email.is_empty() {
            return Err(GitError::MissingIdentity);
        }
        Ok(Identity { name, email })
    }
    pub fn stage_initial(&self, root: &Path) -> Result<Vec<String>, GitError> {
        self.identity(root)?;
        let paths = self.run(
            root,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        )?;
        for path in paths.split('\0').filter(|p| !p.is_empty()) {
            if excluded_initial(path) {
                continue;
            }
            self.run(root, &["--literal-pathspecs", "add", "--", path])?;
        }
        let staged = self.staged_paths(root)?;
        if let Some(path) = staged.iter().find(|p| excluded_initial(p)) {
            return Err(GitError::Secret(path.clone()));
        }
        Ok(staged)
    }
    pub fn staged_paths(&self, root: &Path) -> Result<Vec<String>, GitError> {
        Ok(self
            .run(root, &["diff", "--cached", "--name-only", "-z"])?
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect())
    }
    pub fn commit_initial(&self, root: &Path) -> Result<String, GitError> {
        let identity = self.identity(root)?;
        let branch = self.current_branch(root)?;
        let tree = self.run(root, &["write-tree"])?;
        self.commit_initial_reviewed(root, &branch, tree.trim(), &identity)
    }
    /// Creates exactly the reviewed root tree and author, with an absent-tip CAS.
    pub fn commit_initial_reviewed(
        &self,
        root: &Path,
        branch: &str,
        expected_tree: &str,
        identity: &Identity,
    ) -> Result<String, GitError> {
        self.validate_branch(root, branch)?;
        if !valid_oid(expected_tree) || self.current_branch(root)? != branch {
            return Err(GitError::ReviewChanged);
        }
        self.initial_head_absent(root)?;
        if let Some(path) = self
            .staged_paths(root)?
            .into_iter()
            .find(|path| excluded_initial(path))
        {
            return Err(GitError::Secret(path));
        }
        if self.run(root, &["write-tree"])?.trim() != expected_tree {
            return Err(GitError::ReviewChanged);
        }
        let head = self.commit_tree(
            root,
            expected_tree,
            None,
            identity,
            "Initial Laravel project",
        )?;
        if self.current_branch(root)? != branch {
            return Err(GitError::ReviewChanged);
        }
        self.run(
            root,
            &["update-ref", &format!("refs/heads/{branch}"), &head, ""],
        )?;
        Ok(head)
    }
    /// Stages and commits only this configuration; an unrelated index is refused.
    pub fn commit_config(&self, root: &Path) -> Result<String, GitError> {
        let identity = self.identity(root)?;
        let head = self.head(root)?;
        let bytes = std::fs::read(root.join(".shipslip.toml"))?;
        self.commit_config_reviewed(root, &bytes, &head, &identity)
    }
    fn config_index_is_exclusive(&self, root: &Path) -> Result<(), GitError> {
        if self
            .staged_paths(root)?
            .iter()
            .any(|path| path != ".shipslip.toml")
        {
            return Err(GitError::Command {
                operation: "commit".into(),
                message:
                    "unrelated staged changes; unstage them before committing deployment config"
                        .into(),
            });
        }
        Ok(())
    }
    /// Builds a private index from the reviewed parent, never from moving edits.
    pub fn commit_config_reviewed(
        &self,
        root: &Path,
        expected_bytes: &[u8],
        expected_head: &str,
        identity: &Identity,
    ) -> Result<String, GitError> {
        let branch = self.current_branch(root)?;
        let path = root.join(".shipslip.toml");
        if !valid_oid(expected_head)
            || self.head(root)? != expected_head
            || !std::fs::symlink_metadata(&path)?.file_type().is_file()
            || std::fs::read(&path)? != expected_bytes
        {
            return Err(GitError::ReviewChanged);
        }
        self.config_index_is_exclusive(root)?;
        let mut hashing = self
            .command(root)
            .args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Err(error) = hashing
            .stdin
            .take()
            .expect("piped hash input")
            .write_all(expected_bytes)
        {
            let _ = hashing.kill();
            let _ = hashing.wait();
            return Err(error.into());
        }
        let blob = collect_output(hashing.wait_with_output()?, "hash-object")?;
        let blob = blob.trim();
        let index = TemporaryIndex::new()?;
        self.run_index(root, &index.path, &["read-tree", expected_head])?;
        self.run_index(
            root,
            &index.path,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "100644",
                blob,
                ".shipslip.toml",
            ],
        )?;
        let tree = self.run_index(root, &index.path, &["write-tree"])?;
        self.config_index_is_exclusive(root)?;
        if self.head(root)? != expected_head
            || self.current_branch(root)? != branch
            || std::fs::read(&path)? != expected_bytes
        {
            return Err(GitError::ReviewChanged);
        }
        let head = self.commit_tree(
            root,
            tree.trim(),
            Some(expected_head),
            identity,
            "Configure ShipSlip deployment",
        )?;
        self.run(
            root,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "100644",
                blob,
                ".shipslip.toml",
            ],
        )?;
        self.run(
            root,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                &head,
                expected_head,
            ],
        )?;
        Ok(head)
    }
    pub fn head(&self, root: &Path) -> Result<String, GitError> {
        Ok(self.run(root, &["rev-parse", "HEAD"])?.trim().into())
    }
    pub fn current_branch(&self, root: &Path) -> Result<String, GitError> {
        let branch = self.run(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        self.validate_branch(root, branch.trim())?;
        Ok(branch.trim().into())
    }
    pub fn repository_root(&self, root: &Path) -> Result<PathBuf, GitError> {
        let output = self
            .command(root)
            .args(["rev-parse", "--show-toplevel"])
            .output()?;
        if !output.status.success() {
            return Err(GitError::Command {
                operation: "rev-parse".into(),
                message: String::from_utf8_lossy(&output.stderr).trim().into(),
            });
        }
        let mut bytes = output.stdout;
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
        }
        #[cfg(not(unix))]
        {
            String::from_utf8(bytes)
                .map(PathBuf::from)
                .map_err(|_| GitError::Command {
                    operation: "rev-parse".into(),
                    message: "Git returned an invalid repository path".into(),
                })
        }
    }
    pub fn status(&self, root: &Path) -> Result<String, GitError> {
        self.run(root, &["status", "--short"])
    }
    pub fn history_scan(&self, root: &Path, tip: &str) -> Result<HistoryScan, GitError> {
        // rev-list establishes the exact reachable set. Tree scans preserve paths
        // containing newlines and detect renamed copies of a previously seen blob.
        self.run(root, &["rev-list", "--objects", tip, "--"])?;
        let commits = self.run(root, &["rev-list", tip, "--"])?;
        let mut violations = Vec::new();
        for commit in commits.lines() {
            let tree = self.run(root, &["ls-tree", "-r", "--name-only", "-z", commit])?;
            for path in tree.split('\0').filter(|p| !p.is_empty()) {
                if is_secret_path(path) {
                    violations.push(SecretPath {
                        commit: commit.into(),
                        path: path.into(),
                    });
                }
            }
        }
        Ok(HistoryScan {
            commit_count: commits.lines().count(),
            violations,
        })
    }
    pub fn validate_origin(
        &self,
        root: &Path,
        repository: &str,
    ) -> Result<Option<String>, GitError> {
        let output = self
            .command(root)
            .args(["config", "--get-all", "remote.origin.url"])
            .output()?;
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        let origins = String::from_utf8_lossy(&output.stdout);
        let origins: Vec<_> = origins.lines().collect();
        if !output.status.success()
            || origins.len() != 1
            || GitHubRepository::from_origin(origins[0])
                .map(|r| !r.name().eq_ignore_ascii_case(repository))
                .unwrap_or(true)
        {
            return Err(GitError::WrongOrigin);
        }
        // A different push URL would send code/credentials to an unreviewed host.
        let push = self
            .command(root)
            .args(["config", "--get-all", "remote.origin.pushurl"])
            .output()?;
        if push.status.success() {
            return Err(GitError::WrongOrigin);
        }
        Ok(Some(origins[0].to_owned()))
    }
    pub fn ensure_origin(&self, root: &Path, repository: &str, url: &str) -> Result<(), GitError> {
        if self.validate_origin(root, repository)?.is_none() {
            self.run(root, &["remote", "add", "origin", url])?;
        }
        Ok(())
    }
    fn network_command(
        &self,
        root: &Path,
        gh: &Path,
        credentials: &GhCredentials,
        reviewed_https_url: Option<&str>,
    ) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.binary);
        command
            .current_dir(root)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        credentials.apply(&mut command);
        for name in REPOSITORY_ENV {
            command.env_remove(name);
        }
        command
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_ASKPASS")
            .env_remove("SSH_ASKPASS")
            .env_remove("GIT_CURL_VERBOSE")
            .env_remove("GIT_SSL_NO_VERIFY")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_GRAFT_FILE", "/dev/null")
            .env("GH_HOST", "github.com")
            .env("GH_PROMPT_DISABLED", "1");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_TRACE") {
                command.env_remove(name);
            }
        }
        let helper = format!(
            "!{} auth git-credential",
            crate::script::shell_quote(&gh.to_string_lossy())
        );
        command.args([
            "-c",
            "credential.helper=",
            "-c",
            "credential.https://github.com.helper=",
            "-c",
            &format!("credential.https://github.com.helper={helper}"),
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "http.followRedirects=false",
            "-c",
            "http.https://github.com.extraHeader=",
            "-c",
            "credential.interactive=false",
        ]);
        if let Some(url) = reviewed_https_url {
            // Git selects the most specific URL config. Host-wide settings do
            // not override an existing repository-specific configuration.
            for setting in ["followRedirects=false", "extraHeader=", "sslVerify=true"] {
                command.arg("-c").arg(format!("http.{url}.{setting}"));
            }
        }
        command
    }
    async fn network(
        &self,
        root: &Path,
        gh: &Path,
        credentials: &GhCredentials,
        args: &[&str],
    ) -> Result<String, GitError> {
        let explicit_url = args.iter().find(|arg| {
            arg.starts_with("https://") || arg.starts_with("git@") || arg.starts_with("ssh://")
        });
        let url = match explicit_url {
            Some(url) => (*url).to_owned(),
            None => self
                .run(root, &["config", "--get", "remote.origin.url"])?
                .trim_end_matches('\n')
                .to_owned(),
        };
        let reviewed_https_url = url
            .starts_with("https://github.com/")
            .then_some(url.as_str());
        let https = reviewed_https_url.is_some() && !args.contains(&"--get-url");
        let empty_credentials = GhCredentials::default();
        let output = self
            .network_command(
                root,
                gh,
                if https {
                    credentials
                } else {
                    &empty_credentials
                },
                reviewed_https_url,
            )
            .args(args)
            .output()
            .await?;
        if !output.status.success() {
            return Err(GitError::Command {
                operation: args.first().unwrap_or(&"").to_string(),
                message: credentials.redact(String::from_utf8_lossy(&output.stderr).trim()),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
    async fn check_network_origin(
        &self,
        root: &Path,
        repository: &str,
        gh: &Path,
        credentials: &GhCredentials,
    ) -> Result<(), GitError> {
        let origin = self
            .validate_origin(root, repository)?
            .ok_or(GitError::WrongOrigin)?;
        let effective = self
            .network(root, gh, credentials, &["ls-remote", "--get-url", "origin"])
            .await?;
        let push_urls = self.run(root, &["remote", "get-url", "--push", "--all", "origin"])?;
        if effective.trim() != origin
            || push_urls.trim() != origin
            || push_urls.lines().count() != 1
        {
            return Err(GitError::WrongOrigin);
        }
        Ok(())
    }
    pub async fn remote_refs(
        &self,
        root: &Path,
        repository: &str,
        url: &str,
        gh: &Path,
        credentials: &GhCredentials,
    ) -> Result<String, GitError> {
        GitHubRepository::from_origin(url).map_err(|_| GitError::WrongOrigin)?;
        let effective = self
            .network(root, gh, credentials, &["ls-remote", "--get-url", url])
            .await?;
        if effective.trim() != url
            || !GitHubRepository::from_origin(url)
                .map(|r| r.name().eq_ignore_ascii_case(repository))
                .unwrap_or(false)
        {
            return Err(GitError::WrongOrigin);
        }
        self.network(root, gh, credentials, &["ls-remote", url])
            .await
    }
    pub async fn push(
        &self,
        root: &Path,
        repository: &str,
        branch: &str,
        sha: &str,
        gh: &Path,
        credentials: &GhCredentials,
    ) -> Result<(), GitError> {
        self.validate_branch(root, branch)?;
        if (sha.len() != 40 && sha.len() != 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(GitError::Command {
                operation: "push".into(),
                message: "invalid reviewed commit SHA".into(),
            });
        }
        self.check_network_origin(root, repository, gh, credentials)
            .await?;
        let origin = self
            .validate_origin(root, repository)?
            .ok_or(GitError::WrongOrigin)?;
        self.network(
            root,
            gh,
            credentials,
            &["push", &origin, &format!("{sha}:refs/heads/{branch}")],
        )
        .await?;
        Ok(())
    }
    pub async fn verify_remote(
        &self,
        root: &Path,
        repository: &str,
        branch: &str,
        expected: &str,
        gh: &Path,
        credentials: &GhCredentials,
    ) -> Result<(), GitError> {
        self.check_network_origin(root, repository, gh, credentials)
            .await?;
        let output = self
            .network(
                root,
                gh,
                credentials,
                &["ls-remote", "origin", &format!("refs/heads/{branch}")],
            )
            .await?;
        let reviewed_ref = format!("refs/heads/{branch}");
        let mut matches = output.lines().filter_map(|line| {
            let mut fields = line.split_whitespace();
            let oid = fields.next()?;
            let reference = fields.next()?;
            (reference == reviewed_ref && fields.next().is_none()).then_some(oid)
        });
        let actual = matches.next().unwrap_or("absent");
        if matches.next().is_some() {
            return Err(GitError::RemoteMismatch {
                expected: expected.into(),
                actual: "multiple entries for the reviewed branch".into(),
            });
        }
        if actual != expected {
            return Err(GitError::RemoteMismatch {
                expected: expected.into(),
                actual: actual.into(),
            });
        }
        Ok(())
    }
    pub async fn push_and_verify(
        &self,
        root: &Path,
        repository: &str,
        branch: &str,
        sha: &str,
        gh: &Path,
        credentials: &GhCredentials,
    ) -> Result<(), GitError> {
        self.push(root, repository, branch, sha, gh, credentials)
            .await?;
        self.verify_remote(root, repository, branch, sha, gh, credentials)
            .await
    }
}

#[cfg(all(test, unix))]
pub(crate) mod test_support {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    pub struct Fixture {
        pub root: PathBuf,
        pub repo: PathBuf,
        pub git: Git,
        pub bare: PathBuf,
    }
    impl Fixture {
        pub fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "shipslip-publish-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let repo = root.join("project with spaces");
            std::fs::create_dir(&repo).unwrap();
            let bare = root.join("remote.git");
            let actual = Command::new("/usr/bin/git")
                .args(["init", "--bare", "-q"])
                .arg(&bare)
                .output()
                .unwrap();
            assert!(actual.status.success());
            let wrapper = root.join("git");
            let quote = crate::script::shell_quote;
            Self::script(
                &wrapper,
                &format!(
                    r#"#!/bin/bash
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
args=("$@")
network=0
geturl=0
push=0
for arg in "$@"; do
  [[ "$arg" == ls-remote ]] && network=1
  [[ "$arg" == push ]] && network=1 && push=1
  [[ "$arg" == --get-url ]] && geturl=1
done
if [[ "$network" == 1 && "$geturl" == 0 ]]; then
  if [[ "$push" == 1 && -f {fail} ]]; then rm {fail}; echo 'push rejected' >&2; exit 1; fi
  for i in "${{!args[@]}}"; do
    [[ "${{args[$i]}}" == origin || "${{args[$i]}}" == https://github.com/* || "${{args[$i]}}" == git@github.com:* ]] && args[$i]={bare}
  done
  exec /usr/bin/git -c remote.origin.url={bare} "${{args[@]}}"
fi
exec /usr/bin/git "$@"
"#,
                    fail = quote(&root.join("fail-push").to_string_lossy()),
                    bare = quote(&bare.to_string_lossy())
                ),
            );
            Self {
                root,
                repo,
                git: Git::new(wrapper),
                bare,
            }
        }
        pub fn script(path: &Path, text: &str) {
            std::fs::write(path, text).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        pub fn initialized(&self) {
            self.git.init(&self.repo, "custom-branch").unwrap();
            self.git
                .run(&self.repo, &["config", "user.name", "Test Developer"])
                .unwrap();
            self.git
                .run(&self.repo, &["config", "user.email", "test@example.com"])
                .unwrap();
            std::fs::write(self.repo.join("README.md"), "app").unwrap();
            self.git.stage_initial(&self.repo).unwrap();
            self.git.commit_initial(&self.repo).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::test_support::Fixture;
    use super::*;
    #[test]
    fn initial_stage_excludes_secrets_and_honors_branch() {
        let fixture = Fixture::new();
        fixture.git.init(&fixture.repo, "my-feature").unwrap();
        assert!(matches!(
            fixture.git.identity(&fixture.repo),
            Err(GitError::MissingIdentity)
        ));
        fixture
            .git
            .run(&fixture.repo, &["config", "user.name", "Test"])
            .unwrap();
        fixture
            .git
            .run(&fixture.repo, &["config", "user.email", "test@example.com"])
            .unwrap();
        for path in [
            ".env",
            ".env.example",
            "auth.json",
            "server.key",
            "id_rsa.pub",
            "CLAUDE.md",
            "AUDIT.md",
            "README.md",
        ] {
            std::fs::write(fixture.repo.join(path), "sample").unwrap();
        }
        assert_eq!(
            fixture.git.stage_initial(&fixture.repo).unwrap(),
            vec![".env.example", "README.md"]
        );
        fixture.git.commit_initial(&fixture.repo).unwrap();
        assert_eq!(
            fixture.git.current_branch(&fixture.repo).unwrap(),
            "my-feature"
        );
    }
    #[test]
    fn deleted_secret_and_renamed_blob_in_history_are_blocked() {
        let fixture = Fixture::new();
        fixture.initialized();
        std::fs::write(fixture.repo.join(".env"), "secret").unwrap();
        fixture.git.run(&fixture.repo, &["add", ".env"]).unwrap();
        fixture
            .git
            .run(&fixture.repo, &["commit", "-m", "secret"])
            .unwrap();
        let bad = fixture.git.head(&fixture.repo).unwrap();
        fixture.git.run(&fixture.repo, &["rm", ".env"]).unwrap();
        fixture
            .git
            .run(&fixture.repo, &["commit", "-m", "remove"])
            .unwrap();
        let scan = fixture
            .git
            .history_scan(&fixture.repo, &fixture.git.head(&fixture.repo).unwrap())
            .unwrap();
        assert_eq!(scan.commit_count, 3);
        assert_eq!(
            scan.violations,
            vec![SecretPath {
                commit: bad,
                path: ".env".into()
            }]
        );
        std::fs::write(fixture.repo.join(".env.example"), "sample").unwrap();
        fixture.git.stage_initial(&fixture.repo).unwrap();
        fixture
            .git
            .run(&fixture.repo, &["commit", "-m", "example"])
            .unwrap();
        assert_eq!(
            fixture
                .git
                .history_scan(&fixture.repo, &fixture.git.head(&fixture.repo).unwrap())
                .unwrap()
                .violations
                .len(),
            1
        );
    }
    #[test]
    fn origins_are_exact_and_config_commit_refuses_unrelated_index() {
        let f = Fixture::new();
        f.initialized();
        f.git
            .ensure_origin(&f.repo, "owner/app", "https://github.com/owner/app.git")
            .unwrap();
        assert!(f.git.validate_origin(&f.repo, "other/app").is_err());
        f.git
            .run(
                &f.repo,
                &[
                    "config",
                    "remote.origin.pushurl",
                    "https://evil.example/app",
                ],
            )
            .unwrap();
        assert!(f.git.validate_origin(&f.repo, "owner/app").is_err());
        std::fs::write(f.repo.join("unrelated"), "local").unwrap();
        f.git.run(&f.repo, &["add", "unrelated"]).unwrap();
        std::fs::write(f.repo.join(".shipslip.toml"), "config").unwrap();
        assert!(f.git.commit_config(&f.repo).is_err());
        f.git.run(&f.repo, &["reset", "HEAD", "unrelated"]).unwrap();
        f.git.commit_config(&f.repo).unwrap();
        assert!(f.git.status(&f.repo).unwrap().contains("unrelated"));
    }
    #[tokio::test]
    async fn scoped_push_verifies_sha_and_reports_mismatch() {
        let f = Fixture::new();
        f.initialized();
        f.git
            .ensure_origin(&f.repo, "owner/app", "https://github.com/owner/app.git")
            .unwrap();
        let credentials = GhCredentials::default();
        let sha = f.git.head(&f.repo).unwrap();
        f.git
            .push_and_verify(
                &f.repo,
                "owner/app",
                "custom-branch",
                &sha,
                Path::new("gh"),
                &credentials,
            )
            .await
            .unwrap();
        assert!(matches!(
            f.git
                .verify_remote(
                    &f.repo,
                    "owner/app",
                    "custom-branch",
                    "wrong",
                    Path::new("gh"),
                    &credentials
                )
                .await,
            Err(GitError::RemoteMismatch { .. })
        ));
        assert!(f
            .git
            .run(&f.repo, &["config", "--get", "credential.helper"])
            .is_err());
    }
    #[test]
    fn denylist_covers_nested_files_and_literal_pathspecs() {
        for p in [
            "x/.env.prod",
            "x/auth.json",
            "ssl/a.pem",
            "keys/a.key",
            "ssh/id_rsa.backup",
            ".env.example.bak",
        ] {
            assert!(is_secret_path(p));
        }
        for p in [".env.example", "x/.env.example", "key.txt"] {
            assert!(!is_secret_path(p));
        }
        let f = Fixture::new();
        f.initialized();
        std::fs::write(f.repo.join(":(glob)*"), "literal").unwrap();
        assert_eq!(f.git.stage_initial(&f.repo).unwrap(), vec![":(glob)*"]);
    }
    #[test]
    fn config_commit_disables_hooks_that_stage_unrelated_files() {
        let f = Fixture::new();
        f.initialized();
        std::fs::write(f.repo.join(".shipslip.toml"), "config").unwrap();
        std::fs::write(f.repo.join("unrelated"), "local").unwrap();
        Fixture::script(
            &f.repo.join(".git/hooks/pre-commit"),
            "#!/bin/sh\ngit add unrelated\n",
        );
        f.git.commit_config(&f.repo).unwrap();
        let paths = f
            .git
            .run(&f.repo, &["show", "--pretty=", "--name-only", "HEAD"])
            .unwrap();
        assert_eq!(paths.trim(), ".shipslip.toml");
    }
    #[tokio::test]
    async fn push_rewrites_are_refused_before_any_network_operation() {
        let f = Fixture::new();
        f.initialized();
        f.git
            .ensure_origin(&f.repo, "owner/app", "https://github.com/owner/app.git")
            .unwrap();
        f.git
            .run(
                &f.repo,
                &[
                    "config",
                    "url.https://other.example/.pushInsteadOf",
                    "https://github.com/",
                ],
            )
            .unwrap();
        let sha = f.git.head(&f.repo).unwrap();
        assert!(matches!(
            f.git
                .push(
                    &f.repo,
                    "owner/app",
                    "custom-branch",
                    &sha,
                    Path::new("gh"),
                    &GhCredentials::default()
                )
                .await,
            Err(GitError::WrongOrigin)
        ));
    }
    #[tokio::test]
    async fn publication_pushes_reviewed_sha_even_when_branch_changes() {
        let f = Fixture::new();
        f.initialized();
        f.git
            .ensure_origin(&f.repo, "owner/app", "https://github.com/owner/app.git")
            .unwrap();
        let reviewed = f.git.head(&f.repo).unwrap();
        std::fs::write(f.repo.join("auth.json"), "new secret").unwrap();
        f.git.run(&f.repo, &["add", "auth.json"]).unwrap();
        f.git.run(&f.repo, &["commit", "-m", "unreviewed"]).unwrap();
        f.git
            .push_and_verify(
                &f.repo,
                "owner/app",
                "custom-branch",
                &reviewed,
                Path::new("gh"),
                &GhCredentials::default(),
            )
            .await
            .unwrap();
        let remote = f
            .git
            .run(
                &f.repo,
                &[
                    "--git-dir",
                    f.bare.to_str().unwrap(),
                    "rev-parse",
                    "custom-branch",
                ],
            )
            .unwrap();
        assert_eq!(remote.trim(), reviewed);
    }
    #[test]
    fn ambient_repository_environment_cannot_redirect_initial_commit() {
        const CHILD_ROOT: &str = "SHIPSLIP_TEST_ROUTING_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            let project = root.join("project with spaces");
            let git = Git::new(root.join("git"));
            git.init(&project, "reviewed-project").unwrap();
            git.run(&project, &["config", "user.name", "Local Developer"])
                .unwrap();
            git.run(&project, &["config", "user.email", "local@example.com"])
                .unwrap();
            std::fs::write(project.join("README.md"), "local app").unwrap();
            std::fs::write(project.join(".env"), "do not publish").unwrap();
            assert_eq!(git.stage_initial(&project).unwrap(), vec!["README.md"]);
            git.commit_initial(&project).unwrap();
            assert_eq!(
                git.repository_root(&project)
                    .unwrap()
                    .canonicalize()
                    .unwrap(),
                project.canonicalize().unwrap()
            );
            assert_eq!(git.current_branch(&project).unwrap(), "reviewed-project");
            return;
        }
        let f = Fixture::new();
        let decoy = f.root.join("unrelated repository");
        std::fs::create_dir(&decoy).unwrap();
        f.git.init(&decoy, "unrelated").unwrap();
        let config = std::fs::read(decoy.join(".git/config")).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "git::tests::ambient_repository_environment_cannot_redirect_initial_commit",
                "--nocapture",
            ])
            .env(CHILD_ROOT, &f.root)
            .env("GIT_DIR", decoy.join(".git"))
            .env("GIT_WORK_TREE", &decoy)
            .env("GIT_INDEX_FILE", decoy.join(".git/index"))
            .env("GIT_COMMON_DIR", decoy.join(".git"))
            .env("GIT_OBJECT_DIRECTORY", decoy.join(".git/objects"))
            .env(
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                decoy.join(".git/objects"),
            )
            .env("GIT_NAMESPACE", "unexpected");
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read(decoy.join(".git/config")).unwrap(), config);
        assert!(!decoy.join(".git/index").exists());
        assert!(f.git.head(&decoy).is_err());
        assert_eq!(f.git.current_branch(&f.repo).unwrap(), "reviewed-project");
    }
    fn unborn_project() -> Fixture {
        let f = Fixture::new();
        f.git.init(&f.repo, "reviewed").unwrap();
        f.git
            .run(&f.repo, &["config", "user.name", "Reviewed Developer"])
            .unwrap();
        f.git
            .run(&f.repo, &["config", "user.email", "reviewed@example.com"])
            .unwrap();
        std::fs::write(f.repo.join("README.md"), "reviewed app").unwrap();
        f.git.stage_initial(&f.repo).unwrap();
        f
    }
    #[test]
    fn initial_review_rejects_index_and_branch_drift_and_existing_history() {
        let f = unborn_project();
        let identity = f.git.identity(&f.repo).unwrap();
        let tree = f.git.run(&f.repo, &["write-tree"]).unwrap();
        std::fs::write(f.repo.join("unreviewed"), "new file").unwrap();
        f.git.run(&f.repo, &["add", "unreviewed"]).unwrap();
        assert!(matches!(
            f.git
                .commit_initial_reviewed(&f.repo, "reviewed", tree.trim(), &identity),
            Err(GitError::ReviewChanged)
        ));
        assert!(f.git.head(&f.repo).is_err());
        f.git
            .run(&f.repo, &["rm", "--cached", "unreviewed"])
            .unwrap();
        f.git
            .run(&f.repo, &["symbolic-ref", "HEAD", "refs/heads/different"])
            .unwrap();
        assert!(matches!(
            f.git
                .commit_initial_reviewed(&f.repo, "reviewed", tree.trim(), &identity),
            Err(GitError::ReviewChanged)
        ));
        f.git
            .run(&f.repo, &["symbolic-ref", "HEAD", "refs/heads/reviewed"])
            .unwrap();
        f.git
            .commit_initial_reviewed(&f.repo, "reviewed", tree.trim(), &identity)
            .unwrap();
        assert!(matches!(
            f.git
                .commit_initial_reviewed(&f.repo, "reviewed", tree.trim(), &identity),
            Err(GitError::ReviewChanged)
        ));
    }
    #[test]
    fn initial_commit_pins_reviewed_author_and_tree_despite_later_config_change() {
        let f = unborn_project();
        let identity = f.git.identity(&f.repo).unwrap();
        let tree = f.git.run(&f.repo, &["write-tree"]).unwrap();
        f.git
            .run(&f.repo, &["config", "user.name", "Changed Developer"])
            .unwrap();
        f.git
            .run(&f.repo, &["config", "user.email", "changed@example.com"])
            .unwrap();
        let head = f
            .git
            .commit_initial_reviewed(&f.repo, "reviewed", tree.trim(), &identity)
            .unwrap();
        assert_eq!(
            f.git
                .run(
                    &f.repo,
                    &["show", "-s", "--format=%an <%ae> / %cn <%ce>", &head]
                )
                .unwrap()
                .trim(),
            "Reviewed Developer <reviewed@example.com> / Reviewed Developer <reviewed@example.com>"
        );
        assert_eq!(
            f.git.run(&f.repo, &["rev-parse", "HEAD^{tree}"]).unwrap(),
            tree
        );
        assert!(f.git.status(&f.repo).unwrap().is_empty());
    }
    #[test]
    fn config_review_rejects_changed_bytes_head_and_unrelated_staged_files() {
        let f = Fixture::new();
        f.initialized();
        let identity = f.git.identity(&f.repo).unwrap();
        let head = f.git.head(&f.repo).unwrap();
        let bytes = b"reviewed config";
        std::fs::write(f.repo.join(".shipslip.toml"), "unreviewed config").unwrap();
        assert!(matches!(
            f.git
                .commit_config_reviewed(&f.repo, bytes, &head, &identity),
            Err(GitError::ReviewChanged)
        ));
        std::fs::write(f.repo.join(".shipslip.toml"), bytes).unwrap();
        std::fs::write(f.repo.join("unrelated"), "unreviewed").unwrap();
        f.git.run(&f.repo, &["add", "unrelated"]).unwrap();
        assert!(f
            .git
            .commit_config_reviewed(&f.repo, bytes, &head, &identity)
            .is_err());
        f.git
            .run(&f.repo, &["commit", "-m", "unrelated change"])
            .unwrap();
        assert!(matches!(
            f.git
                .commit_config_reviewed(&f.repo, bytes, &head, &identity),
            Err(GitError::ReviewChanged)
        ));
    }
    #[test]
    fn private_config_tree_preserves_unrelated_index_changes_during_commit() {
        let f = Fixture::new();
        f.initialized();
        let identity = f.git.identity(&f.repo).unwrap();
        let head = f.git.head(&f.repo).unwrap();
        std::fs::write(f.repo.join(".shipslip.toml"), "reviewed config").unwrap();
        let racing = f.root.join("racing-git");
        Fixture::script(
            &racing,
            &format!(
                r#"#!/bin/bash
for arg in "$@"; do
  if [[ "$arg" == commit-tree ]]; then
    printf 'concurrent edit' > unrelated
    /usr/bin/git add -- unrelated || exit $?
  fi
done
exec {} "$@"
"#,
                crate::script::shell_quote(&f.git.binary.to_string_lossy())
            ),
        );
        let git = Git::new(racing);
        git.commit_config_reviewed(&f.repo, b"reviewed config", &head, &identity)
            .unwrap();
        assert_eq!(
            f.git
                .run(&f.repo, &["show", "--pretty=", "--name-only", "HEAD"])
                .unwrap()
                .trim(),
            ".shipslip.toml"
        );
        assert_eq!(f.git.staged_paths(&f.repo).unwrap(), vec!["unrelated"]);
        assert_eq!(
            f.git
                .run(&f.repo, &["show", "HEAD:.shipslip.toml"])
                .unwrap(),
            "reviewed config"
        );
        assert_eq!(
            std::fs::read_to_string(f.repo.join("unrelated")).unwrap(),
            "concurrent edit"
        );
    }
    #[test]
    fn initial_tip_compare_and_swap_never_overwrites_a_competing_commit() {
        let f = unborn_project();
        let identity = f.git.identity(&f.repo).unwrap();
        let tree = f.git.run(&f.repo, &["write-tree"]).unwrap();
        let racing = f.root.join("racing-git");
        Fixture::script(
            &racing,
            &format!(
                r#"#!/bin/bash
for arg in "$@"; do
  if [[ "$arg" == update-ref ]]; then
    competing=$(/usr/bin/git -c user.name=Concurrent -c user.email=concurrent@example.com commit-tree {} -m 'Competing commit') || exit $?
    /usr/bin/git -c core.hooksPath=/dev/null update-ref refs/heads/reviewed "$competing" '' || exit $?
  fi
done
exec {} "$@"
"#,
                tree.trim(),
                crate::script::shell_quote(&f.git.binary.to_string_lossy())
            ),
        );
        let git = Git::new(racing);
        assert!(git
            .commit_initial_reviewed(&f.repo, "reviewed", tree.trim(), &identity)
            .is_err());
        assert_eq!(
            f.git
                .run(&f.repo, &["show", "-s", "--format=%s", "HEAD"])
                .unwrap()
                .trim(),
            "Competing commit"
        );
    }
    #[tokio::test]
    async fn github_token_reaches_only_https_network_children() {
        let f = Fixture::new();
        f.initialized();
        let auditing = f.root.join("auditing-git");
        let audit = f.root.join("network-environment");
        Fixture::script(
            &auditing,
            &format!(
                r#"#!/bin/bash
network=0
geturl=0
for arg in "$@"; do
  [[ "$arg" == push || "$arg" == ls-remote ]] && network=1
  [[ "$arg" == --get-url ]] && geturl=1
done
if [[ "$network" == 1 && "$geturl" == 0 ]]; then
  printf '%s %s' "${{GH_TOKEN-unset}}" "${{GH_HOST-unset}}" > {}
fi
exec {} "$@"
"#,
                crate::script::shell_quote(&audit.to_string_lossy()),
                crate::script::shell_quote(&f.git.binary.to_string_lossy())
            ),
        );
        let git = Git::new(auditing);
        let credentials = GhCredentials::new([("GH_TOKEN".into(), "test_only_token".into())]);
        git.ensure_origin(&f.repo, "owner/app", "https://github.com/owner/app.git")
            .unwrap();
        let sha = git.head(&f.repo).unwrap();
        git.push_and_verify(
            &f.repo,
            "owner/app",
            "custom-branch",
            &sha,
            Path::new("gh"),
            &credentials,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&audit).unwrap(),
            "test_only_token github.com"
        );
        git.run(
            &f.repo,
            &[
                "remote",
                "set-url",
                "origin",
                "git@github.com:owner/app.git",
            ],
        )
        .unwrap();
        git.push_and_verify(
            &f.repo,
            "owner/app",
            "custom-branch",
            &sha,
            Path::new("gh"),
            &credentials,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&audit).unwrap(), "unset github.com");
    }
    #[test]
    fn repository_root_preserves_whitespace_newlines_and_native_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let f = Fixture::new();
        let git = Git::new(PathBuf::from("/usr/bin/git"));
        let sibling = f.root.join("app");
        std::fs::create_dir(&sibling).unwrap();
        git.init(&sibling, "sibling").unwrap();
        let invalid_parent = f
            .root
            .join(std::ffi::OsString::from_vec(b"parent-\xff".to_vec()));
        let mut roots = vec![
            f.root.join("app "),
            f.root.join("app\t"),
            f.root.join("app\n"),
            f.root.join("embedded\nnewline"),
        ];
        match std::fs::create_dir(&invalid_parent) {
            Ok(()) => roots.push(invalid_parent.join("child ")),
            // APFS rejects invalid UTF-8 filenames. Exercise the same native
            // bytes at the subprocess boundary instead on this filesystem.
            Err(error) if error.raw_os_error() == Some(92) => {
                let expected = invalid_parent.join("child ");
                let mut stdout = expected.as_os_str().as_bytes().to_vec();
                stdout.push(b'\n');
                let reply = f.root.join("native-path-reply");
                std::fs::write(&reply, stdout).unwrap();
                let wrapper = f.root.join("native-path-git");
                Fixture::script(
                    &wrapper,
                    &format!(
                        "#!/bin/sh\nexec /bin/cat {}\n",
                        crate::script::shell_quote(&reply.to_string_lossy())
                    ),
                );
                assert_eq!(
                    Git::new(wrapper).repository_root(&f.repo).unwrap(),
                    expected
                );
            }
            Err(error) => panic!("creating native path failed: {error}"),
        }
        for root in roots {
            std::fs::create_dir(&root).unwrap();
            git.init(&root, "reviewed").unwrap();
            let actual = git.repository_root(&root).unwrap();
            assert_eq!(actual, std::fs::canonicalize(&root).unwrap());
            assert_ne!(actual, std::fs::canonicalize(&sibling).unwrap());
            assert_eq!(git.current_branch(&actual).unwrap(), "reviewed");
        }
    }
    #[tokio::test]
    async fn verification_requires_the_exact_branch_ref_not_a_suffix_match() {
        let f = Fixture::new();
        f.initialized();
        f.git
            .ensure_origin(&f.repo, "owner/app", "https://github.com/owner/app.git")
            .unwrap();
        let sha = f.git.head(&f.repo).unwrap();
        f.git
            .run(
                &f.repo,
                &[
                    "--git-dir",
                    f.bare.to_str().unwrap(),
                    "fetch",
                    f.repo.to_str().unwrap(),
                    "custom-branch",
                ],
            )
            .unwrap();
        for reference in [
            "refs/tags/refs/heads/custom-branch",
            "refs/heads/nested/refs/heads/custom-branch",
        ] {
            f.git
                .run(
                    &f.repo,
                    &[
                        "--git-dir",
                        f.bare.to_str().unwrap(),
                        "update-ref",
                        reference,
                        &sha,
                    ],
                )
                .unwrap();
        }
        let credentials = GhCredentials::default();
        assert!(
            matches!(f.git.verify_remote(&f.repo, "owner/app", "custom-branch", &sha, Path::new("gh"), &credentials).await,
            Err(GitError::RemoteMismatch { actual, .. }) if actual == "absent")
        );
        // A suffix match with a different oid must not hide the correct branch.
        std::fs::write(f.repo.join("README.md"), "next commit").unwrap();
        f.git.run(&f.repo, &["add", "README.md"]).unwrap();
        f.git.run(&f.repo, &["commit", "-m", "next"]).unwrap();
        let new_sha = f.git.head(&f.repo).unwrap();
        f.git
            .run(
                &f.repo,
                &[
                    "--git-dir",
                    f.bare.to_str().unwrap(),
                    "fetch",
                    f.repo.to_str().unwrap(),
                    "custom-branch:custom-branch",
                ],
            )
            .unwrap();
        f.git
            .verify_remote(
                &f.repo,
                "owner/app",
                "custom-branch",
                &new_sha,
                Path::new("gh"),
                &credentials,
            )
            .await
            .unwrap();
        assert!(
            matches!(f.git.verify_remote(&f.repo, "owner/app", "custom-branch", &sha, Path::new("gh"), &credentials).await,
            Err(GitError::RemoteMismatch { actual, .. }) if actual == new_sha)
        );
    }
    #[tokio::test]
    async fn verification_rejects_duplicate_exact_branch_entries() {
        let f = Fixture::new();
        f.initialized();
        f.git
            .ensure_origin(&f.repo, "owner/app", "https://github.com/owner/app.git")
            .unwrap();
        let sha = f.git.head(&f.repo).unwrap();
        let wrapper = f.root.join("duplicate-ref-git");
        Fixture::script(
            &wrapper,
            &format!(
                r#"#!/bin/bash
for arg in "$@"; do
  if [[ "$arg" == --get-url ]]; then exec {} "$@"; fi
done
for arg in "$@"; do
  if [[ "$arg" == ls-remote ]]; then
    printf '%s\trefs/heads/custom-branch\n' {sha} {sha}
    exit 0
  fi
done
exec {} "$@"
"#,
                crate::script::shell_quote(&f.git.binary.to_string_lossy()),
                crate::script::shell_quote(&f.git.binary.to_string_lossy())
            ),
        );
        assert!(
            matches!(Git::new(wrapper).verify_remote(&f.repo, "owner/app", "custom-branch", &sha, Path::new("gh"), &GhCredentials::default()).await,
            Err(GitError::RemoteMismatch { actual, .. }) if actual.contains("multiple entries"))
        );
    }
    #[tokio::test]
    async fn reviewed_url_http_guards_override_repo_specific_configuration() {
        let f = Fixture::new();
        f.initialized();
        let url = "https://github.com/owner/app.git";
        for setting in [
            "followRedirects=true",
            "extraHeader=X-Audit: unreviewed",
            "sslVerify=false",
        ] {
            let (name, value) = setting.split_once('=').unwrap();
            f.git
                .run(&f.repo, &["config", &format!("http.{url}.{name}"), value])
                .unwrap();
        }
        let credentials = GhCredentials::default();
        for (name, expected) in [
            ("followRedirects", "false"),
            ("extraHeader", ""),
            ("sslVerify", "true"),
        ] {
            let output = f
                .git
                .network_command(&f.repo, Path::new("gh"), &credentials, Some(url))
                .args(["config", "--get-urlmatch", &format!("http.{name}"), url])
                .output()
                .await
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .trim_end_matches('\n'),
                expected
            );
        }
        // Hardening is command-scoped: normal Git keeps the user's settings.
        assert_eq!(
            f.git
                .run(
                    &f.repo,
                    &["config", "--get-urlmatch", "http.sslVerify", url]
                )
                .unwrap()
                .trim(),
            "false"
        );
    }
    #[tokio::test]
    async fn ambient_tls_override_is_removed_from_network_children() {
        const CHILD: &str = "SHIPSLIP_TEST_TLS_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "git::tests::ambient_tls_override_is_removed_from_network_children",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("GIT_SSL_NO_VERIFY", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert_eq!(std::env::var("GIT_SSL_NO_VERIFY").unwrap(), "1");
        let f = Fixture::new();
        let probe = f.root.join("tls-env-probe");
        Fixture::script(
            &probe,
            "#!/bin/sh\nprintf '%s' \"${GIT_SSL_NO_VERIFY-unset}\"\n",
        );
        let output = Git::new(probe)
            .network_command(
                &f.repo,
                Path::new("gh"),
                &GhCredentials::default(),
                Some("https://github.com/owner/app.git"),
            )
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"unset");
        assert_eq!(std::env::var("GIT_SSL_NO_VERIFY").unwrap(), "1");
    }
}
