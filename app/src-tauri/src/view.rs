//! The release map as the webview gets it. Every string that came from a
//! receipt, the trust store or the file system is escaped here, and receipt
//! files are replaced by ids, so the UI never holds or sends a path.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use shipslip::git::Comparison;
use shipslip::logs::escape_field;
use shipslip::receipt::{
    age, code_text, comparison_text, newer_elsewhere_text, short_sha, stale_hint_text,
    target_moved_text, AttemptSummary, Badge, EnvCell, EnvComparison, Flag, Mark, Overview,
    ProblemKind, ReceiptError, RepoRow, StaleHint,
};
use shipslip::transport::ConnectFailure;
use shipslip::DeployOutcome;

/// The app shows 7 characters of a commit; the CLI shows 12.
const SHORT_SHA: usize = 7;

/// Receipt files behind the ids of one map load.
#[derive(Debug, Default)]
pub struct Locators(Vec<PathBuf>);

impl Locators {
    fn id(&mut self, path: &Path) -> usize {
        match self.0.iter().position(|known| known == path) {
            Some(id) => id,
            None => {
                self.0.push(path.to_path_buf());
                self.0.len() - 1
            }
        }
    }

    pub fn get(&self, id: usize) -> Option<&Path> {
        self.0.get(id).map(PathBuf::as_path)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapView {
    pub screen: Screen,
    /// Column headings, in grid order.
    pub envs: Vec<String>,
    pub rows: Vec<RowView>,
    pub problems: Vec<ProblemView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Screen {
    Map,
    /// No checkouts and nothing unreadable: nothing recorded yet.
    FirstRun,
    /// No checkouts, but files that could not be read.
    NoneReadable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowView {
    pub label: String,
    pub repo_root: String,
    pub path_missing: bool,
    /// One per column in [`MapView::envs`].
    pub cells: Vec<CellView>,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CellView {
    /// The app has no such environment.
    Absent,
    Present(Box<EnvCellView>),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvCellView {
    pub mark: MarkView,
    pub sha: Option<String>,
    pub short_sha: Option<String>,
    pub at_ms: Option<u64>,
    pub age: Option<String>,
    /// Warnings that the cell may be out of date, shown under the commit.
    pub qualifiers: Vec<String>,
    /// The state in words; empty for recorded code.
    pub words: String,
    pub evidence: Vec<String>,
    /// How the run behind the code ended, else the newest run.
    pub outcome: Option<OutcomeView>,
    pub flags: Vec<FlagView>,
    pub target: Option<String>,
    pub target_moved: Option<String>,
    pub needs_attention: bool,
    /// The receipt the pane opens: the run behind the code, else the
    /// newest run.
    pub receipt: Option<usize>,
    /// The newer receipt that could not be read, so the pane can say why.
    pub stale_receipt: Option<usize>,
    /// The newest run, when it is not the one behind the code.
    pub last_attempt: Option<AttemptView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MarkView {
    Recorded,
    Incomplete,
    Observed,
    NotKnown,
    NoCode,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeView {
    pub text: String,
    pub tone: Tone,
    pub kind: OutcomeKind,
}

/// How a run ended, for the one status word a map cell shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutcomeKind {
    Succeeded,
    /// A step exited non-zero.
    Failed,
    /// Stopped after a step, by the user or a lost lock.
    Stopped,
    /// Stopped before any deploy step ran; maintenance mode may be on.
    StoppedInMaintenance,
    /// Nothing changed on the server.
    Cancelled,
    /// A step's result could not be read.
    Unknown,
    /// The receipt is still in progress.
    Unfinished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    Good,
    Bad,
    Neutral,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlagView {
    pub text: String,
    /// Shown with a "NEW" chip.
    pub new_errors: bool,
    /// Something may be wrong now: new errors, a failed smoke check, or the
    /// app left in maintenance mode. Other flags say what was not recorded.
    pub bad: bool,
    pub kind: FlagKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FlagKind {
    NewErrors,
    LogNotFullyObserved,
    LogNotRecorded,
    SmokeFailed,
    SmokeNotRecorded,
    AppLeftDown,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptView {
    pub receipt: usize,
    pub outcome: OutcomeView,
    pub flags: Vec<FlagView>,
    pub at_ms: u64,
    pub age: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemView {
    pub kind: ProblemKindView,
    pub message: String,
    pub path: String,
    /// Cells it may make out of date; the strip item selects the first.
    pub cells: Vec<CellPos>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProblemKindView {
    Trust,
    ProjectDir,
    Receipt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CellPos {
    pub row: usize,
    pub col: usize,
}

/// The message for a receipts root that cannot be read at all.
pub fn load_error(error: &ReceiptError) -> String {
    escape_field(&error.to_string())
}

/// Every env any checkout has, sorted; one grid column each.
pub fn columns(overview: &Overview) -> Vec<&str> {
    overview
        .repos
        .iter()
        .flat_map(|row| row.envs.iter().map(|cell| cell.env.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareView {
    pub text: String,
    /// False for "cannot compare: …", shown muted.
    pub available: bool,
}

pub fn compare_view(a: &str, b: &str, comparison: &EnvComparison) -> CompareView {
    CompareView {
        text: comparison_text(a, b, comparison),
        available: !matches!(comparison.result, Comparison::Unavailable(_)),
    }
}

pub fn map(overview: &Overview, now_ms: u128, locators: &mut Locators) -> MapView {
    let screen = match (overview.repos.is_empty(), overview.problems.is_empty()) {
        (false, _) => Screen::Map,
        (true, true) => Screen::FirstRun,
        (true, false) => Screen::NoneReadable,
    };
    let columns = columns(overview);
    let rows = overview
        .repos
        .iter()
        .map(|row| row_view(row, &columns, now_ms, locators))
        .collect();
    let problems = overview
        .problems
        .iter()
        .map(|problem| ProblemView {
            kind: match problem.kind {
                ProblemKind::Trust => ProblemKindView::Trust,
                ProblemKind::ProjectDir => ProblemKindView::ProjectDir,
                ProblemKind::Receipt => ProblemKindView::Receipt,
            },
            message: escape_field(&problem.message),
            path: escape_field(&problem.path.to_string_lossy()),
            cells: problem
                .cells
                .iter()
                .filter_map(|cell| {
                    let row = overview
                        .repos
                        .iter()
                        .position(|row| row.repo_root == cell.repo_root)?;
                    let col = columns.iter().position(|env| *env == cell.env)?;
                    Some(CellPos { row, col })
                })
                .collect(),
        })
        .collect();
    MapView {
        screen,
        envs: columns.iter().map(|env| escape_field(env)).collect(),
        rows,
        problems,
    }
}

fn row_view(row: &RepoRow, columns: &[&str], now_ms: u128, locators: &mut Locators) -> RowView {
    RowView {
        label: escape_field(&row.label),
        repo_root: escape_field(&row.repo_root),
        path_missing: row.path_missing,
        cells: columns
            .iter()
            .map(|env| match row.envs.iter().find(|cell| cell.env == *env) {
                Some(cell) => CellView::Present(Box::new(cell_view(cell, now_ms, locators))),
                None => CellView::Absent,
            })
            .collect(),
    }
}

fn cell_view(cell: &EnvCell, now_ms: u128, locators: &mut Locators) -> EnvCellView {
    let text = code_text(cell);
    let stale_path = cell.stale_hint.as_ref().map(|hint| match hint {
        StaleHint::NewestUnreadable { path } | StaleHint::UnattributedNewer { path } => path,
    });
    let opened = cell.code_run.as_ref().or(cell.last_attempt.as_ref());
    let last_attempt = cell.last_attempt.as_ref().filter(|last| {
        cell.code_run
            .as_ref()
            .is_some_and(|run| run.run_id != last.run_id)
    });
    EnvCellView {
        mark: match text.mark {
            Mark::Recorded => MarkView::Recorded,
            Mark::Incomplete => MarkView::Incomplete,
            Mark::Observed => MarkView::Observed,
            Mark::NotKnown => MarkView::NotKnown,
            Mark::NoCode => MarkView::NoCode,
        },
        short_sha: text
            .sha
            .as_deref()
            .map(|sha| short_sha(sha, SHORT_SHA).into()),
        sha: text.sha,
        at_ms: text.at_ms.map(millis),
        age: text.at_ms.map(|at_ms| age(now_ms, at_ms)),
        qualifiers: stale_hint_text(cell)
            .into_iter()
            .chain(newer_elsewhere_text(cell, now_ms))
            .collect(),
        words: text.words,
        evidence: text.evidence,
        outcome: opened.map(|run| outcome(&run.badge)),
        flags: cell
            .code_run
            .as_ref()
            .map(|run| flags(&run.badge))
            .unwrap_or_default(),
        target: cell.target.as_deref().map(escape_field),
        target_moved: target_moved_text(cell),
        needs_attention: cell.needs_attention(),
        receipt: opened.map(|run| locators.id(&run.path)),
        stale_receipt: stale_path.map(|path| locators.id(path)),
        last_attempt: last_attempt.map(|last| attempt_view(last, now_ms, locators)),
    }
}

fn attempt_view(attempt: &AttemptSummary, now_ms: u128, locators: &mut Locators) -> AttemptView {
    AttemptView {
        receipt: locators.id(&attempt.path),
        outcome: outcome(&attempt.badge),
        flags: flags(&attempt.badge),
        at_ms: millis(attempt.started_at_ms),
        age: age(now_ms, attempt.started_at_ms),
    }
}

pub fn outcome(badge: &Badge) -> OutcomeView {
    OutcomeView {
        text: escape_field(&badge.headline()),
        tone: tone(badge),
        kind: outcome_kind(badge),
    }
}

fn outcome_kind(badge: &Badge) -> OutcomeKind {
    if badge.unfinished {
        return OutcomeKind::Unfinished;
    }
    match &badge.outcome {
        Some(DeployOutcome::Succeeded) => OutcomeKind::Succeeded,
        Some(DeployOutcome::FailedAtStep { .. }) => OutcomeKind::Failed,
        Some(DeployOutcome::StoppedAfterStep { .. }) => OutcomeKind::Stopped,
        Some(DeployOutcome::StoppedInMaintenance(_)) => OutcomeKind::StoppedInMaintenance,
        Some(DeployOutcome::Unknown { .. }) => OutcomeKind::Unknown,
        Some(DeployOutcome::CancelledBeforeChanges | DeployOutcome::AbortedBeforeChanges(_))
        | None => OutcomeKind::Cancelled,
    }
}

pub fn tone(badge: &Badge) -> Tone {
    match (&badge.outcome, badge.unfinished) {
        (_, true)
        | (
            Some(
                DeployOutcome::FailedAtStep { .. }
                | DeployOutcome::StoppedAfterStep { .. }
                | DeployOutcome::StoppedInMaintenance(_)
                | DeployOutcome::Unknown { .. },
            ),
            false,
        ) => Tone::Bad,
        (Some(DeployOutcome::Succeeded), false) => Tone::Good,
        _ => Tone::Neutral,
    }
}

pub fn flags(badge: &Badge) -> Vec<FlagView> {
    badge
        .flags
        .iter()
        .map(|flag| FlagView {
            text: escape_field(&flag.cell_text()),
            new_errors: matches!(flag, Flag::NewErrors(_)),
            bad: matches!(
                flag,
                Flag::NewErrors(_) | Flag::SmokeFailed(_) | Flag::AppLeftDown
            ),
            kind: match flag {
                Flag::NewErrors(_) => FlagKind::NewErrors,
                Flag::LogNotFullyObserved => FlagKind::LogNotFullyObserved,
                Flag::LogNotRecorded => FlagKind::LogNotRecorded,
                Flag::SmokeFailed(_) => FlagKind::SmokeFailed,
                Flag::SmokeNotRecorded => FlagKind::SmokeNotRecorded,
                Flag::AppLeftDown => FlagKind::AppLeftDown,
            },
        })
        .collect()
}

fn millis(ms: u128) -> u64 {
    u64::try_from(ms).unwrap_or(u64::MAX)
}

/// One part of a next-step line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Span {
    Text {
        text: String,
    },
    /// A `slip` command the user can copy. `copy_text` is exactly what goes
    /// on the clipboard: `prefix` followed by `command`.
    Command {
        prefix: String,
        command: String,
        copy_text: String,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Guidance {
    pub lines: Vec<Vec<Span>>,
    /// Some line names a command, but none could be offered for copying.
    pub copy_withheld: bool,
}

/// Splits each next-step line at its backtick spans. A span becomes a
/// copyable command only when it is a plain `slip` command and nothing
/// interpolated into the line could have made it up or changed its words:
/// a backtick or control character in the checkout path, or an env name
/// that is not one plain word, turns every span of the receipt into text.
pub fn guidance(lines: &[String], repo_root: &str, env: &str) -> Guidance {
    let copyable =
        !repo_root.contains('`') && escape_field(repo_root) == repo_root && is_plain_word(env);
    let prefix = format!("cd {} && ", shell_quote(repo_root));
    let spans = lines
        .iter()
        .map(|line| {
            line.split('`')
                .enumerate()
                .filter(|(_, part)| !part.is_empty())
                .map(|(index, part)| {
                    let inside = index % 2 == 1;
                    if inside && copyable && is_slip_command(part) {
                        Span::Command {
                            prefix: prefix.clone(),
                            command: part.into(),
                            copy_text: format!("{prefix}{part}"),
                        }
                    } else if inside {
                        Span::Text {
                            text: format!("`{part}`"),
                        }
                    } else {
                        Span::Text { text: part.into() }
                    }
                })
                .collect()
        })
        .collect();
    Guidance {
        lines: spans,
        copy_withheld: !copyable && lines.iter().any(|line| line.contains('`')),
    }
}

/// `slip <subcommand> [args]`, each arg letters, digits, `.`, `_` or `-`.
fn is_slip_command(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("slip ") else {
        return false;
    };
    let mut words = rest.split(' ');
    let subcommand = words.next().unwrap_or_default();
    !subcommand.is_empty()
        && subcommand
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-')
        && words.all(is_plain_word)
}

/// The command a refusal screen offers for an ssh connect failure, built from
/// the refusal kind and the configured alias, never from ssh's message. A
/// changed host key never gets one. An alias ssh could read as an option or
/// a shell could split is shown as text with nothing to copy.
pub fn refusal_command(kind: ConnectFailure, alias: &str) -> Option<Span> {
    let command = match kind {
        ConnectFailure::HostKeyUnknown => format!("ssh {alias}"),
        ConnectFailure::KeyNotLoaded => "ssh-add --apple-use-keychain".into(),
        ConnectFailure::HostKeyChanged | ConnectFailure::Other => return None,
    };
    if kind == ConnectFailure::HostKeyUnknown && (!is_plain_word(alias) || alias.starts_with('-')) {
        return Some(Span::Text {
            text: escape_field(&command),
        });
    }
    Some(Span::Command {
        prefix: String::new(),
        copy_text: command.clone(),
        command,
    })
}

/// `slip break-lock <env>` run in the checkout, when [`guidance`] would offer
/// it for copying.
pub fn break_lock_command(repo_root: &str, env: &str) -> Option<Span> {
    guidance(&[format!("`slip break-lock {env}`")], repo_root, env)
        .lines
        .into_iter()
        .flatten()
        .find(|span| matches!(span, Span::Command { .. }))
}

fn is_plain_word(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// POSIX single quotes: `'` becomes `'\''`.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{json, Value};
    use shipslip::receipt::overview;

    use super::*;
    use crate::fixture::{run, Fixture, NEW};

    const NOW: u128 = 1_000_000;

    fn load(fx: &Fixture) -> (MapView, Locators) {
        let overview = overview(&fx.receipts(), &fx.trust()).unwrap();
        let mut locators = Locators::default();
        (map(&overview, NOW, &mut locators), locators)
    }

    fn present(view: &MapView, row: usize, col: usize) -> &EnvCellView {
        match &view.rows[row].cells[col] {
            CellView::Present(cell) => cell,
            CellView::Absent => panic!("cell {row},{col} is absent"),
        }
    }

    #[test]
    fn empty_root_is_the_first_run_screen() {
        let fx = Fixture::new();
        let (view, _) = load(&fx);
        assert_eq!(view.screen, Screen::FirstRun);
        assert!(view.rows.is_empty() && view.problems.is_empty());
    }

    #[test]
    fn only_unreadable_receipts_is_the_none_readable_screen() {
        let fx = Fixture::new();
        let dir = fx.receipts().join("app").join("staging");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("5000-0000000a-0.json"), "not json").unwrap();
        let (view, _) = load(&fx);
        assert_eq!(view.screen, Screen::NoneReadable);
        assert_eq!(view.problems.len(), 1);
        assert_eq!(view.problems[0].kind, ProblemKindView::Receipt);
    }

    #[test]
    fn approval_without_receipts_is_a_map_with_an_approved_empty_cell() {
        let fx = Fixture::new();
        fx.approve("/work/app", "staging");
        let (view, _) = load(&fx);
        assert_eq!(view.screen, Screen::Map);
        let cell = present(&view, 0, 0);
        assert_eq!(cell.mark, MarkView::NoCode);
        assert_eq!(cell.words, "no deploy receipt on this Mac");
        assert_eq!(cell.evidence, ["approved before"]);
        assert!(cell.receipt.is_none() && cell.outcome.is_none());
    }

    #[test]
    fn recorded_success_opens_its_own_receipt() {
        let fx = Fixture::new();
        let path = fx.save(&run(1, "staging"));
        let (view, locators) = load(&fx);
        let cell = present(&view, 0, 0);
        assert_eq!(cell.mark, MarkView::Recorded);
        assert_eq!(cell.sha.as_deref(), Some(NEW));
        assert_eq!(cell.short_sha.as_deref(), Some("2222222"));
        assert_eq!(cell.age.as_deref(), Some("16m ago"));
        let outcome = cell.outcome.as_ref().unwrap();
        assert_eq!(
            (outcome.text.as_str(), outcome.tone),
            ("Succeeded", Tone::Good)
        );
        assert_eq!(cell.target.as_deref(), Some("web1:/srv/app"));
        assert!(!cell.needs_attention);
        assert!(cell.last_attempt.is_none());
        assert_eq!(locators.get(cell.receipt.unwrap()), Some(path.as_path()));
    }

    #[test]
    fn a_later_run_that_changed_nothing_is_the_last_attempt() {
        let fx = Fixture::new();
        let deploy = fx.save(&run(1, "staging"));
        let mut cancelled = run(2, "staging");
        cancelled["outcome"] = json!("CancelledBeforeChanges");
        cancelled["mutation_started"] = json!(false);
        for step in cancelled["steps"].as_array_mut().unwrap() {
            step["status"] = json!("NotStarted");
        }
        let later = fx.save(&cancelled);
        let (view, locators) = load(&fx);
        let cell = present(&view, 0, 0);
        assert_eq!(locators.get(cell.receipt.unwrap()), Some(deploy.as_path()));
        let last = cell.last_attempt.as_ref().unwrap();
        assert_eq!(locators.get(last.receipt), Some(later.as_path()));
        assert_eq!(
            (last.outcome.text.as_str(), last.outcome.tone),
            ("Cancelled before changes", Tone::Neutral)
        );
    }

    #[test]
    fn cells_without_a_code_run_show_the_newest_runs_outcome() {
        let fx = Fixture::new();
        let mut cancelled = run(1, "staging");
        cancelled["outcome"] = json!("CancelledBeforeChanges");
        cancelled["mutation_started"] = json!(false);
        for step in cancelled["steps"].as_array_mut().unwrap() {
            step["status"] = json!("NotStarted");
        }
        fx.save(&cancelled);
        let (view, _) = load(&fx);
        let cell = present(&view, 0, 0);
        assert_eq!(cell.mark, MarkView::NoCode);
        let outcome = cell.outcome.as_ref().unwrap();
        assert_eq!(
            (outcome.text.as_str(), outcome.tone),
            ("Cancelled before changes", Tone::Neutral)
        );
        assert!(cell.last_attempt.is_none());
    }

    #[test]
    fn maintenance_stop_after_a_deploy_flags_the_last_attempt() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let mut stop = run(2, "staging");
        stop["outcome"] = json!({"StoppedInMaintenance": "LockLost"});
        stop["mutation_started"] = json!(false);
        stop["app_left_down"] = json!(true);
        for step in stop["steps"].as_array_mut().unwrap() {
            step["status"] = json!("NotStarted");
        }
        fx.save(&stop);
        let (view, _) = load(&fx);
        let cell = present(&view, 0, 0);
        assert!(cell.needs_attention);
        let last = cell.last_attempt.as_ref().unwrap();
        assert_eq!(last.outcome.tone, Tone::Bad);
        assert!(last
            .flags
            .iter()
            .any(|flag| flag.text == "app may be in maintenance mode" && flag.bad));
        // The map words it as possible maintenance, not as a failed step.
        assert_eq!(last.outcome.kind, OutcomeKind::StoppedInMaintenance);
        assert!(last
            .flags
            .iter()
            .any(|flag| flag.kind == FlagKind::AppLeftDown));
        assert_eq!(cell.outcome.as_ref().unwrap().kind, OutcomeKind::Succeeded);
    }

    #[test]
    fn unknown_and_unfinished_receipts_keep_distinct_structured_outcomes() {
        for (unfinished, expected) in [
            (false, OutcomeKind::Unknown),
            (true, OutcomeKind::Unfinished),
        ] {
            let fx = Fixture::new();
            let mut receipt = run(1, "staging");
            receipt["outcome"] = json!({"Unknown": {"step": 1, "reason": "Connection lost"}});
            receipt["steps"][1]["status"] = json!("Unknown");
            if unfinished {
                receipt["status"] = json!("InProgress");
                receipt["finished_at_ms"] = Value::Null;
            }
            fx.save(&receipt);
            let (view, _) = load(&fx);
            let outcome = present(&view, 0, 0).outcome.as_ref().unwrap();
            assert_eq!(outcome.kind, expected);
            assert_eq!(outcome.tone, Tone::Bad);
        }
    }

    #[test]
    fn failed_step_after_the_fast_forward_is_incomplete() {
        let fx = Fixture::new();
        let mut failed = run(1, "staging");
        failed["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
        failed["steps"][1]["status"] = json!("Failed");
        failed["steps"][1]["exit_code"] = json!(1);
        fx.save(&failed);
        let (view, _) = load(&fx);
        let cell = present(&view, 0, 0);
        assert_eq!(cell.mark, MarkView::Incomplete);
        assert_eq!(cell.evidence, ["deploy incomplete"]);
        let outcome = cell.outcome.as_ref().unwrap();
        assert_eq!(
            (outcome.text.as_str(), outcome.tone),
            ("Failed at step 1", Tone::Bad)
        );
        assert!(cell.needs_attention);
    }

    #[test]
    fn failed_fast_forward_is_not_known_and_has_no_commit() {
        let fx = Fixture::new();
        let mut failed = run(1, "staging");
        failed["outcome"] = json!({"FailedAtStep": {"step": 0, "partial_update": false}});
        failed["steps"][0]["status"] = json!("Failed");
        failed["steps"][1]["status"] = json!("NotStarted");
        fx.save(&failed);
        let (view, _) = load(&fx);
        let cell = present(&view, 0, 0);
        assert_eq!(cell.mark, MarkView::NotKnown);
        assert!(cell.sha.is_none() && cell.age.is_none());
        assert_eq!(cell.words, "server code not known");
        assert!(cell.evidence.contains(&"fast-forward failed".to_string()));
    }

    #[test]
    fn apps_without_an_environment_get_an_absent_cell() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        fx.save(&run(2, "production"));
        let mut other = run(3, "staging");
        other["project"] = json!("blog");
        other["repo_root"] = json!("/work/blog");
        fx.save(&other);
        let (view, _) = load(&fx);
        assert_eq!(view.envs, ["production", "staging"]);
        let blog = view
            .rows
            .iter()
            .find(|row| row.repo_root == "/work/blog")
            .unwrap();
        assert!(matches!(blog.cells[0], CellView::Absent));
        assert!(matches!(blog.cells[1], CellView::Present(_)));
    }

    #[test]
    fn unreadable_newer_receipt_is_a_qualifier_with_its_own_id() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let dir = fx.receipts().join("app").join("staging");
        let broken = dir.join("5000-0000000a-0.json");
        fs::write(&broken, "not json").unwrap();
        let (view, locators) = load(&fx);
        let cell = present(&view, 0, 0);
        assert_eq!(
            cell.qualifiers,
            ["a newer receipt could not be read; this may be out of date"]
        );
        assert_eq!(
            locators.get(cell.stale_receipt.unwrap()),
            Some(broken.as_path())
        );
        assert!(cell.needs_attention);
        assert_eq!(view.problems[0].cells, [CellPos { row: 0, col: 0 }]);
    }

    #[test]
    fn server_and_path_text_is_escaped() {
        let fx = Fixture::new();
        let mut hostile = run(1, "staging");
        hostile["repo_root"] = json!("/work/\u{202e}ppa");
        hostile["target"]["ssh_alias"] = json!("web1\u{1b}[31m");
        hostile["target"]["path"] = json!("/srv/app\nrm -rf /");
        fx.save(&hostile);
        let (view, _) = load(&fx);
        let row = &view.rows[0];
        assert_eq!(row.repo_root, "/work/\\u{202e}ppa");
        assert!(!row.label.contains('\u{202e}'));
        let cell = present(&view, 0, 0);
        assert_eq!(
            cell.target.as_deref(),
            Some("web1\\x1b[31m:/srv/app\\x0arm -rf /")
        );
    }

    #[test]
    fn json_uses_camel_case_and_tagged_cells() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let mut other = run(2, "production");
        other["project"] = json!("blog");
        other["repo_root"] = json!("/work/blog");
        other["target"]["path"] = json!("/srv/blog");
        fx.save(&other);
        let (view, _) = load(&fx);
        let json: Value = serde_json::to_value(&view).unwrap();
        assert_eq!(json["screen"], "map");
        let app = &json["rows"][0];
        assert_eq!(app["pathMissing"], true);
        assert_eq!(app["cells"][0]["kind"], "absent");
        let cell = &app["cells"][1];
        assert_eq!(cell["kind"], "present");
        assert_eq!(cell["mark"], "recorded");
        assert_eq!(cell["shortSha"], "2222222");
        assert_eq!(cell["needsAttention"], false);
        assert_eq!(cell["outcome"]["tone"], "good");
    }

    #[test]
    fn load_error_is_escaped() {
        let error = ReceiptError::Invalid("bad \u{1b}]0;title\u{7}".into());
        assert!(!load_error(&error).contains('\u{1b}'));
    }
}

#[cfg(test)]
mod span_tests {
    use serde_json::{json, Value};
    use shipslip::receipt::read;

    use super::*;
    use crate::fixture::{run, Fixture};

    fn steps(fx: &Fixture, receipt: &Value) -> Vec<String> {
        read(&fx.save(receipt)).unwrap().next_steps()
    }

    fn commands(guidance: &Guidance) -> Vec<&str> {
        guidance
            .lines
            .iter()
            .flatten()
            .filter_map(|span| match span {
                Span::Command { copy_text, .. } => Some(copy_text.as_str()),
                Span::Text { .. } => None,
            })
            .collect()
    }

    fn unfinished(env: &str, repo_root: &str) -> Value {
        let mut receipt = run(1, env);
        receipt["status"] = json!("InProgress");
        receipt["phase"] = json!("BetweenSteps");
        receipt["finished_at_ms"] = json!(null);
        receipt["outcome"] = json!(null);
        receipt["repo_root"] = json!(repo_root);
        receipt
    }

    #[test]
    fn unfinished_run_offers_attach_from_its_checkout() {
        let fx = Fixture::new();
        let lines = steps(&fx, &unfinished("staging", "/work/app"));
        let guidance = guidance(&lines, "/work/app", "staging");
        assert!(!guidance.copy_withheld);
        assert_eq!(
            guidance.lines[0],
            [
                Span::Text {
                    text: "The run did not finish. If no deploy is running, resume it from /work/app with ".into()
                },
                Span::Command {
                    prefix: "cd '/work/app' && ".into(),
                    command: "slip attach staging".into(),
                    copy_text: "cd '/work/app' && slip attach staging".into(),
                },
                Span::Text { text: ".".into() },
            ]
        );
    }

    #[test]
    fn every_library_command_is_copyable() {
        let fx = Fixture::new();
        let mut failed = run(1, "staging");
        failed["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
        failed["steps"][1]["status"] = json!("Failed");
        let mut stopped = run(2, "staging");
        stopped["outcome"] = json!({"StoppedAfterStep": {"step": 0, "reason": "LockLost"}});
        stopped["steps"][1]["status"] = json!("NotStarted");
        let mut down = run(3, "staging");
        down["app_left_down"] = json!(true);
        let mut lines = Vec::new();
        for receipt in [unfinished("staging", "/work/app"), failed, stopped, down] {
            lines.extend(steps(&fx, &receipt));
        }
        let spans = guidance(&lines, "/work/app", "staging");
        assert_eq!(
            commands(&spans),
            [
                "cd '/work/app' && slip attach staging",
                "cd '/work/app' && slip from-step staging 1",
                "cd '/work/app' && slip deploy staging",
                "cd '/work/app' && slip from-step staging 1",
                "cd '/work/app' && slip up staging",
            ]
        );
        let backticks_left = spans.lines.iter().flatten().any(|span| match span {
            Span::Text { text } => text.contains('`'),
            Span::Command { .. } => false,
        });
        assert!(!backticks_left, "{spans:?}");
    }

    #[test]
    fn checkout_path_is_single_quoted() {
        let fx = Fixture::new();
        let repo = "/work/it's app";
        let lines = steps(&fx, &unfinished("staging", repo));
        assert_eq!(
            commands(&guidance(&lines, repo, "staging")),
            ["cd '/work/it'\\''s app' && slip attach staging"]
        );
    }

    #[test]
    fn backtick_in_checkout_path_makes_every_span_text() {
        let fx = Fixture::new();
        let repo = "/work/`slip deploy production`";
        let lines = steps(&fx, &unfinished("staging", repo));
        let spans = guidance(&lines, repo, "staging");
        assert!(commands(&spans).is_empty(), "{spans:?}");
    }

    #[test]
    fn control_characters_in_checkout_path_make_every_span_text() {
        let fx = Fixture::new();
        let repo = "/work/a\u{1b}]0;x\u{7}b";
        let lines = steps(&fx, &unfinished("staging", repo));
        assert!(commands(&guidance(&lines, repo, "staging")).is_empty());
    }

    #[test]
    fn env_with_shell_characters_is_never_copyable() {
        let fx = Fixture::new();
        for env in ["prod;rm", "prod$(id)", "pro duction", "prod`x`"] {
            let lines = steps(&fx, &unfinished(env, "/work/app"));
            let guidance = guidance(&lines, "/work/app", env);
            assert!(guidance.copy_withheld, "{env}");
            assert!(commands(&guidance).is_empty(), "{env}");
        }
    }

    #[test]
    fn command_spans_serialize_in_camel_case() {
        let span = Span::Command {
            prefix: "cd '/a' && ".into(),
            command: "slip up a".into(),
            copy_text: "cd '/a' && slip up a".into(),
        };
        let json = serde_json::to_value(&span).unwrap();
        assert_eq!(json["kind"], "command");
        assert_eq!(json["copyText"], "cd '/a' && slip up a");
    }

    #[test]
    fn lines_without_commands_withhold_nothing() {
        let lines = vec!["The fast-forward failed. Check the server checkout.".to_string()];
        assert!(!guidance(&lines, "/work/app", "prod;rm").copy_withheld);
    }

    #[test]
    fn unknown_host_key_offers_ssh_to_the_alias() {
        assert_eq!(
            refusal_command(ConnectFailure::HostKeyUnknown, "shop-prod"),
            Some(Span::Command {
                prefix: String::new(),
                command: "ssh shop-prod".into(),
                copy_text: "ssh shop-prod".into(),
            })
        );
    }

    #[test]
    fn key_not_loaded_offers_the_constant_ssh_add() {
        for alias in ["shop-prod", "-oProxyCommand=x", "a;b"] {
            assert_eq!(
                refusal_command(ConnectFailure::KeyNotLoaded, alias),
                Some(Span::Command {
                    prefix: String::new(),
                    command: "ssh-add --apple-use-keychain".into(),
                    copy_text: "ssh-add --apple-use-keychain".into(),
                })
            );
        }
    }

    #[test]
    fn changed_host_key_and_other_failures_offer_nothing() {
        for kind in [ConnectFailure::HostKeyChanged, ConnectFailure::Other] {
            assert_eq!(refusal_command(kind, "shop-prod"), None);
        }
    }

    #[test]
    fn unsafe_aliases_render_as_text() {
        for alias in [
            "",
            "-oProxyCommand=open",
            "-x",
            "shop prod",
            "shop;rm",
            "$(id)",
            "`id`",
            "user@host",
            "a\u{1b}]0;x\u{7}",
        ] {
            match refusal_command(ConnectFailure::HostKeyUnknown, alias) {
                Some(Span::Text { text }) => assert!(!text.contains('\u{1b}'), "{alias:?}"),
                other => panic!("{alias:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn break_lock_runs_in_the_checkout_and_needs_a_plain_env() {
        assert_eq!(
            break_lock_command("/work/my app", "production"),
            Some(Span::Command {
                prefix: "cd '/work/my app' && ".into(),
                command: "slip break-lock production".into(),
                copy_text: "cd '/work/my app' && slip break-lock production".into(),
            })
        );
        assert_eq!(break_lock_command("/work/app", "prod;rm"), None);
        assert_eq!(break_lock_command("/work/`x`", "production"), None);
    }

    #[test]
    fn receipt_text_naming_ssh_stays_text() {
        let lines = vec![
            "Run `ssh shop-prod` and compare the fingerprint.".to_string(),
            "Then `ssh-add --apple-use-keychain`.".to_string(),
        ];
        let guidance = guidance(&lines, "/work/app", "production");
        assert!(commands(&guidance).is_empty(), "{guidance:?}");
    }

    #[test]
    fn only_plain_slip_commands_pass() {
        for ok in [
            "slip attach staging",
            "slip from-step prod.eu 2",
            "slip logs a_b-c",
        ] {
            assert!(is_slip_command(ok), "{ok}");
        }
        for bad in [
            "slip",
            "slip ",
            "slip  attach x",
            "slip Attach x",
            "slip attach x ",
            "slip attach x;y",
            "slip attach 'x'",
            "rm -rf /",
            " slip attach x",
            "slipx attach",
        ] {
            assert!(!is_slip_command(bad), "{bad:?}");
        }
    }
}
