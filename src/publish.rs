//! Reviewed, resumable publication to a new GitHub repository.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::create::{CreateError, OperationLock};
use crate::git::{GhCredentials, Git, GitError, HistoryScan};

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Create(#[from] CreateError),
    #[error("publication state could not be read or saved: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid publication journal or GitHub reply: {0}")]
    Json(#[from] serde_json::Error),
    #[error("GitHub CLI is not authenticated; run gh auth login")]
    NotAuthenticated,
    #[error("GitHub owner {0} is not accessible to the authenticated account")]
    OwnerUnavailable(String),
    #[error("gh {operation} failed: {message}")]
    Gh { operation: String, message: String },
    #[error("invalid GitHub owner or repository name")]
    InvalidRepository,
    #[error("history contains secrets; remove the listed files from all reachable commits before publishing")]
    Secrets,
    #[error("publication changed since preview; review it again")]
    Changed,
    #[error("existing publication uses {visibility} visibility; pass --visibility {visibility}; changing repository visibility is not supported")]
    VisibilityMismatch { visibility: Visibility },
    #[error("GitHub repository visibility changed: reviewed {expected}, now {actual}; restore the reviewed visibility before retrying")]
    RemoteVisibilityMismatch {
        expected: Visibility,
        actual: Visibility,
    },
    #[error("reviewed GitHub repository {0} no longer exists; restore it before retrying")]
    RepositoryMissing(String),
    #[error("repository {0} cannot be safely adopted; choose a new repository name")]
    Unresolved(String),
    #[error("a different publication is unfinished for this project; resume its reviewed owner/name first")]
    Pending,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Private,
    Public,
}
impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Private => "private",
            Self::Public => "public",
        })
    }
}
impl Visibility {
    pub fn flag(self) -> &'static str {
        match self {
            Self::Private => "--private",
            Self::Public => "--public",
        }
    }
}
#[derive(Debug, Clone)]
pub struct PublishRequest {
    pub root: PathBuf,
    pub owner: Option<String>,
    pub name: String,
    pub visibility: Visibility,
}
#[derive(Debug, Clone)]
pub struct PublishTools {
    pub git: Git,
    pub gh: PathBuf,
    pub credentials: GhCredentials,
}
impl PublishTools {
    pub fn new(git: PathBuf, gh: PathBuf) -> Self {
        let values = ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok().map(|v| (name.into(), v)));
        Self {
            git: Git::new(git),
            gh,
            credentials: GhCredentials::new(values),
        }
    }
    async fn gh_output(
        &self,
        root: &Path,
        args: &[&str],
    ) -> Result<std::process::Output, PublishError> {
        let mut command = tokio::process::Command::new(&self.gh);
        command
            .current_dir(root)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        self.credentials.apply(&mut command);
        for name in crate::git::REPOSITORY_ENV {
            command.env_remove(name);
        }
        command
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_HOST", "github.com");
        Ok(command.output().await?)
    }
    async fn gh(&self, root: &Path, args: &[&str]) -> Result<String, PublishError> {
        let output = self.gh_output(root, args).await?;
        if !output.status.success() {
            return Err(PublishError::Gh {
                operation: args.first().unwrap_or(&"").to_string(),
                message: self
                    .credentials
                    .redact(String::from_utf8_lossy(&output.stderr).trim()),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
    async fn repo(
        &self,
        root: &Path,
        repository: &str,
    ) -> Result<Option<RemoteRepository>, PublishError> {
        // REST status makes an absent repo distinct from auth/network failures.
        let output = self
            .gh_output(root, &["api", &format!("repos/{repository}")])
            .await?;
        if !output.status.success() {
            let message = self
                .credentials
                .redact(&String::from_utf8_lossy(&output.stderr));
            if message.contains("HTTP 404") {
                return Ok(None);
            }
            return Err(PublishError::Gh {
                operation: "repo view".into(),
                message: message.trim().into(),
            });
        }
        Ok(Some(serde_json::from_slice(&output.stdout)?))
    }
    async fn require_visibility(
        &self,
        root: &Path,
        repository: &str,
        visibility: Visibility,
    ) -> Result<(), PublishError> {
        self.repo(root, repository)
            .await?
            .ok_or_else(|| PublishError::RepositoryMissing(repository.into()))?
            .require_visibility(visibility)
    }
}
#[derive(Debug, Deserialize)]
struct RemoteRepository {
    private: bool,
    #[serde(default)]
    description: Option<String>,
}
impl RemoteRepository {
    fn require_visibility(&self, expected: Visibility) -> Result<(), PublishError> {
        let actual = if self.private {
            Visibility::Private
        } else {
            Visibility::Public
        };
        if actual != expected {
            return Err(PublishError::RemoteVisibilityMismatch { expected, actual });
        }
        Ok(())
    }
}
#[derive(Debug, Clone)]
pub struct PublishPreview {
    pub root: PathBuf,
    pub account: String,
    pub owner: String,
    pub name: String,
    pub visibility: Visibility,
    pub head: String,
    pub branch: String,
    pub history: HistoryScan,
    pub remote_url: String,
    operations_root: PathBuf,
    previous: Option<Journal>,
    lock: Arc<Mutex<Option<OperationLock>>>,
}
impl PublishPreview {
    pub fn repository(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
    pub fn confirm(&self) -> PublishConfirmation {
        PublishConfirmation {
            fingerprint: self.fingerprint(),
        }
    }
    fn fingerprint(&self) -> String {
        Sha256::digest(format!("{:?}", self).as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
#[derive(Debug)]
pub struct PublishConfirmation {
    fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishEvent {
    IntentSaved,
    RepositoryCreated,
    RepositoryAdopted,
    Pushed,
    Verified,
    DescriptionCleared,
    Done,
}
#[derive(Debug, Clone)]
pub struct PublishResult {
    pub repository: String,
    pub url: String,
    pub head: String,
    pub branch: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum State {
    IntentSaved,
    Created,
    Pushed,
    Verified,
    Done,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Journal {
    root: PathBuf,
    owner: String,
    name: String,
    op_id: String,
    planned_sha: String,
    branch: String,
    visibility: Visibility,
    state: State,
}
fn journal_path(operations_root: &Path, root: &Path) -> PathBuf {
    operations_root.join(format!(
        "publish-{}.json",
        Sha256::digest(root.as_os_str().as_encoded_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}
fn read_journal(operations_root: &Path, root: &Path) -> Result<Option<Journal>, PublishError> {
    match std::fs::read(journal_path(operations_root, root)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn save(operations_root: &Path, journal: &Journal) -> Result<(), PublishError> {
    std::fs::create_dir_all(operations_root)?;
    crate::private_file::write_atomic(
        &journal_path(operations_root, &journal.root),
        &serde_json::to_vec_pretty(journal)?,
    )?;
    Ok(())
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// All network probes are read-only; no repository or local origin is created.
pub async fn preview(
    request: PublishRequest,
    tools: &PublishTools,
    operations_root: &Path,
) -> Result<PublishPreview, PublishError> {
    let root = std::fs::canonicalize(tools.git.repository_root(&request.root)?)?;
    let lock = OperationLock::acquire(operations_root, &root)?;
    let previous = read_journal(operations_root, &root)?;
    if previous
        .as_ref()
        .is_some_and(|journal| journal.root != root)
    {
        return Err(PublishError::Changed);
    }
    if !tools
        .gh_output(
            &root,
            &["auth", "status", "--hostname", "github.com", "--active"],
        )
        .await?
        .status
        .success()
    {
        return Err(PublishError::NotAuthenticated);
    }
    let account = tools
        .gh(&root, &["api", "user", "--jq", ".login"])
        .await?
        .trim()
        .to_string();
    let owner = request.owner.unwrap_or_else(|| account.clone());
    if !valid_name(&owner) || !valid_name(&request.name) {
        return Err(PublishError::InvalidRepository);
    }
    if !owner.eq_ignore_ascii_case(&account) {
        let result = tools
            .gh_output(
                &root,
                &["api", &format!("orgs/{owner}/memberships/{account}")],
            )
            .await?;
        if !result.status.success()
            || serde_json::from_slice::<serde_json::Value>(&result.stdout)?
                .get("state")
                .and_then(|v| v.as_str())
                != Some("active")
        {
            return Err(PublishError::OwnerUnavailable(owner));
        }
    }
    let repository = format!("{owner}/{}", request.name);
    let existing_origin = tools.git.validate_origin(&root, &repository)?;
    let head = tools.git.head(&root)?;
    let branch = tools.git.current_branch(&root)?;
    let history = tools.git.history_scan(&root, &head)?;
    // Keep violations in the preview so the caller can show every commit/path.
    let remote_url = if let Some(origin) = existing_origin {
        origin
    } else {
        let protocol = tools
            .gh(
                &root,
                &["config", "get", "git_protocol", "--host", "github.com"],
            )
            .await?;
        match protocol.trim() {
        "https" => format!("https://github.com/{repository}.git"),
        "ssh" => format!("git@github.com:{repository}.git"),
        _ => return Err(PublishError::Gh {
            operation: "config".into(),
            message:
                "unsupported git_protocol; set gh config set git_protocol https --host github.com"
                    .into(),
        }),
        }
    };
    if let Some(journal) = &previous {
        if journal.owner == owner
            && journal.name == request.name
            && journal.visibility != request.visibility
        {
            return Err(PublishError::VisibilityMismatch {
                visibility: journal.visibility,
            });
        }
        if journal.state != State::Done
            && journal.state != State::IntentSaved
            && (journal.owner != owner
                || journal.name != request.name
                || journal.planned_sha != head
                || journal.branch != branch
                || journal.visibility != request.visibility)
        {
            return Err(PublishError::Pending);
        }
        if journal.owner == owner && journal.name == request.name {
            match tools.repo(&root, &repository).await? {
                Some(remote)
                    if journal.state != State::IntentSaved
                        || remote.description.as_deref()
                            == Some(format!("slip:{}", journal.op_id).as_str()) =>
                {
                    remote.require_visibility(request.visibility)?;
                }
                None if journal.state != State::IntentSaved => {
                    return Err(PublishError::RepositoryMissing(repository));
                }
                _ => {}
            }
        }
    }
    if let Some(journal) = &previous {
        if journal.state != State::Done
            && journal.owner == owner
            && journal.name == request.name
            && (journal.planned_sha != head
                || journal.branch != branch
                || journal.visibility != request.visibility)
        {
            return Err(PublishError::Pending);
        }
    }
    Ok(PublishPreview {
        root,
        account,
        owner,
        name: request.name,
        visibility: request.visibility,
        head,
        branch,
        history,
        remote_url,
        operations_root: operations_root.to_owned(),
        previous,
        lock: Arc::new(Mutex::new(Some(lock))),
    })
}

pub async fn execute_publish<F: FnMut(PublishEvent)>(
    preview: &PublishPreview,
    confirmation: PublishConfirmation,
    tools: &PublishTools,
    mut emit: F,
) -> Result<PublishResult, PublishError> {
    if confirmation.fingerprint != preview.fingerprint() {
        return Err(PublishError::Changed);
    }
    let _lock = preview
        .lock
        .lock()
        .map_err(|_| PublishError::Changed)?
        .take()
        .ok_or(PublishError::Changed)?;
    if read_journal(&preview.operations_root, &preview.root)? != preview.previous
        || tools.git.head(&preview.root)? != preview.head
        || tools.git.current_branch(&preview.root)? != preview.branch
    {
        return Err(PublishError::Changed);
    }
    if tools
        .gh(&preview.root, &["api", "user", "--jq", ".login"])
        .await?
        .trim()
        != preview.account
    {
        return Err(PublishError::Changed);
    }
    let repository = preview.repository();
    if tools
        .git
        .validate_origin(&preview.root, &repository)?
        .is_some_and(|origin| origin != preview.remote_url)
    {
        return Err(PublishError::Changed);
    }
    if !tools
        .git
        .history_scan(&preview.root, &preview.head)?
        .violations
        .is_empty()
    {
        return Err(PublishError::Secrets);
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut journal = match &preview.previous {
        Some(previous) if previous.owner == preview.owner && previous.name == preview.name => {
            if previous.state == State::Done
                && (previous.planned_sha != preview.head || previous.branch != preview.branch)
            {
                Journal {
                    planned_sha: preview.head.clone(),
                    branch: preview.branch.clone(),
                    state: State::Created,
                    ..previous.clone()
                }
            } else {
                if previous.planned_sha != preview.head {
                    return Err(PublishError::Changed);
                }
                previous.clone()
            }
        }
        _ => Journal {
            root: preview.root.clone(),
            owner: preview.owner.clone(),
            name: preview.name.clone(),
            op_id: format!(
                "{:x}-{:x}-{:x}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            planned_sha: preview.head.clone(),
            branch: preview.branch.clone(),
            visibility: preview.visibility,
            state: State::IntentSaved,
        },
    };
    save(&preview.operations_root, &journal)?;
    emit(PublishEvent::IntentSaved);
    let marker = format!("slip:{}", journal.op_id);
    if journal.state == State::IntentSaved {
        match tools.repo(&preview.root, &repository).await? {
            None => {
                tools
                    .gh(
                        &preview.root,
                        &[
                            "repo",
                            "create",
                            &repository,
                            preview.visibility.flag(),
                            "--description",
                            &marker,
                        ],
                    )
                    .await?;
                emit(PublishEvent::RepositoryCreated);
            }
            Some(remote) => {
                if remote.description.as_deref() != Some(&marker)
                    || !tools
                        .git
                        .remote_refs(
                            &preview.root,
                            &repository,
                            &preview.remote_url,
                            &tools.gh,
                            &tools.credentials,
                        )
                        .await?
                        .trim()
                        .is_empty()
                {
                    return Err(PublishError::Unresolved(repository));
                }
                emit(PublishEvent::RepositoryAdopted);
            }
        }
        journal.state = State::Created;
        save(&preview.operations_root, &journal)?;
    }
    tools
        .git
        .ensure_origin(&preview.root, &repository, &preview.remote_url)?;
    // Recheck the live remote after confirmation and creation/recovery. This
    // also protects a Done journal whose HEAD did not change from false claims.
    tools
        .require_visibility(&preview.root, &repository, preview.visibility)
        .await?;
    if journal.state == State::Created {
        tools
            .git
            .push(
                &preview.root,
                &repository,
                &preview.branch,
                &preview.head,
                &tools.gh,
                &tools.credentials,
            )
            .await?;
        journal.state = State::Pushed;
        save(&preview.operations_root, &journal)?;
        emit(PublishEvent::Pushed);
    }
    tools
        .git
        .verify_remote(
            &preview.root,
            &repository,
            &preview.branch,
            &preview.head,
            &tools.gh,
            &tools.credentials,
        )
        .await?;
    if journal.state != State::Done {
        journal.state = State::Verified;
        save(&preview.operations_root, &journal)?;
        emit(PublishEvent::Verified);
        if tools
            .repo(&preview.root, &repository)
            .await?
            .and_then(|repo| repo.description)
            .as_deref()
            == Some(&marker)
        {
            tools
                .gh(
                    &preview.root,
                    &["repo", "edit", &repository, "--description", ""],
                )
                .await?;
            emit(PublishEvent::DescriptionCleared);
        }
        journal.state = State::Done;
        save(&preview.operations_root, &journal)?;
    }
    emit(PublishEvent::Done);
    Ok(PublishResult {
        repository: repository.clone(),
        url: format!("https://github.com/{repository}"),
        head: preview.head.clone(),
        branch: preview.branch.clone(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::git::test_support::Fixture;
    struct Publishing {
        fixture: Fixture,
        tools: PublishTools,
        operations: PathBuf,
    }
    impl Publishing {
        fn new() -> Self {
            let fixture = Fixture::new();
            fixture.initialized();
            let gh = fixture.root.join("gh");
            let q = crate::script::shell_quote;
            Fixture::script(
                &gh,
                &format!(
                    r#"#!/bin/bash
root={root}
case "$1 $2" in
  'auth status')
    if [[ -f "$root/not-authenticated" ]]; then exit 1; fi
    if [[ -f "$root/inactive-account-invalid" && " $* " != *' --active '* ]]; then exit 1; fi
    exit 0 ;;
  'api user') echo test-user; exit 0 ;;
  'config get')
    if [[ -f "$root/config-forbidden" ]]; then exit 1; fi
    if [[ -f "$root/protocol" ]]; then cat "$root/protocol"; else echo https; fi; exit 0 ;;
  'api orgs/'*) if [[ -f "$root/owner-denied" ]]; then exit 1; fi; echo '{{"state":"active"}}'; exit 0 ;;
  'api repos/'*) if [[ ! -f "$root/exists" ]]; then echo 'HTTP 404' >&2; exit 1; fi
    description=$(cat "$root/description")
    private=true; [[ -f "$root/public" ]] && private=false
    printf '{{"description":"%s","private":%s}}\n' "$description" "$private"; exit 0 ;;
  'repo create') echo create >> "$root/calls"; touch "$root/exists"; printf '%s' "$6" > "$root/description"
    if [[ "$4" == --public || -f "$root/flip-public-after-create" ]]; then touch "$root/public"; else rm -f "$root/public"; fi
    if [[ -f "$root/fail-create" ]]; then rm "$root/fail-create"; echo 'reply lost' >&2; exit 1; fi; exit 0 ;;
  'repo edit') echo edit >> "$root/calls"; printf '%s' "$5" > "$root/description"; exit 0 ;;
esac
echo "unexpected gh call" >&2
exit 1
"#,
                    root = q(&fixture.root.to_string_lossy())
                ),
            );
            let tools = PublishTools {
                git: fixture.git.clone(),
                gh,
                credentials: GhCredentials::default(),
            };
            let operations = fixture.root.join("operations");
            Self {
                fixture,
                tools,
                operations,
            }
        }
        async fn preview(&self) -> Result<PublishPreview, PublishError> {
            preview(
                PublishRequest {
                    root: self.fixture.repo.clone(),
                    owner: None,
                    name: "app".into(),
                    visibility: Visibility::Private,
                },
                &self.tools,
                &self.operations,
            )
            .await
        }
        fn flag(&self, name: &str) {
            std::fs::write(self.fixture.root.join(name), "").unwrap();
        }
        fn calls(&self) -> String {
            std::fs::read_to_string(self.fixture.root.join("calls")).unwrap_or_default()
        }
        async fn execute(&self, p: &PublishPreview) -> Result<PublishResult, PublishError> {
            execute_publish(p, p.confirm(), &self.tools, |_| {}).await
        }
    }
    #[tokio::test]
    async fn happy_path_creates_pushes_verifies_and_clears_marker() {
        let f = Publishing::new();
        let p = f.preview().await.unwrap();
        assert_eq!(p.account, "test-user");
        assert_eq!(p.history.commit_count, 1);
        assert_eq!(p.visibility, Visibility::Private);
        let result = f.execute(&p).await.unwrap();
        assert_eq!(result.repository, "test-user/app");
        assert_eq!(f.calls(), "create\nedit\n");
        assert_eq!(
            std::fs::read_to_string(f.fixture.root.join("description")).unwrap(),
            ""
        );
        let journal = read_journal(&f.operations, &p.root).unwrap().unwrap();
        assert_eq!(journal.state, State::Done);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(journal_path(&f.operations, &p.root))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let p = f.preview().await.unwrap();
        f.execute(&p).await.unwrap();
        assert_eq!(f.calls(), "create\nedit\n");
    }
    #[tokio::test]
    async fn failed_push_resumes_without_creating_duplicate_repository() {
        let f = Publishing::new();
        f.flag("fail-push");
        let p = f.preview().await.unwrap();
        assert!(f.execute(&p).await.is_err());
        assert_eq!(f.calls(), "create\n");
        assert_eq!(
            read_journal(&f.operations, &p.root).unwrap().unwrap().state,
            State::Created
        );
        let p = f.preview().await.unwrap();
        f.execute(&p).await.unwrap();
        assert_eq!(f.calls(), "create\nedit\n");
    }
    #[tokio::test]
    async fn lost_create_reply_adopts_only_matching_marker_and_empty_refs() {
        let f = Publishing::new();
        f.flag("fail-create");
        let p = f.preview().await.unwrap();
        assert!(f.execute(&p).await.is_err());
        assert_eq!(
            read_journal(&f.operations, &p.root).unwrap().unwrap().state,
            State::IntentSaved
        );
        let p = f.preview().await.unwrap();
        let mut events = Vec::new();
        execute_publish(&p, p.confirm(), &f.tools, |e| events.push(e))
            .await
            .unwrap();
        assert!(events.contains(&PublishEvent::RepositoryAdopted));
        assert_eq!(f.calls(), "create\nedit\n");
    }
    #[tokio::test]
    async fn foreign_marker_and_nonempty_marked_repository_are_unresolved() {
        let f = Publishing::new();
        f.flag("exists");
        std::fs::write(f.fixture.root.join("description"), "some project").unwrap();
        let p = f.preview().await.unwrap();
        assert!(matches!(
            f.execute(&p).await,
            Err(PublishError::Unresolved(_))
        ));
        assert!(f.calls().is_empty());
        let f = Publishing::new();
        f.flag("fail-create");
        let p = f.preview().await.unwrap();
        assert!(f.execute(&p).await.is_err());
        f.fixture
            .git
            .run(
                &f.fixture.repo,
                &["push", f.fixture.bare.to_str().unwrap(), "custom-branch"],
            )
            .unwrap();
        let p = f.preview().await.unwrap();
        assert!(matches!(
            f.execute(&p).await,
            Err(PublishError::Unresolved(_))
        ));
        assert_eq!(f.calls(), "create\n");
    }
    #[tokio::test]
    async fn authentication_and_owner_checks_precede_create() {
        let f = Publishing::new();
        f.flag("not-authenticated");
        assert!(matches!(
            f.preview().await,
            Err(PublishError::NotAuthenticated)
        ));
        assert!(f.calls().is_empty());
        std::fs::remove_file(f.fixture.root.join("not-authenticated")).unwrap();
        f.flag("owner-denied");
        let request = PublishRequest {
            root: f.fixture.repo.clone(),
            owner: Some("organization".into()),
            name: "app".into(),
            visibility: Visibility::Public,
        };
        assert!(matches!(
            preview(request, &f.tools, &f.operations).await,
            Err(PublishError::OwnerUnavailable(_))
        ));
        assert!(f.calls().is_empty());
    }
    #[tokio::test]
    async fn preview_confirmation_rejects_changed_head_or_preview() {
        let f = Publishing::new();
        let mut p = f.preview().await.unwrap();
        let confirmation = p.confirm();
        p.name = "different".into();
        assert!(matches!(
            execute_publish(&p, confirmation, &f.tools, |_| {}).await,
            Err(PublishError::Changed)
        ));
        drop(p);
        let p = f.preview().await.unwrap();
        std::fs::write(f.fixture.repo.join("README.md"), "changed").unwrap();
        f.fixture
            .git
            .run(&f.fixture.repo, &["commit", "-am", "change"])
            .unwrap();
        assert!(matches!(f.execute(&p).await, Err(PublishError::Changed)));
        assert!(f.calls().is_empty());
    }
    #[tokio::test]
    async fn secrets_are_visible_in_preview_and_block_before_intent() {
        let f = Publishing::new();
        std::fs::write(f.fixture.repo.join("auth.json"), "secret").unwrap();
        f.fixture
            .git
            .run(&f.fixture.repo, &["add", "auth.json"])
            .unwrap();
        f.fixture
            .git
            .run(&f.fixture.repo, &["commit", "-m", "secret"])
            .unwrap();
        let p = f.preview().await.unwrap();
        assert_eq!(p.history.violations.len(), 1);
        assert!(matches!(f.execute(&p).await, Err(PublishError::Secrets)));
        assert!(f.calls().is_empty());
        assert!(!journal_path(&f.operations, &p.root).exists());
    }
    #[tokio::test]
    async fn unresolved_intent_allows_new_reviewed_name() {
        let f = Publishing::new();
        f.flag("exists");
        std::fs::write(f.fixture.root.join("description"), "foreign repo").unwrap();
        let p = f.preview().await.unwrap();
        assert!(matches!(
            f.execute(&p).await,
            Err(PublishError::Unresolved(_))
        ));
        let new = preview(
            PublishRequest {
                root: f.fixture.repo.clone(),
                owner: None,
                name: "another-app".into(),
                visibility: Visibility::Private,
            },
            &f.tools,
            &f.operations,
        )
        .await
        .unwrap();
        // The old repository is retained. Fake lookup represents the new name.
        std::fs::remove_file(f.fixture.root.join("exists")).unwrap();
        assert_eq!(
            f.execute(&new).await.unwrap().repository,
            "test-user/another-app"
        );
    }
    #[tokio::test]
    async fn done_journal_publishes_new_commit_without_recreate_or_description_loss() {
        let f = Publishing::new();
        let p = f.preview().await.unwrap();
        f.execute(&p).await.unwrap();
        std::fs::write(f.fixture.root.join("description"), "user description").unwrap();
        std::fs::write(f.fixture.repo.join(".shipslip.toml"), "config").unwrap();
        f.fixture.git.commit_config(&f.fixture.repo).unwrap();
        let p = f.preview().await.unwrap();
        f.execute(&p).await.unwrap();
        assert_eq!(f.calls(), "create\nedit\n");
        assert_eq!(
            std::fs::read_to_string(f.fixture.root.join("description")).unwrap(),
            "user description"
        );
    }
    #[tokio::test]
    async fn review_holds_per_target_lock_until_execution_finishes() {
        let f = Publishing::new();
        let p = f.preview().await.unwrap();
        assert!(matches!(
            f.preview().await,
            Err(PublishError::Create(CreateError::Busy(_)))
        ));
        f.execute(&p).await.unwrap();
        let again = f.preview().await.unwrap();
        drop(again);
    }
    #[tokio::test]
    async fn done_journal_pushes_new_branch_even_when_commit_is_unchanged() {
        let f = Publishing::new();
        let p = f.preview().await.unwrap();
        f.execute(&p).await.unwrap();
        f.fixture
            .git
            .run(&f.fixture.repo, &["switch", "-c", "another-branch"])
            .unwrap();
        let p = f.preview().await.unwrap();
        f.execute(&p).await.unwrap();
        assert_eq!(f.calls(), "create\nedit\n");
        assert_eq!(
            f.fixture
                .git
                .run(
                    &f.fixture.repo,
                    &[
                        "--git-dir",
                        f.fixture.bare.to_str().unwrap(),
                        "rev-parse",
                        "another-branch"
                    ]
                )
                .unwrap()
                .trim(),
            p.head
        );
    }
    #[tokio::test]
    async fn existing_publication_visibility_cannot_be_silently_changed() {
        let f = Publishing::new();
        let p = f.preview().await.unwrap();
        f.execute(&p).await.unwrap();
        let request = PublishRequest {
            root: f.fixture.repo.clone(),
            owner: None,
            name: "app".into(),
            visibility: Visibility::Public,
        };
        assert!(matches!(
            preview(request, &f.tools, &f.operations).await,
            Err(PublishError::VisibilityMismatch {
                visibility: Visibility::Private
            })
        ));
        assert_eq!(f.calls(), "create\nedit\n");
    }
    #[tokio::test]
    async fn invalid_inactive_account_does_not_block_the_active_account() {
        let f = Publishing::new();
        f.flag("inactive-account-invalid");
        let p = f.preview().await.unwrap();
        assert_eq!(p.account, "test-user");
        assert!(f.calls().is_empty());
        f.execute(&p).await.unwrap();
    }
    #[tokio::test]
    async fn preview_uses_existing_origin_in_both_transport_directions() {
        for (origin, preference) in [
            ("git@github.com:test-user/app.git", "https"),
            ("https://github.com/test-user/app.git", "ssh"),
        ] {
            let f = Publishing::new();
            f.fixture
                .git
                .ensure_origin(&f.fixture.repo, "test-user/app", origin)
                .unwrap();
            std::fs::write(f.fixture.root.join("protocol"), preference).unwrap();
            f.flag("config-forbidden");
            let p = f.preview().await.unwrap();
            assert_eq!(p.remote_url, origin);
            f.execute(&p).await.unwrap();
        }
    }
    #[tokio::test]
    async fn resume_preserves_origin_after_gh_transport_preference_changes() {
        let f = Publishing::new();
        std::fs::write(f.fixture.root.join("protocol"), "ssh").unwrap();
        f.flag("fail-push");
        let p = f.preview().await.unwrap();
        assert!(f.execute(&p).await.is_err());
        std::fs::write(f.fixture.root.join("protocol"), "https").unwrap();
        let p = f.preview().await.unwrap();
        assert_eq!(p.remote_url, "git@github.com:test-user/app.git");
        f.execute(&p).await.unwrap();
        assert_eq!(f.calls(), "create\nedit\n");
    }
    #[tokio::test]
    async fn changed_origin_transport_invalidates_confirmation() {
        let f = Publishing::new();
        let p = f.preview().await.unwrap();
        f.fixture
            .git
            .ensure_origin(
                &f.fixture.repo,
                "test-user/app",
                "git@github.com:test-user/app.git",
            )
            .unwrap();
        assert!(matches!(f.execute(&p).await, Err(PublishError::Changed)));
        assert!(f.calls().is_empty());
    }
    #[tokio::test]
    async fn resume_preview_checks_live_visibility_in_created_done_and_intent_states() {
        for state in [State::Created, State::Done, State::IntentSaved] {
            let f = Publishing::new();
            if state == State::Created {
                f.flag("fail-push");
            }
            if state == State::IntentSaved {
                f.flag("fail-create");
            }
            let p = f.preview().await.unwrap();
            let result = f.execute(&p).await;
            assert_eq!(result.is_ok(), state == State::Done);
            assert_eq!(
                read_journal(&f.operations, &p.root).unwrap().unwrap().state,
                state
            );
            f.flag("public");
            assert!(matches!(
                f.preview().await,
                Err(PublishError::RemoteVisibilityMismatch {
                    expected: Visibility::Private,
                    actual: Visibility::Public
                })
            ));
            assert_eq!(
                read_journal(&f.operations, &p.root).unwrap().unwrap().state,
                state
            );
        }
    }
    #[tokio::test]
    async fn remote_visibility_change_during_confirmation_blocks_retry_and_republication() {
        for state in [State::Created, State::Done] {
            let f = Publishing::new();
            if state == State::Created {
                f.flag("fail-push");
            }
            let p = f.preview().await.unwrap();
            let result = f.execute(&p).await;
            assert_eq!(result.is_ok(), state == State::Done);
            if state == State::Done {
                std::fs::write(f.fixture.repo.join(".shipslip.toml"), "new config").unwrap();
                f.fixture.git.commit_config(&f.fixture.repo).unwrap();
            }
            let refs_before = f
                .fixture
                .git
                .run(
                    &f.fixture.repo,
                    &[
                        "--git-dir",
                        f.fixture.bare.to_str().unwrap(),
                        "for-each-ref",
                    ],
                )
                .unwrap();
            let p = f.preview().await.unwrap();
            f.flag("public");
            assert!(matches!(
                f.execute(&p).await,
                Err(PublishError::RemoteVisibilityMismatch {
                    expected: Visibility::Private,
                    actual: Visibility::Public
                })
            ));
            let refs_after = f
                .fixture
                .git
                .run(
                    &f.fixture.repo,
                    &[
                        "--git-dir",
                        f.fixture.bare.to_str().unwrap(),
                        "for-each-ref",
                    ],
                )
                .unwrap();
            assert_eq!(refs_before, refs_after);
        }
    }
    #[tokio::test]
    async fn initial_creation_checks_actual_visibility_before_first_push() {
        let f = Publishing::new();
        f.flag("flip-public-after-create");
        let p = f.preview().await.unwrap();
        assert!(matches!(
            f.execute(&p).await,
            Err(PublishError::RemoteVisibilityMismatch { .. })
        ));
        assert_eq!(f.calls(), "create\n");
        assert_eq!(
            read_journal(&f.operations, &p.root).unwrap().unwrap().state,
            State::Created
        );
        assert!(f
            .fixture
            .git
            .run(
                &f.fixture.repo,
                &[
                    "--git-dir",
                    f.fixture.bare.to_str().unwrap(),
                    "for-each-ref"
                ]
            )
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn done_visibility_drift_blocks_false_success_even_without_a_new_commit() {
        let f = Publishing::new();
        let p = f.preview().await.unwrap();
        f.execute(&p).await.unwrap();
        let p = f.preview().await.unwrap();
        f.flag("public");
        assert!(matches!(
            f.execute(&p).await,
            Err(PublishError::RemoteVisibilityMismatch { .. })
        ));
    }
    #[tokio::test]
    async fn explicitly_public_publication_validates_public_remote() {
        let f = Publishing::new();
        let request = PublishRequest {
            root: f.fixture.repo.clone(),
            owner: None,
            name: "app".into(),
            visibility: Visibility::Public,
        };
        let p = preview(request.clone(), &f.tools, &f.operations)
            .await
            .unwrap();
        f.execute(&p).await.unwrap();
        let p = preview(request.clone(), &f.tools, &f.operations)
            .await
            .unwrap();
        f.execute(&p).await.unwrap();
        std::fs::remove_file(f.fixture.root.join("public")).unwrap();
        assert!(matches!(
            preview(request, &f.tools, &f.operations).await,
            Err(PublishError::RemoteVisibilityMismatch {
                expected: Visibility::Public,
                actual: Visibility::Private
            })
        ));
    }
}
