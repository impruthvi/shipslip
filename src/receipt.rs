//! Durable local record of a deploy, written before every remote mutation.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{
    DeployOutcome, DeployTarget, MaintenancePhase, Preview, RunPlan, SmokeResult, StepStatus,
    WatchResult,
};

mod history;
mod markdown;

pub use history::{
    describe_plan, describe_watch, failed_step_options, find, list, Badge, Flag, Found, Listed,
    Listing, MIN_ID_LEN,
};
pub use markdown::markdown;

const VERSION: u32 = 1;
const OUTPUT_LINES: usize = 200;
const OUTPUT_LINE_BYTES: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ReceiptError {
    #[error("receipt I/O at `{path}`: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid receipt `{path}`: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("could not serialize receipt: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("unsupported receipt schema version {0}")]
    Version(u32),
    #[error("invalid receipt: {0}")]
    Invalid(String),
    #[error("multiple unfinished receipts for this environment: {0}")]
    MultipleOpen(String),
    #[error("another process is already using receipt `{0}`")]
    Claimed(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptStatus {
    InProgress,
    Final,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptPhase {
    Preview,
    MaintenanceDown,
    Step(usize),
    MaintenanceUp,
    BetweenSteps,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptStepStatus {
    Pending,
    Running,
    Ok,
    Failed,
    Unknown,
    NotStarted,
    Skipped,
}

impl From<StepStatus> for ReceiptStepStatus {
    fn from(status: StepStatus) -> Self {
        match status {
            StepStatus::Ok => Self::Ok,
            StepStatus::Failed => Self::Failed,
            StepStatus::Unknown => Self::Unknown,
            StepStatus::NotStarted => Self::NotStarted,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptStep {
    pub index: usize,
    pub command: String,
    pub status: ReceiptStepStatus,
    pub exit_code: Option<i32>,
    pub output: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub version: u32,
    pub project: String,
    pub repo_root: String,
    pub run_id: String,
    pub owner_pid: u32,
    pub confirmed: bool,
    pub started_at_ms: u128,
    pub finished_at_ms: Option<u128>,
    pub status: ReceiptStatus,
    pub phase: ReceiptPhase,
    pub target: DeployTarget,
    pub run_plan: RunPlan,
    pub from_sha: String,
    pub target_sha: String,
    pub commits: Vec<String>,
    pub recipe_hash: String,
    pub steps: Vec<ReceiptStep>,
    pub maintenance_down_done: bool,
    pub maintenance_up_done: bool,
    pub maintenance_down_status: Option<StepStatus>,
    pub maintenance_down_exit_code: Option<i32>,
    pub maintenance_up_status: Option<StepStatus>,
    pub maintenance_up_exit_code: Option<i32>,
    pub app_left_down: bool,
    pub mutation_started: bool,
    pub outcome: Option<DeployOutcome>,
    pub server_head_at_end: Option<String>,
    pub tree_dirty: Option<bool>,
    pub last_message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub watch: Option<WatchResult>,
    #[serde(default)]
    pub smoke: Option<SmokeResult>,
    /// Who started the run; `None` in receipts written before it was recorded.
    #[serde(default)]
    pub actor: Option<Actor>,
}

/// The local user and Git identity that started a run. Each part is best
/// effort and never blocks a deploy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub user: Option<String>,
    pub git_name: Option<String>,
    pub git_email: Option<String>,
}

impl Actor {
    pub fn local(repo_root: &Path) -> Self {
        let git = crate::git::Git::new(PathBuf::from("git"));
        let config = |key: &str| {
            git.run(repo_root, &["config", "--get", key])
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        Self {
            user: std::env::var("USER").ok().filter(|user| !user.is_empty()),
            git_name: config("user.name"),
            git_email: config("user.email"),
        }
    }
}

impl Receipt {
    fn from_preview(project: &str, repo_root: &Path, preview: &Preview) -> Self {
        let target = preview.target().clone();
        let run_plan = preview.run_plan();
        let first = run_plan.first_recipe_step();
        let steps = crate::engine::plan_steps(&target, run_plan, preview.target_sha())
            .into_iter()
            .map(|(index, command)| ReceiptStep {
                index,
                command,
                status: if index > 0 && index < first {
                    ReceiptStepStatus::Skipped
                } else {
                    ReceiptStepStatus::Pending
                },
                exit_code: None,
                output: Vec::new(),
            })
            .collect();
        Self {
            version: VERSION,
            project: project.into(),
            repo_root: repo_root.to_string_lossy().into_owned(),
            run_id: preview.run_id().into(),
            owner_pid: std::process::id(),
            confirmed: false,
            started_at_ms: now_ms(),
            finished_at_ms: None,
            status: ReceiptStatus::InProgress,
            phase: ReceiptPhase::Preview,
            target,
            run_plan,
            from_sha: preview.from_sha().into(),
            target_sha: preview.target_sha().into(),
            commits: preview.commits().to_vec(),
            recipe_hash: preview.recipe_hash().into(),
            steps,
            maintenance_down_done: false,
            maintenance_up_done: false,
            maintenance_down_status: None,
            maintenance_down_exit_code: None,
            maintenance_up_status: None,
            maintenance_up_exit_code: None,
            app_left_down: false,
            mutation_started: false,
            outcome: None,
            server_head_at_end: None,
            tree_dirty: None,
            last_message: None,
            warnings: Vec::new(),
            watch: None,
            smoke: None,
            actor: Some(Actor::local(repo_root)),
        }
    }

    pub fn last_completed_step(&self) -> Option<usize> {
        self.steps
            .iter()
            .filter(|step| step.status == ReceiptStepStatus::Ok)
            .map(|step| step.index)
            .max()
    }
}

pub struct ReceiptJournal {
    path: PathBuf,
    state: Mutex<Receipt>,
    claim: Mutex<Option<File>>,
}

impl Drop for ReceiptJournal {
    fn drop(&mut self) {
        self.release_claim();
    }
}

impl ReceiptJournal {
    pub fn create(
        root: &Path,
        project: &str,
        repo_root: &Path,
        preview: &Preview,
    ) -> Result<Self, ReceiptError> {
        let receipt = Receipt::from_preview(project, repo_root, preview);
        let dir = receipt_dir(root, project, &receipt.target.env);
        fs::create_dir_all(&dir).map_err(|source| io_error(&dir, source))?;
        private_dir(&dir)?;
        let path = dir.join(format!("{}-{}.json", receipt.started_at_ms, receipt.run_id));
        write_atomic(&path, &receipt)?;
        let journal = Self {
            path,
            state: Mutex::new(receipt),
            claim: Mutex::new(None),
        };
        journal.claim()?;
        Ok(journal)
    }

    pub fn load(path: &Path) -> Result<Self, ReceiptError> {
        let receipt = read_receipt(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            state: Mutex::new(receipt),
            claim: Mutex::new(None),
        })
    }

    /// Holds an OS file lock until this journal is dropped. A killed process
    /// releases it automatically, while the receipt remains available.
    pub fn claim(&self) -> Result<(), ReceiptError> {
        let mut claim = self.claim.lock().unwrap();
        if claim.is_some() {
            return Ok(());
        }
        let lock_path = self.path.with_extension("json.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&lock_path)
            .map_err(|source| io_error(&lock_path, source))?;
        match file.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Err(ReceiptError::Claimed(self.path.clone()));
            }
            Err(source) => return Err(io_error(&lock_path, source)),
        }
        *self.state.lock().unwrap() = read_receipt(&self.path)?;
        *claim = Some(file);
        Ok(())
    }

    pub(crate) fn release_claim(&self) {
        if let Some(file) = self
            .claim
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = FileExt::unlock(&file);
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn snapshot(&self) -> Receipt {
        self.state.lock().unwrap().clone()
    }

    pub fn set_owner_pid(&self, pid: u32) -> Result<(), ReceiptError> {
        self.update(|receipt| receipt.owner_pid = pid)
    }

    pub fn confirm(&self) -> Result<(), ReceiptError> {
        self.update(|receipt| receipt.confirmed = true)
    }

    pub fn warning(&self, message: String) -> Result<(), ReceiptError> {
        self.update(|receipt| {
            if receipt.warnings.len() < 20 {
                receipt.warnings.push(message);
            }
        })
    }

    pub fn begin_step(&self, index: usize) -> Result<(), ReceiptError> {
        self.update(|receipt| {
            receipt.phase = ReceiptPhase::Step(index);
            receipt.mutation_started = true;
            if let Some(step) = receipt.steps.iter_mut().find(|step| step.index == index) {
                step.status = ReceiptStepStatus::Running;
            }
        })
    }

    pub fn finish_step(
        &self,
        index: usize,
        status: StepStatus,
        exit_code: Option<i32>,
    ) -> Result<(), ReceiptError> {
        self.update(|receipt| {
            receipt.phase = ReceiptPhase::BetweenSteps;
            if let Some(step) = receipt.steps.iter_mut().find(|step| step.index == index) {
                step.status = status.into();
                step.exit_code = exit_code;
            }
        })
    }

    pub fn begin_maintenance(&self, phase: MaintenancePhase) -> Result<(), ReceiptError> {
        self.update(|receipt| {
            receipt.phase = match phase {
                MaintenancePhase::Down => ReceiptPhase::MaintenanceDown,
                MaintenancePhase::Up => ReceiptPhase::MaintenanceUp,
            };
            receipt.mutation_started = true;
            if phase == MaintenancePhase::Down {
                receipt.app_left_down = true;
            }
        })
    }

    pub fn finish_maintenance(
        &self,
        phase: MaintenancePhase,
        status: StepStatus,
        exit_code: Option<i32>,
    ) -> Result<(), ReceiptError> {
        self.update(|receipt| {
            receipt.phase = ReceiptPhase::BetweenSteps;
            match phase {
                MaintenancePhase::Down => {
                    receipt.maintenance_down_status = Some(status.clone());
                    receipt.maintenance_down_exit_code = exit_code;
                    receipt.maintenance_down_done = status == StepStatus::Ok;
                    receipt.app_left_down = crate::event::down_may_have_started(&status);
                }
                MaintenancePhase::Up => {
                    receipt.maintenance_up_status = Some(status.clone());
                    receipt.maintenance_up_exit_code = exit_code;
                    receipt.maintenance_up_done = status == StepStatus::Ok;
                    receipt.app_left_down = status != StepStatus::Ok;
                }
            }
        })
    }

    pub fn output_line(&self, key: &str, line: &str) {
        let mut receipt = self.state.lock().unwrap();
        if let Some(index) = key
            .strip_prefix("step-")
            .and_then(|index| index.parse::<usize>().ok())
        {
            if let Some(step) = receipt.steps.iter_mut().find(|step| step.index == index) {
                if step.output.len() == OUTPUT_LINES {
                    step.output.remove(0);
                }
                let mut end = line.len().min(OUTPUT_LINE_BYTES);
                while !line.is_char_boundary(end) {
                    end -= 1;
                }
                step.output.push(line[..end].to_string());
            }
        }
    }

    pub fn server_state(
        &self,
        head: Option<String>,
        dirty: Option<bool>,
    ) -> Result<(), ReceiptError> {
        self.update(|receipt| {
            receipt.server_head_at_end = head;
            receipt.tree_dirty = dirty;
        })
    }

    pub fn message(&self, message: String) -> Result<(), ReceiptError> {
        self.update(|receipt| receipt.last_message = Some(message))
    }

    pub fn record_outcome(&self, outcome: DeployOutcome) -> Result<(), ReceiptError> {
        self.update(|receipt| receipt.outcome = Some(outcome))
    }

    pub fn observation(&self, watch: WatchResult, smoke: SmokeResult) -> Result<(), ReceiptError> {
        self.update(|receipt| {
            receipt.watch = Some(watch);
            receipt.smoke = Some(smoke);
        })
    }

    /// Where this env's log signature history lives: beside the receipts
    /// root, so a project named `signatures` cannot collide with it.
    pub fn history_paths(&self) -> HistoryPaths {
        let env_dir = self
            .path
            .parent()
            .expect("receipt has environment directory");
        let project_dir = env_dir.parent().expect("receipt has project directory");
        let root = project_dir.parent().expect("receipt has root directory");
        let tail = Path::new(project_dir.file_name().expect("project directory has name"))
            .join(env_dir.file_name().expect("environment directory has name"))
            .join("history.json");
        HistoryPaths {
            current: root
                .parent()
                .expect("receipts root has a parent")
                .join("signatures")
                .join(&tail),
            legacy: root.join("signatures").join(tail),
        }
    }

    pub fn finish(&self, outcome: DeployOutcome) -> Result<(), ReceiptError> {
        self.update(|receipt| {
            receipt.status = ReceiptStatus::Final;
            receipt.phase = ReceiptPhase::Finished;
            receipt.finished_at_ms = Some(now_ms());
            receipt.outcome = Some(outcome);
        })
    }

    fn update(&self, change: impl FnOnce(&mut Receipt)) -> Result<(), ReceiptError> {
        self.claim()?;
        let mut receipt = self.state.lock().unwrap();
        let mut next = receipt.clone();
        change(&mut next);
        write_atomic(&self.path, &next)?;
        *receipt = next;
        Ok(())
    }
}

/// Signature history file, and where releases up to 0.5.3 kept it inside the
/// receipts root. The legacy file is read when the current one is missing and
/// moved on the next write; no directory is ever removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPaths {
    pub current: PathBuf,
    pub legacy: PathBuf,
}

pub fn default_receipts_root() -> Result<PathBuf, ReceiptError> {
    let home =
        std::env::var_os("HOME").ok_or_else(|| ReceiptError::Invalid("HOME is not set".into()))?;
    let home = PathBuf::from(home);
    #[cfg(target_os = "macos")]
    let path = home.join("Library/Application Support/Shipslip/receipts");
    #[cfg(not(target_os = "macos"))]
    let path = home.join(".local/share/shipslip/receipts");
    Ok(path)
}

/// The fields [`find_open`] needs, readable from any receipt version.
#[derive(Deserialize)]
struct ReceiptSummary {
    status: ReceiptStatus,
    repo_root: String,
}

pub fn find_open(
    root: &Path,
    project: &str,
    env: &str,
    repo_root: &Path,
) -> Result<Option<PathBuf>, ReceiptError> {
    let dir = receipt_dir(root, project, env);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error(&dir, source)),
    };
    let mut open = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| io_error(&dir, source))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        // Only the summary must parse, so finished receipts from other
        // versions and stray files do not block attach.
        let bytes = fs::read(&path).map_err(|source| io_error(&path, source))?;
        let Ok(summary) = serde_json::from_slice::<ReceiptSummary>(&bytes) else {
            continue;
        };
        if summary.status != ReceiptStatus::InProgress
            || summary.repo_root != repo_root.to_string_lossy()
        {
            continue;
        }
        read_receipt(&path).map_err(|error| {
            ReceiptError::Invalid(format!(
                "unfinished receipt `{}` cannot be resumed: {error}",
                path.display()
            ))
        })?;
        open.push(path);
    }
    match open.len() {
        0 => Ok(None),
        1 => Ok(open.pop()),
        _ => Err(ReceiptError::MultipleOpen(
            open.iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        )),
    }
}

/// Reads a receipt this version of Shipslip can fully interpret.
pub fn read(path: &Path) -> Result<Receipt, ReceiptError> {
    read_receipt(path)
}

#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

fn read_receipt(path: &Path) -> Result<Receipt, ReceiptError> {
    let bytes = fs::read(path).map_err(|source| io_error(path, source))?;
    // Check the version first so a newer receipt reports its version
    // instead of a field error.
    let probe: VersionProbe =
        serde_json::from_slice(&bytes).map_err(|source| ReceiptError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    if probe.version != VERSION {
        return Err(ReceiptError::Version(probe.version));
    }
    let receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|source| ReceiptError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    if receipt.run_id.is_empty()
        || !receipt
            .run_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(ReceiptError::Invalid("invalid run id".into()));
    }
    Ok(receipt)
}

fn receipt_dir(root: &Path, project: &str, env: &str) -> PathBuf {
    root.join(safe_component(project)).join(safe_component(env))
}

fn safe_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("~{byte:02x}"));
        }
    }
    output
}

/// Removes color sequences (`ESC [ digits/semicolons m`) that tools like
/// Composer and Artisan write, so stored output reads as plain text. Other
/// control characters remain for the caller to escape.
pub fn strip_color(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("\x1b[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let params = after
            .find(|c: char| !(c.is_ascii_digit() || c == ';'))
            .unwrap_or(after.len());
        if after[params..].starts_with('m') {
            rest = &after[params + 1..];
        } else {
            out.push('\x1b');
            rest = &rest[start + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// `2026-10-07 14:02:11Z`.
pub fn format_utc(millis: u128) -> String {
    let secs = millis / 1000;
    let days = (secs / 86_400) as i64;
    let time = secs % 86_400;
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// `2m 44s`, `1h 3m`, or `850 ms`.
pub fn format_duration(millis: u128) -> String {
    let secs = millis / 1000;
    match secs {
        0 => format!("{millis} ms"),
        1..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs / 60 % 60),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn io_error(path: &Path, source: io::Error) -> ReceiptError {
    ReceiptError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn private_dir(path: &Path) -> Result<(), ReceiptError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|source| io_error(path, source))?;
    }
    Ok(())
}

fn write_atomic(path: &Path, receipt: &Receipt) -> Result<(), ReceiptError> {
    let mut bytes = serde_json::to_vec_pretty(receipt)?;
    bytes.push(b'\n');
    crate::private_file::write_atomic(path, &bytes).map_err(|source| io_error(path, source))
}

#[cfg(test)]
mod tests;
