//! What the app keeps between commands: the latest map, the receipt files
//! behind its ids, and git comparisons already made. Every answer carries
//! the generation of the map it belongs to, so the UI can drop late ones.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use shipslip::config::LoadedConfig;
use shipslip::git::{Comparison, Git};
use shipslip::receipt::{compare_envs_with, ReceiptError};
use shipslip::records::{env_order, CodeState, EnvCell, ReceiptQueryError, Status as Overview};

use shipslip::records::{receipt_at, receipts, ReceiptListRow};

use crate::receipt_view::{slip, SlipView};
use crate::view::{self, compare_view, CellView, CompareView, Locators, MapView, ReceiptRowView};

#[derive(Default)]
pub struct AppState {
    inner: Mutex<Inner>,
    /// Keyed by checkout and both commits. Only definite answers are kept.
    memo: Mutex<HashMap<(String, String, String), Comparison>>,
}

#[derive(Default)]
struct Inner {
    generation: u64,
    snapshot: Option<Arc<Snapshot>>,
    loading: bool,
    reload_after: bool,
}

struct Snapshot {
    generation: u64,
    overview: Overview,
    /// Grows when a checkout's receipts are listed, so their ids open too.
    locators: Mutex<Locators>,
    map: Arc<MapView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loaded {
    pub generation: u64,
    pub map: Arc<MapView>,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LoadResult {
    Loaded(Loaded),
    /// A load was already running; it reads once more before it answers.
    Coalesced,
    /// The read failed. `current` is the map still shown, if any.
    Failed {
        message: String,
        current: Option<Loaded>,
    },
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CompareResult {
    Compared {
        generation: u64,
        row: usize,
        a: usize,
        b: usize,
        comparison: CompareView,
    },
    /// The map changed since the request.
    Stale,
    NotFound,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReceiptResult {
    Opened {
        generation: u64,
        id: usize,
        slip: SlipView,
    },
    Stale,
    NotFound,
}

/// Receipt files read for one checkout's list; a project's twins share them.
const RECEIPTS_READ: usize = 50;

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CheckoutReceipts {
    Listed {
        generation: u64,
        row: usize,
        receipts: Vec<ReceiptRowView>,
    },
    /// The project's receipts folder could not be read.
    Unreadable {
        message: String,
    },
    Stale,
    NotFound,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SelectionError {
    Stale,
    NotFound,
}

/// Compares with `git`, or says it cannot when none was found.
pub fn with_git(git: Option<&Git>) -> impl Fn(&Path, &str, &str) -> Comparison + '_ {
    move |root, a, b| match git {
        Some(git) => git.compare(root, a, b),
        None => Comparison::Unavailable("git not found".into()),
    }
}

impl AppState {
    pub fn journal_claims_changed(&self) -> Result<bool, ReceiptError> {
        let Some(snapshot) = self.inner().snapshot.clone() else {
            return Ok(false);
        };
        for (row, viewed) in snapshot.overview.repos.iter().zip(&snapshot.map.rows) {
            for run in row
                .envs
                .iter()
                .filter_map(|cell| cell.last_attempt.as_ref())
            {
                if !run.badge.outcome.unfinished {
                    continue;
                }
                let run_id = run.run_id.field();
                let was_claimed = viewed.cells.iter().any(|cell| {
                    matches!(cell, CellView::Present(cell) if cell.terminal_run.as_ref().is_some_and(|run| run.run_id == run_id))
                });
                if view::terminal_run(run)?.is_some() != was_claimed {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Resolves a UI position through the current map. Paths are never inputs
    /// to deployment commands, and an env must belong to the selected row.
    pub fn resolve_target(
        &self,
        generation: u64,
        row: usize,
        env: &str,
    ) -> Result<PathBuf, SelectionError> {
        let snapshot = self.current(generation).ok_or(SelectionError::Stale)?;
        let repo = snapshot
            .overview
            .repos
            .get(row)
            .ok_or(SelectionError::NotFound)?;
        if !repo.envs.iter().any(|cell| cell.env.raw() == env) {
            return Err(SelectionError::NotFound);
        }
        Ok(PathBuf::from(repo.repo_root.raw()))
    }

    /// Reads the map unless a read is already running; then that read runs
    /// once more when it finishes and answers with the newer map.
    pub fn load_map<R: Into<MapRead>>(
        &self,
        now_ms: impl Fn() -> u128,
        read_overview: impl Fn() -> Result<R, ReceiptQueryError>,
    ) -> LoadResult {
        {
            let mut inner = self.inner();
            if inner.loading {
                inner.reload_after = true;
                return LoadResult::Coalesced;
            }
            inner.loading = true;
        }
        let _release = ReleaseOnPanic(&self.inner);
        loop {
            let started = std::time::Instant::now();
            let built = read_overview().map(|read| {
                let MapRead {
                    overview,
                    needs_approval,
                } = read.into();
                let mut locators = Locators::default();
                let map = view::map(&overview, &needs_approval, now_ms(), &mut locators);
                (overview, locators, map)
            });
            if cfg!(debug_assertions) {
                eprintln!("load_map: {:?}", started.elapsed());
            }
            let mut inner = self.inner();
            let outcome = match built {
                Ok((overview, locators, map)) => {
                    inner.generation += 1;
                    let snapshot = Arc::new(Snapshot {
                        generation: inner.generation,
                        overview,
                        locators: Mutex::new(locators),
                        map: Arc::new(map),
                    });
                    inner.snapshot = Some(Arc::clone(&snapshot));
                    LoadResult::Loaded(loaded(&snapshot))
                }
                Err(error) => LoadResult::Failed {
                    message: view::load_error(&error),
                    current: inner.snapshot.as_deref().map(loaded),
                },
            };
            if !std::mem::take(&mut inner.reload_after) {
                inner.loading = false;
                return outcome;
            }
        }
    }

    pub fn compare(
        &self,
        generation: u64,
        row: usize,
        a: usize,
        b: usize,
        compare_commits: impl Fn(&Path, &str, &str) -> Comparison,
    ) -> CompareResult {
        let Some(snapshot) = self.current(generation) else {
            return CompareResult::Stale;
        };
        let columns = view::columns(&snapshot.overview);
        let (Some(repo), Some(env_a), Some(env_b)) = (
            snapshot.overview.repos.get(row),
            columns.get(a),
            columns.get(b),
        ) else {
            return CompareResult::NotFound;
        };
        let comparison = compare_envs_with(repo, env_a, env_b, |root, x, y| {
            self.memoized(root, x, y, &compare_commits)
        });
        CompareResult::Compared {
            generation,
            row,
            a,
            b,
            comparison: compare_view(env_a, env_b, &comparison),
        }
    }

    pub fn load_receipt(&self, generation: u64, id: usize) -> ReceiptResult {
        let Some(snapshot) = self.current(generation) else {
            return ReceiptResult::Stale;
        };
        let Some(path) = snapshot
            .locators
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .map(Path::to_path_buf)
        else {
            return ReceiptResult::NotFound;
        };
        ReceiptResult::Opened {
            generation,
            id,
            slip: slip(receipt_at(&path)),
        }
    }

    /// The checkout's own receipts, newest first, each with an id
    /// [`Self::load_receipt`] opens. A receipt no checkout can claim is left
    /// to the map's problems.
    pub fn checkout_receipts(
        &self,
        generation: u64,
        row: usize,
        receipts_root: &Path,
        now_ms: u128,
    ) -> CheckoutReceipts {
        let Some(snapshot) = self.current(generation) else {
            return CheckoutReceipts::Stale;
        };
        let Some(repo) = snapshot.overview.repos.get(row) else {
            return CheckoutReceipts::NotFound;
        };
        let listed = match &repo.project {
            Some(project) => {
                match receipts(
                    receipts_root,
                    project.raw(),
                    None,
                    Some(RECEIPTS_READ),
                    None,
                ) {
                    Ok(list) => list.rows,
                    Err(error) => {
                        return CheckoutReceipts::Unreadable {
                            message: view::load_error(&error),
                        }
                    }
                }
            }
            None => Vec::new(),
        };
        let own = listed.iter().filter_map(|listed| match listed {
            ReceiptListRow::Receipt { path, receipt, .. }
                if receipt.repo_root.raw() == repo.repo_root.raw() =>
            {
                Some((Path::new(path.raw()), receipt.as_ref()))
            }
            _ => None,
        });
        let mut locators = snapshot
            .locators
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        CheckoutReceipts::Listed {
            generation,
            row,
            receipts: view::receipt_rows(own, now_ms, &mut locators),
        }
    }

    /// The checkout behind a map row, for opening it on this Mac.
    pub fn resolve_checkout(&self, generation: u64, row: usize) -> Result<PathBuf, SelectionError> {
        let snapshot = self.current(generation).ok_or(SelectionError::Stale)?;
        let repo = snapshot
            .overview
            .repos
            .get(row)
            .ok_or(SelectionError::NotFound)?;
        Ok(PathBuf::from(repo.repo_root.raw()))
    }

    fn memoized(
        &self,
        root: &Path,
        a: &str,
        b: &str,
        compare_commits: impl Fn(&Path, &str, &str) -> Comparison,
    ) -> Comparison {
        let key = (root.to_string_lossy().into_owned(), a.into(), b.into());
        if let Some(hit) = self.memo().get(&key) {
            return hit.clone();
        }
        let result = compare_commits(root, a, b);
        if !matches!(result, Comparison::Unavailable(_)) {
            self.memo().insert(key, result.clone());
        }
        result
    }

    fn current(&self, generation: u64) -> Option<Arc<Snapshot>> {
        self.inner()
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.generation == generation)
            .cloned()
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn memo(&self) -> MutexGuard<'_, HashMap<(String, String, String), Comparison>> {
        self.memo.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn loaded(snapshot: &Snapshot) -> Loaded {
    Loaded {
        generation: snapshot.generation,
        map: Arc::clone(&snapshot.map),
    }
}

/// Lets later loads run if a read panics.
struct ReleaseOnPanic<'a>(&'a Mutex<Inner>);

impl Drop for ReleaseOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            inner.loading = false;
            inner.reload_after = false;
        }
    }
}

/// What one map load read: the overview, and the checkouts whose
/// `.shipslip.toml` has an environment without a current approval.
pub struct MapRead {
    pub overview: Overview,
    /// Checkout paths, as the overview spells them.
    pub needs_approval: BTreeSet<String>,
}

impl From<Overview> for MapRead {
    fn from(overview: Overview) -> Self {
        Self {
            overview,
            needs_approval: BTreeSet::new(),
        }
    }
}

/// Adds each checkout's configured environments that have no cell yet, as
/// never deployed, so "Review deploy…" can reach them, names checkouts no
/// receipt names after their config's project, and notes the checkouts that
/// need approval. A config that can't be read adds nothing and is not
/// judged; the environment's entry explains why when there is a cell.
pub fn with_checkout_configs(mut overview: Overview, trust_path: &Path) -> MapRead {
    let mut needs_approval = BTreeSet::new();
    for repo in overview.repos.iter_mut().filter(|repo| !repo.path_missing) {
        let Ok(config) = LoadedConfig::load(Path::new(repo.repo_root.raw()), None) else {
            continue;
        };
        if config.repo_root() != Path::new(repo.repo_root.raw()) {
            continue;
        }
        // Known only from the projects list or approvals: no receipt names it.
        if repo.project.is_none() {
            repo.label = config.project_name().into();
        }
        for env in config.environment_names() {
            if repo.envs.iter().any(|cell| cell.env.raw() == env) {
                continue;
            }
            repo.envs.push(EnvCell {
                env: env.into(),
                approved: false,
                approved_target: None,
                code: CodeState::NeverDeployed,
                last_attempt: None,
                code_run: None,
                stale_hint: None,
                target: None,
                newer_elsewhere: None,
            });
        }
        repo.envs
            .sort_by(|a, b| env_order(a.env.raw(), b.env.raw()));
        if !approved(&config, trust_path) {
            needs_approval.insert(repo.repo_root.raw().to_string());
        }
    }
    overview.repos.sort_by(|a, b| {
        a.label
            .cmp(&b.label)
            .then_with(|| a.repo_root.cmp(&b.repo_root))
    });
    MapRead {
        overview,
        needs_approval,
    }
}

/// Every configured environment has an approval that matches the config as it
/// reads now. An approvals file that can't be read approves nothing.
fn approved(config: &LoadedConfig, trust_path: &Path) -> bool {
    config
        .environment_names()
        .all(|env| config.trusted_target(env, trust_path).is_ok())
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use std::fs;

    use serde_json::json;
    use shipslip::receipt::ReceiptError;
    use shipslip::records::status as overview;

    use super::*;
    use crate::fixture::{run, Fixture};

    #[test]
    fn deploy_selection_is_bound_to_the_shown_map_and_environment() {
        let fx = Fixture::new();
        fx.approve("/work/one", "production");
        fx.approve("/work/two", "production");
        let state = AppState::default();
        let generation = match state.load_map(|| 0, || overview(&fx.receipts(), &fx.trust())) {
            LoadResult::Loaded(map) => map.generation,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            state.resolve_target(generation, 0, "production").unwrap(),
            Path::new("/work/one")
        );
        assert_eq!(
            state.resolve_target(generation, 1, "production").unwrap(),
            Path::new("/work/two")
        );
        assert_eq!(
            state.resolve_target(generation, 0, "staging"),
            Err(SelectionError::NotFound)
        );
        assert_eq!(
            state.resolve_target(generation, 5, "production"),
            Err(SelectionError::NotFound)
        );
        state.load_map(|| 0, || overview(&fx.receipts(), &fx.trust()));
        assert_eq!(
            state.resolve_target(generation, 0, "production"),
            Err(SelectionError::Stale)
        );
    }

    #[test]
    fn configured_envs_without_runs_or_approval_show_as_never_deployed() {
        let fx = Fixture::new();
        let root = fx.checkout(
            "shop",
            "[project]\nname = \"app\"\nstack = \"laravel\"\n\
             [env.production]\nssh = \"web1\"\npath = \"/srv/app\"\nbranch = \"main\"\nproduction = true\n\
             [env.demo]\nssh = \"web3\"\npath = \"/srv/app\"\nbranch = \"main\"\nproduction = false\n\
             [env.staging]\nssh = \"web2\"\npath = \"/srv/app\"\nbranch = \"main\"\nproduction = false\n",
        );
        let mut receipt = run(1, "production");
        receipt["repo_root"] = json!(root.to_string_lossy());
        fx.save(&receipt);

        let shown =
            with_checkout_configs(overview(&fx.receipts(), &fx.trust()).unwrap(), &fx.trust())
                .overview;
        let envs: Vec<_> = shown.repos[0].envs.iter().map(|c| c.env.raw()).collect();
        assert_eq!(envs, ["production", "staging", "demo"]);
        let staging = &shown.repos[0].envs[1];
        assert_eq!(staging.code, CodeState::NeverDeployed);
        assert!(!staging.approved);
        assert!(staging.last_attempt.is_none());

        // A checkout that is gone or has no config adds nothing.
        fs::remove_file(root.join(".shipslip.toml")).unwrap();
        let shown =
            with_checkout_configs(overview(&fx.receipts(), &fx.trust()).unwrap(), &fx.trust())
                .overview;
        assert_eq!(shown.repos[0].envs.len(), 1);
    }

    const TWO_ENVS: &str = "[project]\nname = \"app\"\nstack = \"laravel\"\n\
        [env.production]\nssh = \"web1\"\npath = \"/srv/app\"\nbranch = \"main\"\nproduction = true\n\
        [env.staging]\nssh = \"web2\"\npath = \"/srv/app\"\nbranch = \"main\"\nproduction = false\n";

    fn needs_approval(fx: &Fixture) -> bool {
        let state = AppState::default();
        let read = || {
            Ok(with_checkout_configs(
                overview(&fx.receipts(), &fx.trust())?,
                &fx.trust(),
            ))
        };
        match state.load_map(|| NOW, read) {
            LoadResult::Loaded(loaded) => loaded.map.rows[0].needs_approval,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_checkout_needs_approval_until_every_configured_environment_has_a_current_one() {
        let fx = Fixture::new();
        let root = fx.checkout("shop", TWO_ENVS);
        let mut receipt = run(1, "production");
        receipt["repo_root"] = json!(root.to_string_lossy());
        fx.save(&receipt);
        assert!(needs_approval(&fx), "nothing approved");

        fx.approve_config(&root, "production");
        assert!(needs_approval(&fx), "staging has no approval");

        fx.approve_config(&root, "staging");
        assert!(!needs_approval(&fx), "both approved as configured");

        let changed = TWO_ENVS.replace("ssh = \"web2\"", "ssh = \"web9\"");
        fs::write(root.join(".shipslip.toml"), changed).unwrap();
        assert!(needs_approval(&fx), "staging changed since its approval");
    }

    #[test]
    fn a_checkout_known_only_from_approvals_needs_approval_when_its_config_changed() {
        let fx = Fixture::new();
        let root = fx.checkout("shop", TWO_ENVS);
        fx.approve_at(&root.to_string_lossy(), "staging", "web2", "/srv/app");
        assert!(needs_approval(&fx));
    }

    #[test]
    fn a_checkout_added_in_the_app_shows_its_config_and_needs_approval() {
        let fx = Fixture::new();
        let root = fx.checkout("shop-checkout", TWO_ENVS);
        crate::add_project::add(&root, &fx.trust(), NOW).unwrap();
        assert!(needs_approval(&fx));

        let shown =
            with_checkout_configs(overview(&fx.receipts(), &fx.trust()).unwrap(), &fx.trust())
                .overview;
        let [repo] = shown.repos.as_slice() else {
            panic!("{:?}", shown.repos);
        };
        assert_eq!(
            repo.label.raw(),
            "app",
            "named by its config, not its folder"
        );
        let envs: Vec<_> = repo.envs.iter().map(|c| c.env.raw()).collect();
        assert_eq!(envs, ["production", "staging"]);
    }

    #[test]
    fn an_unreadable_approvals_file_approves_nothing() {
        let fx = Fixture::new();
        let root = fx.checkout("shop", TWO_ENVS);
        let mut receipt = run(1, "production");
        receipt["repo_root"] = json!(root.to_string_lossy());
        fx.save(&receipt);
        fx.approve_config(&root, "production");
        fx.approve_config(&root, "staging");
        fs::write(fx.trust(), "not json").unwrap();
        assert!(needs_approval(&fx));
    }

    #[test]
    fn a_checkout_without_a_readable_config_does_not_need_approval() {
        let fx = Fixture::new();
        // Receipts only: the checkout is not on this Mac.
        fx.save(&run(1, "staging"));
        assert!(!needs_approval(&fx));

        let fx = Fixture::new();
        let root = fx.checkout("shop", "not = [toml");
        let mut receipt = run(1, "staging");
        receipt["repo_root"] = json!(root.to_string_lossy());
        fx.save(&receipt);
        assert!(!needs_approval(&fx));
    }

    const NOW: u128 = 1_000_000;

    fn reader(fx: &Fixture) -> impl Fn() -> Result<Overview, ReceiptQueryError> + '_ {
        || overview(&fx.receipts(), &fx.trust())
    }

    fn generation(result: &LoadResult) -> u64 {
        match result {
            LoadResult::Loaded(loaded) => loaded.generation,
            other => panic!("not loaded: {other:?}"),
        }
    }

    /// A checkout on disk with staging and production on the same commit.
    fn two_envs(fx: &Fixture) {
        let root = fx.dir().to_string_lossy().into_owned();
        for (n, env) in [(1, "staging"), (2, "production")] {
            let mut receipt = run(n, env);
            receipt["repo_root"] = json!(root);
            receipt["target"]["path"] = json!(format!("/srv/{env}"));
            fx.save(&receipt);
        }
    }

    #[test]
    fn each_load_is_a_new_generation() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let state = AppState::default();
        assert_eq!(generation(&state.load_map(|| NOW, reader(&fx))), 1);
        assert_eq!(generation(&state.load_map(|| NOW, reader(&fx))), 2);
    }

    #[test]
    fn failed_load_keeps_and_returns_the_shown_map() {
        let state = AppState::default();
        let fail = || Err::<Overview, _>(ReceiptError::Invalid("root \u{1b}gone".into()).into());
        match state.load_map(|| NOW, fail) {
            LoadResult::Failed { message, current } => {
                assert!(current.is_none());
                assert!(!message.contains('\u{1b}'));
            }
            other => panic!("{other:?}"),
        }
        let fx = Fixture::new();
        state.load_map(|| NOW, reader(&fx));
        match state.load_map(|| NOW, fail) {
            LoadResult::Failed { current, .. } => assert_eq!(current.unwrap().generation, 1),
            other => panic!("{other:?}"),
        }
        assert!(matches!(state.load_receipt(1, 0), ReceiptResult::NotFound));
    }

    #[test]
    fn loads_during_a_load_coalesce_into_one_more_read() {
        let fx = Fixture::new();
        let state = AppState::default();
        let reads = AtomicUsize::new(0);
        let (started_tx, started_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let (state, reads, fx) = (&state, &reads, &fx);
        std::thread::scope(|scope| {
            let first = scope.spawn(move || {
                state.load_map(
                    || NOW,
                    || {
                        if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                            started_tx.send(()).unwrap();
                            go_rx.recv().unwrap();
                        }
                        overview(&fx.receipts(), &fx.trust())
                    },
                )
            });
            started_rx.recv().unwrap();
            assert!(matches!(
                state.load_map(|| NOW, reader(fx)),
                LoadResult::Coalesced
            ));
            assert!(matches!(
                state.load_map(|| NOW, reader(fx)),
                LoadResult::Coalesced
            ));
            go_tx.send(()).unwrap();
            assert_eq!(generation(&first.join().unwrap()), 2);
        });
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        assert_eq!(generation(&state.load_map(|| NOW, reader(fx))), 3);
    }

    #[test]
    fn a_panicking_read_does_not_block_later_loads() {
        let fx = Fixture::new();
        let state = AppState::default();
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            state.load_map(
                || NOW,
                || -> Result<Overview, ReceiptQueryError> { panic!("read failed") },
            )
        }));
        assert!(panicked.is_err());
        assert_eq!(generation(&state.load_map(|| NOW, reader(&fx))), 1);
    }

    #[test]
    fn compare_answers_only_for_the_current_map() {
        let fx = Fixture::new();
        two_envs(&fx);
        let state = AppState::default();
        state.load_map(|| NOW, reader(&fx));
        let same = |_: &Path, _: &str, _: &str| Comparison::Same;
        match state.compare(1, 0, 1, 0, same) {
            CompareResult::Compared {
                generation,
                row,
                a,
                b,
                comparison,
            } => {
                assert_eq!((generation, row, a, b), (1, 0, 1, 0));
                assert_eq!(
                    comparison.text,
                    "staging and production are on the same commit"
                );
                assert!(comparison.available);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            state.compare(0, 0, 1, 0, same),
            CompareResult::Stale
        ));
        assert!(matches!(
            state.compare(1, 5, 1, 0, same),
            CompareResult::NotFound
        ));
        assert!(matches!(
            state.compare(1, 0, 9, 0, same),
            CompareResult::NotFound
        ));
        state.load_map(|| NOW, reader(&fx));
        assert!(matches!(
            state.compare(1, 0, 1, 0, same),
            CompareResult::Stale
        ));
    }

    #[test]
    fn definite_comparisons_are_memoized_across_loads() {
        let fx = Fixture::new();
        two_envs(&fx);
        let state = AppState::default();
        let calls = AtomicUsize::new(0);
        let counted = |_: &Path, _: &str, _: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            Comparison::Ahead(3)
        };
        state.load_map(|| NOW, reader(&fx));
        state.compare(1, 0, 1, 0, counted);
        state.load_map(|| NOW, reader(&fx));
        state.compare(2, 0, 1, 0, counted);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unavailable_comparisons_are_not_memoized() {
        let fx = Fixture::new();
        two_envs(&fx);
        let state = AppState::default();
        let calls = AtomicUsize::new(0);
        let counted = |_: &Path, _: &str, _: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            Comparison::Unavailable("git timed out".into())
        };
        state.load_map(|| NOW, reader(&fx));
        state.compare(1, 0, 1, 0, counted);
        state.compare(1, 0, 1, 0, counted);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn without_git_compare_says_so() {
        let fx = Fixture::new();
        two_envs(&fx);
        let state = AppState::default();
        state.load_map(|| NOW, reader(&fx));
        match state.compare(1, 0, 1, 0, with_git(None)) {
            CompareResult::Compared { comparison, .. } => {
                assert_eq!(comparison.text, "cannot compare: git not found");
                assert!(!comparison.available);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn receipts_open_by_id_for_the_current_map_only() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let state = AppState::default();
        state.load_map(|| NOW, reader(&fx));
        match state.load_receipt(1, 0) {
            ReceiptResult::Opened {
                generation,
                id,
                slip,
            } => {
                assert_eq!((generation, id), (1, 0));
                assert!(matches!(slip, SlipView::Receipt(_)));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(state.load_receipt(1, 7), ReceiptResult::NotFound));
        state.load_map(|| NOW, reader(&fx));
        assert!(matches!(state.load_receipt(1, 0), ReceiptResult::Stale));
    }

    #[test]
    fn a_checkouts_receipts_list_newest_first_and_open_by_id() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let mut failed = run(2, "production");
        failed["outcome"] = json!({"FailedAtStep": {"step": 1, "partial_update": false}});
        failed["steps"][1]["status"] = json!("Failed");
        failed["steps"][1]["exit_code"] = json!(1);
        fx.save(&failed);
        let mut twin = run(3, "staging");
        twin["repo_root"] = json!("/work/twin");
        fx.save(&twin);
        let state = AppState::default();
        let generation = generation(&state.load_map(|| NOW, reader(&fx)));
        let rows = match state.checkout_receipts(generation, 0, &fx.receipts(), NOW) {
            CheckoutReceipts::Listed {
                generation: listed,
                row,
                receipts,
            } => {
                assert_eq!((listed, row), (generation, 0));
                receipts
            }
            other => panic!("{other:?}"),
        };
        let seen: Vec<_> = rows
            .iter()
            .map(|r| {
                (
                    r.env.as_str(),
                    r.outcome.text.as_str(),
                    r.short_sha.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("production", "Failed at step 1", "2222222"),
                ("staging", "Succeeded", "2222222")
            ]
        );
        assert_eq!(rows[0].age, "16m ago");
        match state.load_receipt(generation, rows[0].receipt) {
            ReceiptResult::Opened {
                slip: SlipView::Receipt(receipt),
                ..
            } => assert_eq!(receipt.env, "production"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_receipt_already_on_the_map_keeps_its_id_in_the_list() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let state = AppState::default();
        let loaded = match state.load_map(|| NOW, reader(&fx)) {
            LoadResult::Loaded(loaded) => loaded,
            other => panic!("{other:?}"),
        };
        let CellView::Present(cell) = &loaded.map.rows[0].cells[0] else {
            panic!("no cell");
        };
        let CheckoutReceipts::Listed { receipts, .. } =
            state.checkout_receipts(loaded.generation, 0, &fx.receipts(), NOW)
        else {
            panic!("not listed");
        };
        assert_eq!(Some(receipts[0].receipt), cell.receipt);
    }

    #[test]
    fn checkout_receipts_answer_only_for_the_current_map() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let state = AppState::default();
        state.load_map(|| NOW, reader(&fx));
        assert!(matches!(
            state.checkout_receipts(1, 4, &fx.receipts(), NOW),
            CheckoutReceipts::NotFound
        ));
        state.load_map(|| NOW, reader(&fx));
        assert!(matches!(
            state.checkout_receipts(1, 0, &fx.receipts(), NOW),
            CheckoutReceipts::Stale
        ));
    }

    #[test]
    fn a_checkout_known_only_from_approvals_lists_no_receipts() {
        let fx = Fixture::new();
        fx.approve("/work/app", "staging");
        let state = AppState::default();
        state.load_map(|| NOW, reader(&fx));
        assert!(matches!(
            state.checkout_receipts(1, 0, &fx.receipts(), NOW),
            CheckoutReceipts::Listed { receipts, .. } if receipts.is_empty()
        ));
    }

    #[test]
    fn an_unreadable_receipts_folder_is_said_not_listed_as_empty() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let state = AppState::default();
        state.load_map(|| NOW, reader(&fx));
        let not_a_folder = fx.dir().join("file");
        fs::write(&not_a_folder, "").unwrap();
        assert!(matches!(
            state.checkout_receipts(1, 0, &not_a_folder, NOW),
            CheckoutReceipts::Unreadable { .. }
        ));
    }

    #[test]
    fn checkout_paths_resolve_through_the_current_map() {
        let fx = Fixture::new();
        fx.save(&run(1, "staging"));
        let state = AppState::default();
        state.load_map(|| NOW, reader(&fx));
        assert_eq!(
            state.resolve_checkout(1, 0).unwrap(),
            Path::new("/work/app")
        );
        assert_eq!(state.resolve_checkout(1, 3), Err(SelectionError::NotFound));
        assert_eq!(state.resolve_checkout(2, 0), Err(SelectionError::Stale));
    }

    #[test]
    fn results_serialize_with_kind_tags() {
        let fx = Fixture::new();
        let state = AppState::default();
        let json = serde_json::to_value(state.load_map(|| NOW, reader(&fx))).unwrap();
        assert_eq!(json["kind"], "loaded");
        assert_eq!(json["generation"], 1);
        assert_eq!(json["map"]["screen"], "firstRun");
        assert_eq!(
            serde_json::to_value(CompareResult::Stale).unwrap(),
            json!({"kind": "stale"})
        );
    }
}
