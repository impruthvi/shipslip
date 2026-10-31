//! One receipt as the side pane shows it: the same facts as
//! `slip receipts show`, in fields. Server text is escaped here.

use serde::Serialize;
use shipslip::logs::escape_field;
use shipslip::receipt::{describe_watch, format_duration, strip_color};
use shipslip::records::{
    ReceiptQueryError, ReceiptRecord, Smoke, StepKind, StepRow, StepState, Watch,
};
use shipslip::{StepTimes, WatchResult, WatchStatus};

use crate::view::{flags, guidance, outcome, FlagView, Guidance, OutcomeView};

/// Error groups listed before "and N more groups".
const TOP_GROUPS: usize = 5;

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SlipView {
    Receipt(Box<ReceiptView>),
    /// The file is gone since the map was read.
    Missing,
    /// Written by a newer Shipslip.
    NewerVersion,
    Unreadable {
        message: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptView {
    pub run_id: String,
    /// The short headline, e.g. "Failed at step 2".
    pub outcome: OutcomeView,
    /// The receipt's own sentence for a finished run.
    pub summary: Option<String>,
    pub flags: Vec<FlagView>,
    pub project: String,
    pub repo_root: String,
    pub env: String,
    pub production: bool,
    /// `ssh_alias:path`.
    pub target: String,
    pub branch: String,
    pub plan: String,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub duration: Option<String>,
    pub started_by: Option<String>,
    pub from_sha: String,
    pub target_sha: String,
    /// What the server was on after a failed run, when it was read.
    pub server_head: Option<ServerHead>,
    pub commits: Vec<String>,
    pub steps: Vec<StepView>,
    pub watch: Option<WatchView>,
    pub smoke: Option<String>,
    pub notes: Vec<String>,
    pub next_steps: Guidance,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerHead {
    pub sha: Option<String>,
    pub dirty: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepView {
    /// `on` and `off` for maintenance mode, else the step number.
    pub index: String,
    pub command: String,
    pub status: StepStatusView,
    /// The library's label, e.g. "✗ failed".
    pub label: String,
    pub exit_code: Option<i32>,
    pub timing: Option<StepTiming>,
    pub output: Vec<String>,
}

/// What this Mac recorded of a step's run time. `None` when it recorded nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StepTiming {
    /// `duration` is e.g. "2m 44s".
    Took { duration: String, duration_ms: u64 },
    /// Sent at this time; the end was not seen.
    EndNotKnown { started_at_ms: u64 },
}

pub(crate) fn step_timing(times: StepTimes) -> Option<StepTiming> {
    match (times.started_at_ms, times.duration_ms()) {
        (_, Some(ms)) => Some(StepTiming::Took {
            duration: format_duration(ms),
            duration_ms: millis(ms),
        }),
        (Some(started), None) => Some(StepTiming::EndNotKnown {
            started_at_ms: millis(started),
        }),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatusView {
    Ok,
    Failed,
    Unknown,
    NotRun,
    Running,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchView {
    /// e.g. "complete".
    pub status: String,
    pub duration: String,
    pub lines_read: u64,
    pub log_path: String,
    /// Signatures compared against; `None` when the watch did not run.
    pub baseline: Option<(usize, usize)>,
    /// The largest new error groups, by count.
    pub groups: Vec<GroupView>,
    pub more_groups: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub count: u64,
    pub exception: String,
    pub file: Option<String>,
    pub variants: Vec<VariantView>,
    pub more_variants: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantView {
    pub count: u64,
    pub location: Option<String>,
    pub message: String,
}

pub fn slip(result: Result<ReceiptRecord, ReceiptQueryError>) -> SlipView {
    match result {
        Ok(receipt) => SlipView::Receipt(Box::new(receipt_view(&receipt))),
        Err(ReceiptQueryError::Missing) => SlipView::Missing,
        Err(ReceiptQueryError::NewerVersion { .. }) => SlipView::NewerVersion,
        Err(ReceiptQueryError::Unreadable { message }) => SlipView::Unreadable {
            message: message.field(),
        },
        Err(ReceiptQueryError::Ambiguous { ids }) => SlipView::Unreadable {
            message: escape_field(&format!("Several receipts match: {}", ids.join(", "))),
        },
    }
}

fn receipt_view(receipt: &ReceiptRecord) -> ReceiptView {
    let badge = receipt.badge();
    let target = &receipt.target;
    let steps = receipt.steps.iter().map(step).collect();
    ReceiptView {
        run_id: receipt.run_id.field(),
        outcome: outcome(&badge),
        summary: receipt
            .summary
            .as_ref()
            .filter(|_| !badge.outcome.unfinished)
            .map(|summary| summary.field()),
        flags: flags(&badge),
        project: receipt.project.field(),
        repo_root: receipt.repo_root.field(),
        env: target.env.field(),
        production: target.production,
        target: escape_field(&format!("{}:{}", target.ssh_alias.raw(), target.path.raw())),
        branch: target.branch.field(),
        plan: receipt.plan_text(),
        started_at_ms: millis(receipt.started_at_ms),
        finished_at_ms: receipt.finished_at_ms.map(millis),
        duration: receipt
            .finished_at_ms
            .map(|end| format_duration(end.saturating_sub(receipt.started_at_ms))),
        started_by: receipt.started_by.as_ref().map(|who| who.field()),
        from_sha: receipt.from_sha.field(),
        target_sha: receipt.target_sha.field(),
        server_head: receipt.ended_in_failure().then(|| ServerHead {
            sha: receipt.server_head_at_end.as_ref().map(|head| head.field()),
            dirty: receipt.tree_dirty == Some(true),
        }),
        commits: receipt.commits.iter().map(|c| c.field()).collect(),
        steps,
        watch: receipt
            .watch
            .as_ref()
            .map(|watch| record_watch_view(watch, target.log_path.raw())),
        smoke: receipt.smoke.as_ref().map(Smoke::text),
        notes: receipt.notes.iter().map(|note| note.field()).collect(),
        next_steps: guidance(&receipt.next_steps, receipt.repo_root.raw()),
    }
}

fn step(row: &StepRow) -> StepView {
    let (index, command) = match row.kind {
        StepKind::MaintenanceDown => (
            "on".to_string(),
            format!("Maintenance on: {}", row.command.field()),
        ),
        StepKind::Step { index } => (index.to_string(), row.command.field()),
        StepKind::MaintenanceUp => (
            "off".to_string(),
            format!("Maintenance off: {}", row.command.field()),
        ),
    };
    StepView {
        index,
        command,
        status: match row.status {
            StepState::Ok => StepStatusView::Ok,
            StepState::Failed => StepStatusView::Failed,
            StepState::Unknown => StepStatusView::Unknown,
            StepState::NotStarted | StepState::Pending => StepStatusView::NotRun,
            StepState::Running => StepStatusView::Running,
            StepState::Skipped => StepStatusView::Skipped,
        },
        label: row.status.label().into(),
        exit_code: row.exit_code,
        timing: step_timing(row.times()),
        output: row
            .output
            .iter()
            .map(|line| escape_field(&strip_color(line.raw())))
            .collect(),
    }
}

pub(crate) fn watch_view(watch: &WatchResult, default_log: &str) -> WatchView {
    record_watch_view(&Watch::from(watch), default_log)
}

fn record_watch_view(watch: &Watch, default_log: &str) -> WatchView {
    let mut groups: Vec<&shipslip::records::ErrorGroup> = watch.new_errors.iter().collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.count));
    let hidden = groups.len().saturating_sub(TOP_GROUPS) as u64;
    WatchView {
        status: describe_watch(watch.status).into(),
        duration: format_duration(u128::from(watch.duration_ms)),
        lines_read: watch.observed_lines,
        log_path: escape_field(
            watch
                .log_path
                .as_ref()
                .map(|path| path.raw())
                .unwrap_or(default_log),
        ),
        baseline: (watch.status != WatchStatus::NotRun)
            .then_some((watch.baseline_signatures, watch.history_signatures)),
        groups: groups
            .into_iter()
            .take(TOP_GROUPS)
            .map(|group| GroupView {
                count: group.count,
                exception: group.exception.field(),
                file: group.file.as_ref().map(|file| file.field()),
                variants: group
                    .variants
                    .iter()
                    .map(|variant| VariantView {
                        count: variant.count,
                        location: variant.display_file_line.as_ref().map(|line| line.field()),
                        message: variant.message.field(),
                    })
                    .collect(),
                more_variants: group.overflow_variants,
            })
            .collect(),
        more_groups: hidden + watch.overflow_groups,
        warnings: watch.warnings.iter().map(|w| w.field()).collect(),
    }
}

fn millis(ms: u128) -> u64 {
    u64::try_from(ms).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{json, Value};
    use shipslip::records::receipt_at as read;

    use super::*;
    use crate::fixture::{run, Fixture};
    use crate::view::{Span, Tone};

    fn open(fx: &Fixture, receipt: &Value) -> ReceiptView {
        match slip(read(&fx.save(receipt))) {
            SlipView::Receipt(view) => *view,
            other => panic!("not a receipt: {other:?}"),
        }
    }

    fn group(exception: &str, count: u64) -> Value {
        json!({
            "exception": exception, "file": "app/Foo.php", "count": count,
            "variants": [{
                "signature": "s", "level": "ERROR", "message": "boom\u{1b}[2J",
                "display_file_line": "app/Foo.php:12", "count": count, "phase": "After"
            }],
            "overflow_variants": 0
        })
    }

    #[test]
    fn successful_run_reads_like_receipts_show() {
        let fx = Fixture::new();
        let view = open(&fx, &run(1, "staging"));
        assert_eq!(view.run_id, "00000001-0");
        assert_eq!(view.outcome.text, "Succeeded");
        assert_eq!(view.summary.as_deref(), Some("Deploy run succeeded."));
        assert_eq!(view.outcome.tone, Tone::Good);
        assert_eq!((view.env.as_str(), view.production), ("staging", false));
        assert_eq!(view.target, "web1:/srv/app");
        assert_eq!(view.plan, "deploy");
        assert_eq!(view.duration.as_deref(), Some("500 ms"));
        assert!(view.server_head.is_none() && view.watch.is_none() && view.smoke.is_none());
        let steps: Vec<_> = view
            .steps
            .iter()
            .map(|s| (s.index.as_str(), s.status))
            .collect();
        assert_eq!(
            steps,
            [("0", StepStatusView::Ok), ("1", StepStatusView::Ok)]
        );
        assert_eq!(view.steps[0].label, "✓ ok");
        assert!(view.next_steps.lines.is_empty());
    }

    #[test]
    fn failed_step_shows_later_steps_not_run_and_the_server_head() {
        let fx = Fixture::new();
        let mut failed = run(1, "staging");
        failed["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
        failed["steps"][1]["status"] = json!("Failed");
        failed["steps"][1]["exit_code"] = json!(1);
        failed["steps"][1]["output"] = json!(["\u{1b}[31mSQLSTATE\u{1b}[0m denied\u{1b}]0;x\u{7}"]);
        failed["steps"].as_array_mut().unwrap().push(
            json!({"index": 2, "command": "php artisan optimize", "status": "NotStarted", "exit_code": null, "output": []}),
        );
        failed["server_head_at_end"] = json!("3333333333333333333333333333333333333333");
        failed["tree_dirty"] = json!(true);
        let view = open(&fx, &failed);
        assert_eq!(view.outcome.tone, Tone::Bad);
        assert_eq!(view.steps[1].status, StepStatusView::Failed);
        assert_eq!(view.steps[1].exit_code, Some(1));
        assert_eq!(view.steps[1].output, ["SQLSTATE denied\\x1b]0;x\\x07"]);
        assert_eq!(view.steps[2].label, "– not run");
        let head = view.server_head.unwrap();
        assert!(head.dirty);
        let copyable = view
            .next_steps
            .lines
            .iter()
            .flatten()
            .filter(|span| matches!(span, Span::Command { .. }))
            .count();
        assert_eq!(copyable, 2);
    }

    #[test]
    fn standalone_up_has_one_step_with_escaped_saved_output() {
        let fx = Fixture::new();
        let mut receipt = run(1, "staging");
        receipt["version"] = json!(2);
        receipt["kind"] = json!("up");
        receipt["maintenance_up_status"] = json!("Failed");
        receipt["maintenance_up_exit_code"] = json!(7);
        receipt["steps"] = json!([{
            "index": 0, "command": "php artisan up", "status": "Failed",
            "exit_code": 7, "output": ["up failed\u{1b}[2J"]
        }]);
        let view = open(&fx, &receipt);
        assert_eq!(view.steps.len(), 1);
        assert_eq!(view.steps[0].status, StepStatusView::Failed);
        assert_eq!(view.steps[0].output, ["up failed\\x1b[2J"]);
    }

    #[test]
    fn maintenance_steps_wrap_the_recipe() {
        let fx = Fixture::new();
        let mut receipt = run(1, "staging");
        receipt["maintenance_down_status"] = json!("Ok");
        receipt["maintenance_down_exit_code"] = json!(0);
        receipt["maintenance_up_status"] = json!("Failed");
        receipt["maintenance_up_exit_code"] = json!(1);
        let view = open(&fx, &receipt);
        let index: Vec<_> = view.steps.iter().map(|s| s.index.as_str()).collect();
        assert_eq!(index, ["on", "0", "1", "off"]);
        assert_eq!(view.steps[3].status, StepStatusView::Failed);
    }

    #[test]
    fn step_rows_carry_the_recorded_times() {
        let fx = Fixture::new();
        let mut receipt = run(1, "staging");
        receipt["steps"][0]["started_at_ms"] = json!(1_000);
        receipt["steps"][0]["finished_at_ms"] = json!(165_000);
        receipt["steps"][1]["status"] = json!("Unknown");
        receipt["steps"][1]["started_at_ms"] = json!(170_000);
        receipt["maintenance_down_status"] = json!("Ok");
        receipt["maintenance_down_exit_code"] = json!(0);
        receipt["maintenance_down_started_at_ms"] = json!(500);
        receipt["maintenance_down_finished_at_ms"] = json!(900);
        receipt["maintenance_up_status"] = json!("Unknown");
        receipt["maintenance_up_exit_code"] = json!(null);
        receipt["maintenance_up_started_at_ms"] = json!(180_000);
        let view = open(&fx, &receipt);
        let took = |duration: &str, duration_ms| {
            Some(StepTiming::Took {
                duration: duration.into(),
                duration_ms,
            })
        };
        let unseen = |started_at_ms| Some(StepTiming::EndNotKnown { started_at_ms });
        let timing: Vec<_> = view.steps.iter().map(|s| s.timing.clone()).collect();
        assert_eq!(
            timing,
            [
                took("400 ms", 400),
                took("2m 44s", 164_000),
                unseen(170_000),
                unseen(180_000)
            ]
        );
    }

    #[test]
    fn steps_without_times_carry_none() {
        let fx = Fixture::new();
        let view = open(&fx, &run(1, "staging"));
        assert!(view.steps.iter().all(|s| s.timing.is_none()));
        let json = serde_json::to_value(StepTiming::EndNotKnown { started_at_ms: 5 }).unwrap();
        assert_eq!(json, json!({"kind": "endNotKnown", "startedAtMs": 5}));
    }

    #[test]
    fn log_watch_lists_the_five_largest_groups_then_the_rest() {
        let fx = Fixture::new();
        let mut receipt = run(1, "staging");
        let groups: Vec<Value> = (1..=7).map(|n| group(&format!("E{n}"), n)).collect();
        receipt["watch"] = json!({
            "status": "Complete", "log_path": null, "duration_ms": 60000,
            "baseline_signatures": 4, "history_signatures": 9,
            "observed_lines": 120, "parsed_lines": 120, "dropped_view_lines": 0,
            "truncated_entries": 0, "new_errors": groups, "overflow_groups": 3,
            "overflow_signatures": 0, "warnings": []
        });
        let view = open(&fx, &receipt);
        let watch = view.watch.unwrap();
        let shown: Vec<_> = watch.groups.iter().map(|g| g.exception.as_str()).collect();
        assert_eq!(shown, ["E7", "E6", "E5", "E4", "E3"]);
        assert_eq!(watch.more_groups, 2 + 3);
        assert_eq!(watch.baseline, Some((4, 9)));
        assert_eq!(watch.log_path, "storage/logs/laravel.log");
        assert_eq!(watch.groups[0].variants[0].message, "boom\\x1b[2J");
        assert_eq!(view.flags[0].text, "10 new error groups after this run");
        assert!(view.flags[0].new_errors);
    }

    #[test]
    fn deleted_newer_and_corrupt_files_are_their_own_states() {
        let fx = Fixture::new();
        let path = fx.save(&run(1, "staging"));
        fs::remove_file(&path).unwrap();
        assert!(matches!(slip(read(&path)), SlipView::Missing));

        let mut newer = run(2, "staging");
        newer["version"] = json!(99);
        assert!(matches!(
            slip(read(&fx.save(&newer))),
            SlipView::NewerVersion
        ));

        let corrupt = path.with_file_name("3000-00000003-0.json");
        fs::write(&corrupt, "{\"version\": 1, \u{202e}").unwrap();
        match slip(read(&corrupt)) {
            SlipView::Unreadable { message } => assert!(!message.contains('\u{202e}')),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn json_tags_the_slip_kind() {
        let fx = Fixture::new();
        let view = slip(read(&fx.save(&run(1, "staging"))));
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["kind"], "receipt");
        assert_eq!(json["runId"], "00000001-0");
        assert_eq!(json["nextSteps"]["copyWithheld"], false);
        let missing = serde_json::to_value(SlipView::Missing).unwrap();
        assert_eq!(missing, json!({"kind": "missing"}));
    }
}
