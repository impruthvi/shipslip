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
use shipslip::receipt::ReceiptError;
use shipslip::records::status as overview;

use shipslip::records::receipt_at as read;

use crate::fixture::{run, Fixture};
use crate::receipt_view::slip;
use crate::state::{AppState, CheckoutReceipts, LoadResult, ReceiptResult};

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

/// A successful run from step 2 of 3 on the deployed commit.
fn from_step(b: &mut Builder, ago: u128) -> Value {
    let mut receipt = b.deploy("shop", "staging", ago, &sha('a'));
    receipt["run_plan"] = json!({"FromStep": 2});
    receipt["target"]["steps"] = json!([
        "composer install --no-dev",
        "php artisan migrate --force",
        "php artisan optimize"
    ]);
    let started = receipt["started_at_ms"].as_u64().unwrap();
    receipt["steps"] = json!([
        {"index": 1, "command": "composer install --no-dev", "status": "Skipped", "exit_code": null, "output": []},
        {"index": 2, "command": "php artisan migrate --force", "status": "Ok", "exit_code": 0, "output": [],
         "started_at_ms": started + 400, "finished_at_ms": started + 9_400},
        {"index": 3, "command": "php artisan optimize", "status": "Ok", "exit_code": 0, "output": [],
         "started_at_ms": started + 9_500, "finished_at_ms": started + 12_000}
    ]);
    receipt
}

/// The map cell after a run-from-step that followed a full deploy.
fn from_step_cell() -> Value {
    let fx = Fixture::new();
    let mut b = Builder { fx: &fx, next: 400 };
    let deploy = b.deploy("shop", "staging", 30, &sha('a'));
    b.save(&deploy);
    let partial = from_step(&mut b, 5);
    b.save(&partial);
    let overview = overview(&fx.receipts(), &fx.trust()).unwrap();
    let view = crate::view::map(
        &overview,
        &Default::default(),
        NOW,
        &mut crate::view::Locators::default(),
    );
    serde_json::to_value(&view.rows[0].cells[0]).unwrap()
}

fn smoke_cells() -> BTreeMap<&'static str, Value> {
    let mut cells = BTreeMap::new();
    for (name, smoke, prior_deploy) in [
        (
            "passed",
            json!({"Passed": {"status": 200, "latency_ms": 5}}),
            true,
        ),
        (
            "failed",
            json!({"Failed": {"status": 503, "reason": "HTTP 503"}}),
            true,
        ),
        (
            "upOnlyFailed",
            json!({"Failed": {"status": 503, "reason": "HTTP 503"}}),
            false,
        ),
    ] {
        let fx = Fixture::new();
        let mut b = Builder { fx: &fx, next: 0 };
        if prior_deploy {
            let mut deploy = b.deploy("shop", "staging", 10, &sha('a'));
            deploy["smoke"] = json!({"Failed": {"status": 503, "reason": "HTTP 503"}});
            b.save(&deploy);
        }
        let mut up = b.deploy("shop", "staging", 2, &sha('a'));
        up["version"] = json!(2);
        up["kind"] = json!("up");
        up["steps"] = json!([{"index": 0, "command": "php artisan up", "status": "Ok", "exit_code": 0, "output": []}]);
        up["maintenance_up_status"] = json!("Ok");
        up["maintenance_up_done"] = json!(true);
        up["watch"] = json!(null);
        up["smoke"] = smoke;
        b.save(&up);
        let overview = overview(&fx.receipts(), &fx.trust()).unwrap();
        let view = crate::view::map(
            &overview,
            &Default::default(),
            NOW,
            &mut crate::view::Locators::default(),
        );
        cells.insert(name, serde_json::to_value(&view.rows[0].cells[0]).unwrap());
    }
    cells
}

/// Every answer the UI handles, keyed so the preview can serve them.
struct Fixtures {
    terminal_map: LoadResult,
    map: LoadResult,
    wide_map: LoadResult,
    approval_map: LoadResult,
    added_map: LoadResult,
    removable_map: LoadResult,
    folders: BTreeMap<&'static str, crate::add_project::FolderView>,
    setup_map: LoadResult,
    config_texts: BTreeMap<&'static str, crate::add_project::ConfigText>,
    ssh_aliases: crate::ssh_alias::SshAliases,
    host_blocks: BTreeMap<&'static str, crate::ssh_alias::HostBlock>,
    alias_checks: BTreeMap<&'static str, crate::ssh_alias::AliasCheck>,
    connection_checks: BTreeMap<&'static str, crate::connection_check::CheckView>,
    watch_results: LoadResult,
    compares: BTreeMap<String, Value>,
    checkout_receipts: Vec<CheckoutReceipts>,
    receipts: BTreeMap<String, ReceiptResult>,
    first_run: LoadResult,
    none_readable: LoadResult,
    failed: LoadResult,
    coalesced: LoadResult,
    slips: BTreeMap<&'static str, Value>,
    quit_runs: Vec<crate::deploy_session::QuitRun>,
    deploy: BTreeMap<&'static str, Value>,
    smoke_cells: BTreeMap<&'static str, Value>,
    from_step_cell: Value,
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
    // Listed first, so the ids they add are among the receipts below.
    let checkout_receipts: Vec<_> = (0..rows)
        .map(|row| state.checkout_receipts(generation, row, &fx.receipts(), NOW))
        .collect();
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
        Err::<shipslip::records::Status, _>(ReceiptError::Invalid(format!(
            "receipts root `{HOME}/Library/Application Support/Shipslip/receipts`: Permission denied (os error 13)"
        )).into())
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
    let terminal_path = empty.save(&unfinished);
    let journal = shipslip::receipt::ReceiptJournal::load(&terminal_path).unwrap();
    journal.claim().unwrap();
    let terminal_map =
        AppState::default().load_map(|| NOW, || overview(&empty.receipts(), &empty.trust()));
    drop(journal);
    slips.insert("unfinished", unfinished);
    let mut gaps = b.deploy("shop", "staging", 2, &sha('a'));
    gaps["steps"][1]["finished_at_ms"] = json!(null);
    gaps["steps"][1]["result_seen_at_ms"] = json!(NOW - 60_000);
    gaps["steps"][1]["connection_gaps"] = json!([
        { "lost_at_ms": NOW - 100_000, "reattached_at_ms": NOW - 90_000, "detail": "raw ssh detail" },
        { "lost_at_ms": NOW - 80_000, "reattached_at_ms": NOW - 60_000, "detail": "raw ssh detail" }
    ]);
    gaps["status"] = json!("InProgress");
    gaps["phase"] = json!("MaintenanceUp");
    gaps["outcome"] = json!(null);
    gaps["finished_at_ms"] = json!(null);
    gaps["app_left_down"] = json!(true);
    gaps["maintenance_up_status"] = json!("Unknown");
    gaps["maintenance_up_started_at_ms"] = json!(NOW - 50_000);
    gaps["maintenance_up_connection_gaps"] = json!([
        { "lost_at_ms": NOW - 40_000, "reattached_at_ms": null, "detail": "raw ssh detail" }
    ]);
    slips.insert("connectionGaps", gaps);
    let mut withheld = b.deploy("shop", "qa", 10, &sha('a'));
    withheld["target"]["env"] = json!("qa;eu");
    withheld["app_left_down"] = json!(true);
    slips.insert("copyWithheld", withheld);
    slips.insert("fromStep", from_step(&mut b, 4));
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

    let watch_fx = Fixture::new();
    let mut watch_builder = Builder {
        fx: &watch_fx,
        next: 200,
    };
    for (name, status, skipped) in [
        ("watchInterrupted", "Partial", false),
        ("watchUnreadable", "Unavailable", false),
        ("watchNoFile", "NoLogSeen", false),
        ("watchStopped", "Cancelled", false),
        ("watchSkipped", "NotRun", true),
    ] {
        let mut receipt = watch_builder.deploy(name, "staging", 1, &sha('a'));
        receipt["watch"] = serde_json::to_value(shipslip::WatchResult::not_run()).unwrap();
        receipt["watch"]["status"] = json!(status);
        receipt["watch"]["duration_ms"] = json!(if skipped { 0 } else { 120_000 });
        if skipped {
            receipt["checks_skip_reason"] = json!("AppLeftInMaintenance");
        }
        let path = watch_fx.save(&receipt);
        slips.insert(name, serde_json::to_value(slip(read(&path))).unwrap());
    }
    let watch_results =
        AppState::default().load_map(|| NOW, || overview(&watch_fx.receipts(), &watch_fx.trust()));

    // More environments than fit across, in no particular order on disk.
    let wide_fx = Fixture::new();
    let mut b = Builder {
        fx: &wide_fx,
        next: 300,
    };
    for env in ["uat", "demo", "staging", "qa", "production", "sandbox"] {
        let deploy = b.deploy("shop", env, 30, &sha('a'));
        b.save(&deploy);
    }
    let wide_map =
        AppState::default().load_map(|| NOW, || overview(&wide_fx.receipts(), &wide_fx.trust()));

    // shop: both environments approved as configured. docs: approved for
    // an older path, so its config changed since.
    let approval_fx = Fixture::new();
    let mut b = Builder {
        fx: &approval_fx,
        next: 500,
    };
    let config = |name: &str| {
        format!(
            "[project]\nname = \"{name}\"\nstack = \"laravel\"\n\
             [env.production]\nssh = \"production1\"\npath = \"/var/www/{name}\"\nbranch = \"main\"\nproduction = true\n\
             [env.staging]\nssh = \"staging1\"\npath = \"/var/www/{name}\"\nbranch = \"main\"\nproduction = false\n"
        )
    };
    for (app, approve) in [("shop", true), ("docs", false)] {
        let root = approval_fx.checkout(app, &config(app));
        for env in ["staging", "production"] {
            let mut deploy = b.deploy(app, env, 30, &sha('a'));
            deploy["repo_root"] = json!(root.to_string_lossy());
            b.save(&deploy);
            if approve {
                approval_fx.approve_config(&root, env);
            } else {
                approval_fx.approve_at(
                    &root.to_string_lossy(),
                    env,
                    &format!("{env}1"),
                    "/var/www/old",
                );
            }
        }
    }
    let approval_trust = approval_fx.trust();
    let approval_map = AppState::default().load_map(
        || NOW,
        || {
            Ok(crate::state::with_checkout_configs(
                overview(&approval_fx.receipts(), &approval_trust)?,
                &approval_trust,
            ))
        },
    );

    // crm: added in the app with its config, staging approved, production
    // not, nothing deployed.
    let added_fx = Fixture::new();
    let crm = added_fx.checkout("crm", &config("crm"));
    let added_trust = added_fx.trust();
    added_fx.approve_config(&crm, "staging");
    let crm_folder = crate::add_project::inspect(&crm, &added_trust, None);
    crate::add_project::add(&crm, &added_trust, NOW).unwrap();
    let added_map = AppState::default().load_map(
        || NOW,
        || {
            Ok(crate::state::with_checkout_configs(
                overview(&added_fx.receipts(), &added_trust)?,
                &added_trust,
            ))
        },
    );
    // shop: added in the app with its config, nothing approved or deployed.
    let removable_fx = Fixture::new();
    let shop = removable_fx.checkout("shop", &config("shop"));
    let removable_trust = removable_fx.trust();
    crate::add_project::add(&shop, &removable_trust, NOW).unwrap();
    let removable_map = AppState::default().load_map(
        || NOW,
        || {
            Ok(crate::state::with_checkout_configs(
                overview(&removable_fx.receipts(), &removable_trust)?,
                &removable_trust,
            ))
        },
    );
    let loose = added_fx.dir().join("Downloads");
    fs::create_dir_all(&loose).unwrap();
    let bare = added_fx.checkout("api", "");
    fs::remove_file(bare.join(".shipslip.toml")).unwrap();
    let invalid = added_fx.checkout("billing", "[project]\nname = \"\"\n");
    let folders = BTreeMap::from([
        ("config", crm_folder),
        (
            "notGit",
            crate::add_project::inspect(&loose, &added_trust, None),
        ),
        (
            "newConfig",
            crate::add_project::inspect(&bare, &added_trust, None),
        ),
        (
            "unreadable",
            crate::add_project::inspect(&invalid, &added_trust, None),
        ),
        ("cancelled", crate::add_project::FolderView::Cancelled),
    ]);

    // api: chosen in the sheet with no config, then Finish later.
    let setup_fx = Fixture::new();
    let api = setup_fx.checkout("api", "");
    fs::remove_file(api.join(".shipslip.toml")).unwrap();
    fs::write(
        api.join("package.json"),
        r#"{"scripts":{"build":"vite build"}}"#,
    )
    .unwrap();
    fs::write(api.join("yarn.lock"), "").unwrap();
    let setup_trust = setup_fx.trust();
    crate::add_project::add(&api, &setup_trust, NOW).unwrap();
    let setup_map = AppState::default().load_map(
        || NOW,
        || {
            Ok(crate::state::with_checkout_configs(
                overview(&setup_fx.receipts(), &setup_trust)?,
                &setup_trust,
            ))
        },
    );
    let answers = |env: &str, path: &str| shipslip::config::InitAnswers {
        env: env.into(),
        ssh_alias: "api-staging".into(),
        path: path.into(),
        branch: "main".into(),
        production: false,
        maintenance: true,
        smoke_url: Some("https://staging.api.test/up".into()),
    };
    let config_texts = BTreeMap::from([
        (
            "rendered",
            crate::add_project::render(&api, &answers("staging", "/var/www/api"))
                .unwrap()
                .0,
        ),
        (
            "invalid",
            crate::add_project::render(&api, &answers("env.staging", "var/www/api"))
                .unwrap()
                .0,
        ),
    ]);

    let (ssh_aliases, host_blocks, alias_checks) = ssh_alias_fixtures(setup_fx.dir());

    let approval_home = fs::canonicalize(approval_fx.dir()).unwrap();
    let added_home = fs::canonicalize(added_fx.dir()).unwrap();
    let setup_home = fs::canonicalize(setup_fx.dir()).unwrap();
    let removable_home = fs::canonicalize(removable_fx.dir()).unwrap();
    let homes = [
        removable_home.as_path(),
        removable_fx.dir(),
        approval_home.as_path(),
        approval_fx.dir(),
        added_home.as_path(),
        added_fx.dir(),
        setup_home.as_path(),
        setup_fx.dir(),
        fx.dir(),
        empty.dir(),
        broken.dir(),
        watch_fx.dir(),
        wide_fx.dir(),
    ]
    .iter()
    .map(|dir| dir.to_string_lossy().into_owned())
    .collect();
    (
        Fixtures {
            terminal_map,
            map,
            wide_map,
            approval_map,
            added_map,
            removable_map,
            folders,
            setup_map,
            config_texts,
            ssh_aliases,
            host_blocks,
            alias_checks,
            connection_checks: connection_check_fixtures(),
            watch_results,
            compares,
            checkout_receipts,
            receipts,
            first_run,
            none_readable,
            failed,
            coalesced,
            slips,
            quit_runs: crate::deploy_session::sample_quit_runs(),
            deploy: crate::deploy_session::sample_deploy_views(),
            smoke_cells: smoke_cells(),
            from_step_cell: from_step_cell(),
        },
        homes,
    )
}

/// The connection check for `api-staging`: running, every line passing,
/// both host-key refusals, and a wrong branch.
fn connection_check_fixtures() -> BTreeMap<&'static str, crate::connection_check::CheckView> {
    use shipslip::connection_check::{CheckEvent, Failed, Outcome, Passed, Probe, Request};
    use shipslip::ssh_aliases::Resolved;
    use shipslip::transport::ConnectFailure;

    let request = Request {
        alias: "api-staging".into(),
        path: "/var/www/api".into(),
        branch: "main".into(),
        log_path: "storage/logs/laravel.log".into(),
        log_daily: false,
    };
    let done = |probe, outcome| CheckEvent::Finished(probe, outcome);
    let alias = done(
        Probe::Alias,
        Outcome::Pass(Passed::Alias(Resolved {
            host_name: "203.0.113.7".into(),
            user: "forge".into(),
            port: 22,
        })),
    );
    let passing = |branch: Outcome| {
        vec![
            alias.clone(),
            done(Probe::Connect, Outcome::Pass(Passed::Connected)),
            done(Probe::Checkout, Outcome::Pass(Passed::Checkout)),
            done(Probe::Branch, branch),
            done(
                Probe::Artisan,
                Outcome::Pass(Passed::Artisan("Laravel Framework 11.9.2".into())),
            ),
            done(
                Probe::Log,
                Outcome::Pass(Passed::Log("storage/logs/laravel.log".into())),
            ),
        ]
    };
    let refused = |failure, message: &str| {
        let mut events = vec![
            alias.clone(),
            done(
                Probe::Connect,
                Outcome::Fail(Failed::Connect {
                    failure,
                    message: message.into(),
                }),
            ),
        ];
        events.extend(Probe::ALL[2..].iter().map(|&p| done(p, Outcome::NotRun)));
        events
    };
    let replay = |events: &[CheckEvent]| crate::connection_check::replay(&request, events);
    BTreeMap::from([
        ("waiting", replay(&[])),
        (
            "running",
            replay(&[
                alias.clone(),
                done(Probe::Connect, Outcome::Pass(Passed::Connected)),
                CheckEvent::Started(Probe::Checkout),
            ]),
        ),
        ("pass", replay(&passing(Outcome::Pass(Passed::Branch)))),
        (
            "wrongBranch",
            replay(&passing(Outcome::Fail(Failed::OtherBranch(
                "feature/search".into(),
            )))),
        ),
        (
            "unknownKey",
            replay(&refused(
                ConnectFailure::HostKeyUnknown,
                "No ED25519 host key is known for web and you have requested strict checking.\n\
                 Host key verification failed.",
            )),
        ),
        (
            "changedKey",
            replay(&refused(
                ConnectFailure::HostKeyChanged,
                "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\nHost key verification failed.",
            )),
        ),
        (
            "unverifiedKey",
            replay(&refused(
                ConnectFailure::HostKeyUnverified,
                "Host key verification failed.",
            )),
        ),
    ])
}

type SshAliasFixtures = (
    crate::ssh_alias::SshAliases,
    BTreeMap<&'static str, crate::ssh_alias::HostBlock>,
    BTreeMap<&'static str, crate::ssh_alias::AliasCheck>,
);

/// The ssh alias step: two aliases in a config, two keys, and the Host
/// block and Check again states for "None of these fit".
fn ssh_alias_fixtures(dir: &Path) -> SshAliasFixtures {
    use shipslip::ssh_aliases::{HostDraft, ResolveError, Resolved};

    let ssh = dir.join("ssh");
    fs::create_dir_all(&ssh).unwrap();
    fs::write(
        ssh.join("config"),
        "Host *\n  ServerAliveInterval 30\n\nHost production1\n\nHost staging1 *.internal\n",
    )
    .unwrap();
    for key in ["id_ed25519", "id_ed25519.pub", "id_rsa"] {
        fs::write(ssh.join(key), "").unwrap();
    }
    let resolved = |host_name: &str, user: &str| Resolved {
        host_name: host_name.into(),
        user: user.into(),
        port: 22,
    };
    let aliases = crate::ssh_alias::aliases(&ssh.join("config"), &ssh, |alias| {
        Ok(match alias {
            "production1" => resolved("203.0.113.10", "deploy"),
            _ => resolved("203.0.113.20", "deploy"),
        })
    });
    let draft = HostDraft {
        alias: "api".into(),
        host_name: "203.0.113.7".into(),
        user: "forge".into(),
        identity_file: "~/.ssh/id_ed25519".into(),
        port: "22".into(),
    };
    let invalid = HostDraft {
        host_name: String::new(),
        port: "x".into(),
        ..draft.clone()
    };
    let host_blocks = BTreeMap::from([
        ("block", crate::ssh_alias::host_block(&draft)),
        ("invalid", crate::ssh_alias::host_block(&invalid)),
    ]);
    let alias_checks = BTreeMap::from([
        (
            "matches",
            crate::ssh_alias::check(&draft, &[], |_| Ok(resolved("203.0.113.7", "forge"))),
        ),
        (
            "differs",
            crate::ssh_alias::check(&draft, &[], |alias| Ok(resolved(alias, "you"))),
        ),
        (
            "failed",
            crate::ssh_alias::check(&draft, &[], |_| {
                Err(ResolveError::Failed(
                    "/Users/you/.ssh/config line 4: Bad configuration option: hostnme".into(),
                ))
            }),
        ),
    ]);
    (aliases, host_blocks, alias_checks)
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
         import type {\n  AliasCheck,\n  CellView,\n  CheckView,\n  CheckoutReceipts,\n  CommandError,\n  CompareResult,\n  ConfigText,\n  DeployEntry,\n  DeployReview,\n  FolderView,\n  HostBlock,\n  LiveRuns,\n  LoadResult,\n  PrepareProgress,\n  QuitRun,\n  ReceiptResult,\n  RecoveryReview,\n  RecoveryView,\n  RunBatch,\n  RunInfo,\n  SlipView,\n  SshAliases,\n  TrustReview,\n} from \"./types\";\n",
    );
    let item = |out: &mut String, name: &str, ty: &str, value: &dyn erased::Json| {
        let _ = write!(out, "\nexport const {name}: {ty} = {};\n", json(value));
    };
    item(&mut out, "map", "LoadResult", &fixtures.map);
    item(&mut out, "wideMap", "LoadResult", &fixtures.wide_map);
    item(
        &mut out,
        "approvalMap",
        "LoadResult",
        &fixtures.approval_map,
    );
    item(&mut out, "addedMap", "LoadResult", &fixtures.added_map);
    item(
        &mut out,
        "removableMap",
        "LoadResult",
        &fixtures.removable_map,
    );
    item(
        &mut out,
        "folders",
        "Record<string, FolderView>",
        &fixtures.folders,
    );
    item(&mut out, "setupMap", "LoadResult", &fixtures.setup_map);
    item(
        &mut out,
        "configTexts",
        "Record<string, ConfigText>",
        &fixtures.config_texts,
    );
    item(&mut out, "sshAliases", "SshAliases", &fixtures.ssh_aliases);
    item(
        &mut out,
        "connectionChecks",
        "Record<string, CheckView>",
        &fixtures.connection_checks,
    );
    item(
        &mut out,
        "hostBlocks",
        "Record<string, HostBlock>",
        &fixtures.host_blocks,
    );
    item(
        &mut out,
        "aliasChecks",
        "Record<string, AliasCheck>",
        &fixtures.alias_checks,
    );
    item(
        &mut out,
        "terminalMap",
        "LoadResult",
        &fixtures.terminal_map,
    );
    item(
        &mut out,
        "smokeCells",
        "Record<string, CellView>",
        &fixtures.smoke_cells,
    );
    item(
        &mut out,
        "fromStepCell",
        "CellView",
        &fixtures.from_step_cell,
    );
    item(
        &mut out,
        "compares",
        "Record<string, CompareResult>",
        &fixtures.compares,
    );
    item(
        &mut out,
        "checkoutReceipts",
        "CheckoutReceipts[]",
        &fixtures.checkout_receipts,
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
        "watchResults",
        "LoadResult",
        &fixtures.watch_results,
    );
    item(
        &mut out,
        "slips",
        "Record<string, SlipView>",
        &fixtures.slips,
    );
    item(&mut out, "quitRuns", "QuitRun[]", &fixtures.quit_runs);
    let deploy = |name: &str| &fixtures.deploy[name];
    item(&mut out, "deployReview", "DeployReview", deploy("review"));
    item(
        &mut out,
        "terminalEntry",
        "DeployEntry",
        deploy("terminalEntry"),
    );
    item(&mut out, "runInfo", "RunInfo", deploy("runInfo"));
    item(
        &mut out,
        "pickerEntry",
        "DeployEntry",
        deploy("pickerEntry"),
    );
    item(&mut out, "gapRunInfo", "RunInfo", deploy("gapRunInfo"));
    item(
        &mut out,
        "attachedRunInfo",
        "RunInfo",
        deploy("attachedRunInfo"),
    );
    item(
        &mut out,
        "attachReview",
        "RecoveryReview",
        deploy("attachReview"),
    );
    item(&mut out, "upReview", "RecoveryReview", deploy("upReview"));
    item(
        &mut out,
        "upReviewAllRan",
        "RecoveryReview",
        deploy("upReviewAllRan"),
    );
    let batches: BTreeMap<&str, &Value> = [
        "attached",
        "running",
        "stepGap",
        "stepReattached",
        "watchGap",
        "watchReattached",
        "watching",
        "failed",
        "failedHeadline",
        "failedWatching",
        "stoppedHeadline",
        "stoppedWatching",
        "succeededWatching",
        "succeeded",
        "lockTaken",
    ]
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
    item(
        &mut out,
        "terminalLockHeld",
        "CommandError",
        deploy("terminalLockHeld"),
    );
    item(&mut out, "staleLock", "CommandError", deploy("staleLock"));
    item(&mut out, "trustReview", "TrustReview", deploy("trust"));
    item(
        &mut out,
        "recoveries",
        "(RecoveryView | null)[]",
        deploy("recoveries"),
    );
    item(
        &mut out,
        "unfinishedRecoveries",
        "Record<string, RecoveryView | null>",
        deploy("unfinishedRecoveries"),
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
fn log_watch_fixture_wording() {
    let (fixtures, _) = build();
    for (name, expected) in [
        (
            "watchInterrupted",
            "log watch interrupted · entries may be missing",
        ),
        ("watchUnreadable", "log could not be read"),
        ("watchNoFile", "no log file yet"),
        ("watchStopped", "log watch stopped early"),
        (
            "watchSkipped",
            "log watch skipped · app left in maintenance",
        ),
    ] {
        assert_eq!(fixtures.slips[name]["watch"]["status"], expected, "{name}");
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
