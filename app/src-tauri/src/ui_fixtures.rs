//! Writes `app/src/fixtures.gen.ts`: real command results for every state
//! the UI draws, typed against `app/src/types.ts`. tsc fails when the types
//! drift from what Rust sends; this test fails when Rust changes what it
//! sends. The UI's tests and browser preview read the same file.
//!
//! Regenerate with `UPDATE_UI_FIXTURES=1 cargo test ui_fixtures`.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use shipslip::git::Comparison;
use shipslip::receipt::{overview, read, ReceiptError};

use crate::fixture::{run, Fixture};
use crate::receipt_view::slip;
use crate::state::{AppState, LoadResult, ReceiptResult};

/// 2026-10-12 08:53 UTC.
const NOW: u128 = 1_791_795_000_000;
const MINUTE: u128 = 60_000;
/// Stands in for the temp dir, so the file is the same on every machine.
const HOME: &str = "/Users/you";

struct Builder<'a> {
    fx: &'a Fixture,
    next: u64,
}

impl Builder<'_> {
    /// A successful deploy of `sha` to `app`'s `env`, `ago` minutes before NOW.
    fn deploy(&mut self, app: &str, env: &str, ago: u128, sha: &str) -> Value {
        self.next += 1;
        let mut receipt = run(self.next, env);
        let started = NOW - ago * MINUTE;
        receipt["project"] = json!(app);
        receipt["repo_root"] = json!(self.root(app));
        receipt["started_at_ms"] = json!(started);
        receipt["finished_at_ms"] = json!(started + 95_000);
        receipt["steps"][0]["started_at_ms"] = json!(started + 400);
        receipt["steps"][0]["finished_at_ms"] = json!(started + 1_600);
        receipt["steps"][1]["started_at_ms"] = json!(started + 1_700);
        receipt["steps"][1]["finished_at_ms"] = json!(started + 49_700);
        receipt["target_sha"] = json!(sha);
        receipt["target"]["ssh_alias"] = json!(format!("{env}1"));
        receipt["target"]["path"] = json!(format!("/var/www/{app}"));
        receipt["watch"] = watch(&[]);
        receipt["smoke"] = json!({"Passed": {"status": 200, "latency_ms": 380}});
        receipt
    }

    fn root(&self, app: &str) -> String {
        let root = self.fx.dir().join("code").join(app);
        fs::create_dir_all(&root).unwrap();
        root.to_string_lossy().into_owned()
    }

    fn save(&self, receipt: &Value) {
        self.fx.save(receipt);
    }
}

fn sha(c: char) -> String {
    c.to_string().repeat(40)
}

fn watch(groups: &[(&str, u64)]) -> Value {
    let groups: Vec<Value> = groups
        .iter()
        .map(|(exception, count)| {
            json!({
                "exception": exception, "file": "app/Http/Controllers/CheckoutController.php",
                "count": count, "overflow_variants": 0,
                "variants": [{
                    "signature": "s", "level": "ERROR", "count": count, "phase": "After",
                    "message": "SQLSTATE[42S22]: Column not found: 1054 Unknown column 'discount_code' in 'field list'",
                    "display_file_line": "app/Http/Controllers/CheckoutController.php:88"
                }]
            })
        })
        .collect();
    json!({
        "status": "Complete", "log_path": null, "duration_ms": 120000,
        "baseline_signatures": 14, "history_signatures": 37,
        "observed_lines": 412, "parsed_lines": 412, "dropped_view_lines": 0,
        "truncated_entries": 0, "new_errors": groups, "overflow_groups": 0,
        "overflow_signatures": 0, "warnings": []
    })
}

fn not_started(receipt: &mut Value, from: usize) {
    for step in receipt["steps"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .skip(from)
    {
        step["status"] = json!("NotStarted");
        step["exit_code"] = json!(null);
        step["started_at_ms"] = json!(null);
        step["finished_at_ms"] = json!(null);
    }
}

/// shop: all good, staging ahead and with new log errors. billing: failed
/// step on staging, an unreadable newer receipt on production. blog:
/// approved but never deployed, a failed fast-forward. api: a maintenance
/// stop after a deploy and a moved target. One checkout that is gone.
fn release_map(fx: &Fixture) {
    let mut b = Builder { fx, next: 0 };

    let mut shop = b.deploy("shop", "staging", 62, &sha('a'));
    shop["watch"] = watch(&[("QueryException", 9), ("TypeError", 2)]);
    // Server text is untrusted: a bidi override, a newline and markup.
    shop["watch"]["new_errors"][1]["variants"][0]["message"] =
        json!("Unsupported operand \u{202e}gnp.exe\n<b>bold</b>");
    shop["smoke"] = json!({"Passed": {"status": 200, "latency_ms": 412}});
    shop["target"]["production"] = json!(false);
    b.save(&shop);
    let mut prod = b.deploy("shop", "production", 4320, &sha('b'));
    prod["target"]["production"] = json!(true);
    b.save(&prod);
    let mut cancelled = b.deploy("shop", "production", 45, &sha('a'));
    cancelled["target"]["production"] = json!(true);
    cancelled["outcome"] = json!("CancelledBeforeChanges");
    cancelled["mutation_started"] = json!(false);
    cancelled["watch"] = json!(null);
    cancelled["smoke"] = json!(null);
    not_started(&mut cancelled, 0);
    b.save(&cancelled);

    let mut failed = b.deploy("billing", "staging", 120, &sha('c'));
    failed["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
    failed["steps"][1]["status"] = json!("Failed");
    failed["steps"][1]["exit_code"] = json!(1);
    failed["steps"][1]["output"] = json!([
        "   INFO  Running migrations.",
        "  2026_10_09_add_discount_code ......................... FAIL",
        "SQLSTATE[42000]: Syntax error or access violation",
        "\u{1b}[31m<img src=x onerror=alert(1)>\u{1b}[0m \u{202e}fdp.exe"
    ]);
    b.save(&failed);
    let receipt = b.deploy("billing", "production", 8640, &sha('d'));
    b.save(&receipt);
    let dir = fx.receipts().join("billing").join("production");
    fs::write(
        dir.join(format!("{}-0000beef-0.json", NOW - 60 * MINUTE)),
        "{\"version\": 1,",
    )
    .unwrap();

    let blog = b.root("blog");
    fx.approve_at(&blog, "staging", "staging1", "/var/www/blog");
    let mut ff = b.deploy("blog", "production", 30, &sha('e'));
    ff["outcome"] = json!({"FailedAtStep": {"step": 0, "partial_update": false}});
    ff["steps"][0]["status"] = json!("Failed");
    ff["steps"][0]["exit_code"] = json!(128);
    not_started(&mut ff, 1);
    b.save(&ff);

    let api = b.root("api");
    let receipt = b.deploy("api", "production", 1500, &sha('f'));
    b.save(&receipt);
    let mut stop = b.deploy("api", "production", 20, &sha('f'));
    stop["outcome"] = json!({"StoppedInMaintenance": "LockLost"});
    stop["mutation_started"] = json!(false);
    stop["app_left_down"] = json!(true);
    stop["maintenance_down_status"] = json!("Ok");
    stop["maintenance_down_exit_code"] = json!(0);
    stop["maintenance_down_started_at_ms"] = json!(stop["started_at_ms"].as_u64().unwrap() + 300);
    stop["maintenance_down_finished_at_ms"] =
        json!(stop["started_at_ms"].as_u64().unwrap() + 2_100);
    not_started(&mut stop, 0);
    b.save(&stop);
    fx.approve_at(&api, "production", "production2", "/var/www/api");
    let mut observed = b.deploy("api", "staging", 240, &sha('7'));
    observed["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
    observed["steps"][1]["status"] = json!("Failed");
    observed["steps"][1]["exit_code"] = json!(255);
    observed["server_head_at_end"] = json!(sha('8'));
    observed["tree_dirty"] = json!(true);
    b.save(&observed);

    let mut gone = b.deploy("shop", "staging", 5760, &sha('9'));
    gone["repo_root"] = json!(format!("{HOME}/tmp/shop-copy"));
    b.save(&gone);
}

/// Every answer the UI handles, keyed so the preview can serve them.
struct Fixtures {
    map: LoadResult,
    compares: BTreeMap<String, Value>,
    receipts: BTreeMap<String, ReceiptResult>,
    first_run: LoadResult,
    none_readable: LoadResult,
    failed: LoadResult,
    coalesced: LoadResult,
    slips: BTreeMap<&'static str, Value>,
    quit_runs: Vec<crate::deploy_session::QuitRun>,
    deploy: BTreeMap<&'static str, Value>,
}

fn build() -> (Fixtures, Vec<String>) {
    let fx = Fixture::new();
    release_map(&fx);
    let state = AppState::default();
    let read_map = || overview(&fx.receipts(), &fx.trust());
    let map = state.load_map(|| NOW, read_map);
    let LoadResult::Loaded(loaded) = &map else {
        panic!("{map:?}");
    };
    let (generation, rows, cols) = (
        loaded.generation,
        loaded.map.rows.len(),
        loaded.map.envs.len(),
    );

    let mut compares = BTreeMap::new();
    let commits = |_: &Path, a: &str, b: &str| match (a.as_bytes()[0], b.as_bytes()[0]) {
        (x, y) if x == y => Comparison::Same,
        (b'a', b'b') => Comparison::Ahead(4),
        (b'b', b'a') => Comparison::Behind(4),
        _ => Comparison::Diverged { a: 3, b: 1 },
    };
    for row in 0..rows {
        for a in 0..cols {
            for b in (0..cols).filter(|b| *b != a) {
                let result = state.compare(generation, row, a, b, commits);
                compares.insert(
                    format!("{row}:{a}:{b}"),
                    serde_json::to_value(result).unwrap(),
                );
            }
        }
    }
    let mut receipts = BTreeMap::new();
    for id in 0.. {
        match state.load_receipt(generation, id) {
            ReceiptResult::NotFound => break,
            result => receipts.insert(id.to_string(), result),
        };
    }

    let empty = Fixture::new();
    let first_run =
        AppState::default().load_map(|| NOW, || overview(&empty.receipts(), &empty.trust()));
    let broken = Fixture::new();
    let dir = broken.receipts().join("shop").join("staging");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{}-0000cafe-0.json", NOW)), "not json").unwrap();
    let none_readable =
        AppState::default().load_map(|| NOW, || overview(&broken.receipts(), &broken.trust()));
    let unreadable_root = || {
        Err(ReceiptError::Invalid(format!(
            "receipts root `{HOME}/Library/Application Support/Shipslip/receipts`: Permission denied (os error 13)"
        )))
    };
    let failed = AppState::default().load_map(|| NOW, unreadable_root);
    let coalesced = LoadResult::Coalesced;

    let mut slips = BTreeMap::new();
    let mut b = Builder {
        fx: &empty,
        next: 100,
    };
    let mut unfinished = b.deploy("shop", "staging", 3, &sha('a'));
    unfinished["status"] = json!("InProgress");
    unfinished["phase"] = json!("BetweenSteps");
    unfinished["finished_at_ms"] = json!(null);
    unfinished["outcome"] = json!(null);
    unfinished["steps"][1]["status"] = json!("Running");
    unfinished["steps"][1]["exit_code"] = json!(null);
    unfinished["steps"][1]["finished_at_ms"] = json!(null);
    slips.insert("unfinished", unfinished);
    let mut withheld = b.deploy("shop", "qa", 10, &sha('a'));
    withheld["target"]["env"] = json!("qa;eu");
    withheld["app_left_down"] = json!(true);
    slips.insert("copyWithheld", withheld);
    let mut slips: BTreeMap<&str, Value> = slips
        .into_iter()
        .map(|(name, receipt)| {
            let path = empty.save(&receipt);
            (name, serde_json::to_value(slip(read(&path))).unwrap())
        })
        .collect();
    let gone = empty.dir().join("missing.json");
    slips.insert("missing", serde_json::to_value(slip(read(&gone))).unwrap());
    let mut newer = b.deploy("shop", "staging", 1, &sha('a'));
    newer["version"] = json!(3);
    let newer_path = empty.save(&newer);
    slips.insert(
        "newerVersion",
        serde_json::to_value(slip(read(&newer_path))).unwrap(),
    );

    let homes = [fx.dir(), empty.dir(), broken.dir()]
        .iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    (
        Fixtures {
            map,
            compares,
            receipts,
            first_run,
            none_readable,
            failed,
            coalesced,
            slips,
            quit_runs: crate::deploy_session::sample_quit_runs(),
            deploy: crate::deploy_session::sample_deploy_views(),
        },
        homes,
    )
}

fn render(fixtures: &Fixtures, homes: &[String]) -> String {
    let json = |value: &dyn erased::Json| {
        let mut text = value.pretty();
        for home in homes {
            text = text.replace(home.as_str(), HOME);
        }
        text
    };
    let mut out = String::from(
        "// Generated by `UPDATE_UI_FIXTURES=1 cargo test ui_fixtures` in app/src-tauri.\n\
         // Do not edit: real command results, typed so tsc checks them against types.ts.\n\n\
         import type {\n  CommandError,\n  CompareResult,\n  DeployEntry,\n  DeployReview,\n  LiveRuns,\n  LoadResult,\n  PrepareProgress,\n  QuitRun,\n  ReceiptResult,\n  RecoveryView,\n  RunBatch,\n  RunInfo,\n  SlipView,\n  TrustReview,\n} from \"./types\";\n",
    );
    let item = |out: &mut String, name: &str, ty: &str, value: &dyn erased::Json| {
        let _ = write!(out, "\nexport const {name}: {ty} = {};\n", json(value));
    };
    item(&mut out, "map", "LoadResult", &fixtures.map);
    item(
        &mut out,
        "compares",
        "Record<string, CompareResult>",
        &fixtures.compares,
    );
    item(
        &mut out,
        "receipts",
        "Record<string, ReceiptResult>",
        &fixtures.receipts,
    );
    item(&mut out, "firstRun", "LoadResult", &fixtures.first_run);
    item(
        &mut out,
        "noneReadable",
        "LoadResult",
        &fixtures.none_readable,
    );
    item(&mut out, "failed", "LoadResult", &fixtures.failed);
    item(&mut out, "coalesced", "LoadResult", &fixtures.coalesced);
    item(
        &mut out,
        "slips",
        "Record<string, SlipView>",
        &fixtures.slips,
    );
    item(&mut out, "quitRuns", "QuitRun[]", &fixtures.quit_runs);
    let deploy = |name: &str| &fixtures.deploy[name];
    item(&mut out, "deployReview", "DeployReview", deploy("review"));
    item(&mut out, "runInfo", "RunInfo", deploy("runInfo"));
    let batches: BTreeMap<&str, &Value> =
        ["running", "watching", "failed", "succeeded", "lockTaken"]
            .into_iter()
            .map(|name| (name, deploy(name)))
            .collect();
    item(&mut out, "runBatches", "Record<string, RunBatch>", &batches);
    item(
        &mut out,
        "deployEntries",
        "DeployEntry[]",
        deploy("entries"),
    );
    item(
        &mut out,
        "prepareProgress",
        "PrepareProgress[]",
        deploy("progress"),
    );
    item(&mut out, "refusal", "CommandError", deploy("refusal"));
    item(&mut out, "lockHeld", "CommandError", deploy("lockHeld"));
    item(&mut out, "trustReview", "TrustReview", deploy("trust"));
    item(
        &mut out,
        "recoveries",
        "(RecoveryView | null)[]",
        deploy("recoveries"),
    );
    item(&mut out, "liveRuns", "LiveRuns", deploy("liveRuns"));
    out
}

mod erased {
    pub trait Json {
        fn pretty(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn pretty(&self) -> String {
            serde_json::to_string_pretty(self).unwrap()
        }
    }
}

#[test]
fn ui_fixtures() {
    let (fixtures, homes) = build();
    let text = render(&fixtures, &homes);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/fixtures.gen.ts");
    if std::env::var_os("UPDATE_UI_FIXTURES").is_some() {
        fs::write(&path, &text).unwrap();
        return;
    }
    let committed = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == text,
        "app/src/fixtures.gen.ts is out of date; run `UPDATE_UI_FIXTURES=1 cargo test ui_fixtures` in app/src-tauri"
    );
}
