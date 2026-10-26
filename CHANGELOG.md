# Changelog

All notable changes to Shipslip are listed here. Shipslip follows
[Semantic Versioning](https://semver.org/); before 1.0, a minor release may
change the library API.

## [Unreleased]

## [0.9.1] - 2026-10-26

### Changed

- `deploy`, `rerun` and `from-step` say what they are waiting on before the
  preview: "Connecting to <alias>…", "Fetching <branch>…", "Checking the
  server…" and "Taking the deploy lock…". Runs that fetch with a temporary
  GitHub token show only the connecting line.
- Declining a confirmation, or mistyping a production environment name, now
  exits 1 instead of 0 for `deploy`, `rerun`, `from-step`, `attach`, `up` and
  `break-lock` (and when a GitHub token account is not approved). Nothing is
  changed on the server in those cases; scripts can now tell that apart from
  a completed run.
- `slip receipts` lists an Up run as "brought up at <commit>" instead of a
  same-commit `FROM → TARGET`.

## [0.9.0] - 2026-10-21

### Changed

- `slip up` is a recorded run. It saves a receipt before sending
  `php artisan up`, keeps the deploy lock's heartbeat while it runs, and if
  interrupted, `slip attach` follows the command again without sending it
  twice. `slip receipts` and `slip status` show it ("app brought up"); it
  never changes the code a release map cell shows, and a successful `up`
  clears the "app may be in maintenance mode" flag of the deploy before it.
- Up receipts keep the maintenance command's exit status and last 200 output
  lines. Recovery observes the saved command without launching it again,
  and a successful Up preserves recovery guidance for an earlier failed deploy.
- Receipts gain `kind` (`deploy` or `up`). Deploy receipts are unchanged and
  stay schema version 1; `up` receipts are version 2, which releases before
  0.9.0 list as unreadable instead of misreading them.
- Library: `bring_app_up` takes the receipts root and returns `RunEvents`
  and an `ExecutionHandle` like `execute`; `BringUpError` keeps only the
  errors before the command starts. `attach` resumes `up` receipts.
  `Receipt::summary` and `Receipt::plan_text` give kind-aware wording;
  `AttemptSummary::kind`.
- Library: the trust check moved from the CLI into the library. `prepare`,
  `prepare_with_plan`, `prepare_with_github_token`, `attach`, `bring_app_up`
  and `break_lock` take a `TrustedTarget`, which only
  `LoadedConfig::trusted_target` builds, after checking the environment's
  settings match the ones approved with `slip trust`. A front-end can no
  longer run an unapproved recipe. CLI messages and exit codes are unchanged.
- Library: `attach` refuses a run whose project, checkout or deploy settings
  differ from the approved ones (`AttachError::SettingsChanged`);
  `check_resumable` runs the same check before asking to resume.
- A deploy step's output line longer than 16 KiB is cut and ends in
  `… [line truncated, N bytes]`, so a step printing a huge line without a
  newline can't exhaust memory.
- When the screen falls behind a step's output, Shipslip reads the step's log
  from the server more slowly instead of buffering it; no line is lost, and
  the lock release and receipt never wait on the screen.
- The deploy preview lists at most 2,000 commits and ends with
  `and N more commits`; receipts keep the count (`more_commits`).
- Library: `execute`, `execute_recorded` and `attach` return `RunEvents`
  (read with `recv`). `Transport::run` takes a bounded `mpsc::Sender` and a
  line limit (`STEP_LINE_LIMIT`, `SCRIPT_LINE_LIMIT`); callers must read the
  output while the command runs. `Preview::more_commits`.

### Added

- Library: `TransportError::connect_failure` identifies unknown host keys,
  changed host keys and rejected public keys as `ConnectFailure` values.
  Unrecognized connection errors stay `Other`, and a lost connection is
  never treated as a refusal. Original diagnostics are preserved.
- Library: `prepare_with_progress` calls back with each `PreparePhase`
  (`Fetching`, `Checking`, `Locking`) as it starts, so a front-end can say
  what a slow preview is waiting on. The preflight script prints `@phase`
  markers for it.
- Library: `interrupt` module (`Stage`, `next_stage`, `Interrupt`,
  `on_interrupt`) says what Ctrl-C or closing a window means at each stage of
  a followed run, and `config::trust_changes` lists the settings to review
  before trusting an environment. Both moved from the CLI so the desktop app
  can share them; CLI behavior is unchanged.

## [0.8.0] - 2026-10-21

### Added

- `slip logs ENV ID` lists other entries that share the latest entry's
  `request_id`, `trace_id`, `correlation_id`, `job_id` or `batch_id`, read
  from the top level of Laravel's context or extra JSON. Errors without an ID
  get a one-line tip on adding one with `Context::add`.
- Library: `request_id`, `related`, `render_related`, `RequestId`, `Related`
  and `Correlation`.
- `slip logs ENV` counts every entry in the window by level in its header,
  regardless of `--level` and `--grep`.
- `slip logs ENV ID` folds runs of three or more vendor stack frames; the first
  frame and app frames always show. `--full` shows every frame.
- `slip logs ENV ID --md` prints the detail as Markdown with its environment,
  timezone, window, coverage, baseline and filters. Log text is escaped or
  fenced so it never renders as links, images or HTML.
- Library: `level_counts`, `coverage_incomplete`, `render_detail_md` and
  `DetailFilter`; `Snapshot.read_at` holds when the server was read.

### Changed

- `slip logs` summary and variant lines show the log message without the
  JSON context Laravel appends, so `Payment failed {"exception":"[object] …`
  reads `Payment failed`. The detail view still shows the whole entry.
- Counts are lower bounds (`≥`, `at least`) when a channel has lines slip
  couldn't parse, not only when it was partly read. Variant counts follow
  their group.
- With `--grep`, an empty summary says `No entries match --grep …`.
- Library: `render_summary` takes the `--grep` text and `render_detail` takes
  a `full` flag.

## [0.7.1] - 2026-10-14

### Added

- A macOS desktop app, built locally from `app/`, shows the release map from
  `slip status` and opens each cell's receipt beside it. It reads the same
  files on this Mac and never contacts a server. See
  [Desktop app (local build)](README.md#desktop-app-local-build).
- Library: `stale_hint_text` and `newer_elsewhere_text` give a release-map
  cell's out-of-date warnings in words, `describe_smoke` gives a smoke check
  result in words, and `compare_envs_with` compares two environments with a
  caller-supplied commit comparison, e.g. to answer from a cache. `slip status`
  and `slip receipts show` print the same as before.

## [0.7.0] - 2026-10-12

### Changed

- The maintenance flag reads `⛔ app may be in maintenance mode`: it is set
  whenever `php artisan down` may have started or `php artisan up` did not
  finish, including when `down` itself failed.
- `slip status` warns under a cell when its approved config deploys to a
  different `ssh_alias:path` than the newest receipt, until a run uses the
  new target.
- Library: `EnvCell` gains `approved_target` and `target_moved`, and
  `target_moved_text` gives the warning in words. `config::approved_envs`
  returns each approved env's `ssh_alias:path`.

## [0.6.2] - 2026-10-11

### Changed

- `slip status` says `no deploy receipt on this Mac` for an environment with
  no receipt, instead of claiming it was never deployed.
- `slip status` and `slip receipts show` add a next step when the log showed
  new error groups after a run: `slip logs ENV` shows the log.
- Bidirectional-text control characters in remote output (such as U+202E)
  are shown escaped, like other control characters, so they cannot reorder
  what is printed.
- Library: the release map wording is public (`code_text`, `sentence`,
  `comparison_text`, `Flag::cell_text`, `age`), so the CLI and other
  front-ends show the same facts. `EnvCell` gains `code_run` and
  `needs_attention`, `AttemptSummary` gains the receipt `path`, and each
  `Problem` has a `kind` and the cells it may make out of date.

## [0.6.1] - 2026-10-10

### Changed

- `slip status` says how a run ended next to a commit it observed on the
  server (`observed 2fa3f2490cb2 after run 18dcdde4 failed at step 2`), and
  lists the same next steps as `slip receipts show` under a run that needs
  attention, such as `slip from-step` or `slip up`.
- Library: `CodeState::Observed` carries the run's outcome and
  `AttemptSummary` carries its `next_steps`.

## [0.6.0] - 2026-10-09

### Added

- `slip status` shows the last recorded code of every app and environment
  from local receipts and approvals, from any folder and without contacting a
  server. It only names a commit a receipt backs (fast-forward succeeded,
  rerun checked it out, or server commit read after a failed step) and
  otherwise says the code is not known, and warns when another checkout ran
  later on the same server path. `--compare ENV ENV` counts commits between
  two environments in the local checkout.
- Library: `receipt::overview`, `receipt::compare_envs`, `Git::compare`, and
  `Receipt::next_steps`, so other front-ends can show the same map and advice.

### Changed

- Log signature history moved from `receipts/signatures/` to a `signatures/`
  directory beside `receipts/`, so a project named `signatures` cannot clash
  with it. The old file is still read and is moved on the next deploy; old
  folders are left in place.

## [0.5.3] - 2026-10-08

### Changed

- Log watch and `slip logs` find the source file of errors thrown in
  `database/`, `routes/`, `config/`, and `resources/` (for example a failing
  migration), not only `app/`; `vendor/` is still skipped. Such errors now
  group by file, so they may be reported as new once.
- The `slip receipts` list names its time zone (`STARTED (IST)`), since
  `slip logs` shows UTC.
- `slip logs --channels` says `no lines in this window` instead of `empty`.
- `slip trust` shows optional settings as values or `not set` (with the
  default that applies) instead of `None` / `Some("…")`. Existing approvals
  are unaffected.

## [0.5.2] - 2026-10-08

### Fixed

- Ctrl-C while a step, `php artisan down`, or `php artisan up` was being
  launched could interrupt the launch and still report "it continues on the
  server", although the step never started. A launch now always completes
  before a detach applies, and a Ctrl-C pressed before the launch is sent
  starts no step.
- A run that stopped after maintenance mode was turned on, but before any deploy
  step ran, reported "cancelled/aborted before changes" while the app was down.
  It now reports the new outcome "Stopped in maintenance mode, no steps ran",
  and `slip receipts show` points to `slip up`. Receipts with this outcome are
  shown as unreadable by Shipslip 0.5.1 and earlier.

### Changed

- Library: `DeployOutcome::StoppedInMaintenance(AbortReason)` and
  `AbortReason::Cancelled`.

## [0.5.1] - 2026-10-08

### Changed

- After any failed recipe step, or one whose exit code was lost, the deploy
  reads the server's commit and working-tree state (read-only, best effort)
  and records it in the receipt and output, as it already did for a failed
  fast-forward. `slip receipts show` then reports where the server ended up.
  A failed read leaves it `not recorded` and never changes the outcome.

### Fixed

- After a failed step, `slip receipts show` no longer suggests only
  `slip from-step`, which reruns the same commit. It separates a server cause
  (fix it there, then `slip from-step`) from a code cause (push the fix, then
  `slip deploy`).
- When `slip deploy` is blocked as already up to date and the last run on that
  commit failed, is unknown, did not finish, or left the app in maintenance
  mode, the message says so and points to `slip receipts show`, `slip
  from-step`, and `slip up`.

## [0.5.0] - 2026-10-08

### Added

- `slip receipts [ENV] [--all]` lists saved receipts newest first, with the
  outcome and flags for new log errors, failed smoke checks, incomplete or
  unrecorded checks, and an app left in maintenance mode. Unfinished runs read
  as unfinished. Unreadable receipts and receipts from a newer Shipslip are
  listed, not hidden. Only the newest 20 are read unless `--all` is given.
- `slip receipts show ID` prints one run: steps with maintenance on/off, exit
  codes, saved output, log watch, smoke check, and next steps based only on
  recorded facts. `--md` prints Markdown without server output, log messages,
  warnings, reasons, or the deployer's email; `--md --with-details` includes
  them.
- New receipts record who started the run: the local user and the checkout's
  Git name and email. Older receipts show `not recorded`.
- Library: `receipt::{list, find, read, markdown}`, `Receipt::badge`,
  `DeployOutcome::summary` and `DeployOutcome::label`, and
  `logs::escape_field`.

### Fixed

- Deploy output no longer passes server text to the terminal unescaped. Step
  and maintenance output show without color codes, and other control
  characters, as well as those in log errors, server state, reasons, warnings
  and commit subjects, print as visible escapes such as `\x1b`, so server
  output cannot move the cursor, clear the screen or retitle the terminal.

## [0.4.0] - 2026-10-05

### Added

- `slip doctor [--for create|publish] [--json]` reports every requirement for
  project creation and GitHub publishing at once: tools, PHP extensions, Git
  identity, and GitHub login. It is read-only, exits 0 when ready and 3 when a
  blocking requirement is missing, too old, or broken, and prints versioned
  JSON for scripts. The GitHub login check stops waiting after 10 seconds.
- Tools installed in Homebrew's or Composer's global bin directory but not on
  `PATH` are used, with a warning that shows the line to add to your shell
  startup file.
- `slip setup` on macOS plans exact Homebrew, Composer, and Git identity
  commands, asks once, runs them with the approved tools first on `PATH`, and
  checks again. It never installs Homebrew, never edits shell startup files,
  and leaves tools owned by Herd, php.new, nvm, fnm, asdf, or mise untouched.
  Runs are locked, recorded privately, and safe to interrupt and rerun. Linux
  receives manual guidance.
- `slip new` and `slip publish github` repair missing prerequisites in place
  with one confirmation and continue with the answers already given, printing
  the exact rerun command when they stop. `slip new` warns early when
  publishing will later need `gh` or a GitHub login.

### Changed

- `slip publish github` exits 3 when publication is declined or prerequisites
  remain missing, instead of 0.
- Project name, branch, and destination are validated before any tool checks
  or setup.

## [0.3.0] - 2026-10-04

### Added

- `slip new <name|.>` creates a Laravel app with plain Laravel, React, Vue,
  Svelte, or Livewire; Laravel or no authentication; database and testing
  choices; an initial branch; and optional Laravel Boost. The command previews
  installed tools and the exact installer call, verifies tests and built
  assets in staging, moves without overwriting, and offers a reviewed initial
  Git commit. Creation requires Laravel installer 5.31.1 or newer.
- Per-destination local operation locks, private operation journals, retained
  staging on failure, and cancellation of the installer's process group.
  GitHub token environment variables are removed from installation processes.
- `slip publish github` previews the authenticated account and publication
  intent, scans the pushed history for sensitive filenames, creates a private
  repository by default, pushes without force, and verifies the remote SHA.
  Saved intent supports interrupted publication recovery without duplicate
  repository creation or overwriting an existing origin.
- Optional publishing and `slip init` continuation after project creation,
  with a separate reviewed commit and push for a newly written deploy config.
- A real eleven-case Laravel installer compatibility matrix, retaining nine
  starter-kit/auth combinations and adding plain/React Boost checks, exercised
  weekly and manually on macOS and Linux independently of pull-request checks.

### Fixed

- Git branch validation preserves executable launch errors instead of reporting
  them as invalid branch names.
- Linux release binaries now link correctly while preserving no-overwrite
  project moves.
- Publication checks the remote repository's actual visibility during review
  and before pushing, including resumed and completed operations. A mismatch
  stops instead of uploading under a stale private/public label.
- Publication preserves repository paths containing whitespace or native path
  bytes, verifies the exact reviewed branch ref, and applies HTTPS redirect,
  header, and TLS guards to the exact reviewed repository URL.
- Publication uses the active GitHub account and previews the existing origin's
  transport, even when the GitHub CLI's preferred protocol has changed.
- Ending setup input now reports retained project/commit/publication state
  accurately, and noninteractive missing-Boost guidance names `--boost` and
  `--no-boost`.
- Installer child commands use the reviewed tool executables when multiple
  versions are installed. Setup refuses a stale frontend recipe if its manifest
  or lockfiles change during the wizard.
- The installer matrix reports every case and a final tally even when a failed
  installer leaves malformed `composer.lock` JSON. Boost cases verify the locked
  package and MCP server after moving the scaffold. Checks recognize generated
  Claude/Cursor/VS Code/Junie/Codex configurations and verify the server directly
  when Boost selected no AI client.

### Changed

- Project creation prints clean progress messages instead of raw terminal codes
  and repeated installer spinner frames, while retaining warnings and errors.
- Project creation recommends Laravel Boost with a default-Yes setup prompt.
  `--boost` and `--no-boost` select it explicitly without prompting.
- Newly generated deploy configs include `npm ci` and `npm run build` when
  the project has `package-lock.json` and an npm build script. Other lockfiles
  produce a warning; existing configs and non-frontend recipes are unchanged.
- Setup documentation covers local creation and optional GitHub publishing.
  Deployment still requires an existing server checkout; server bootstrap and
  VPS provisioning remain outside this release.

## [0.2.2] - 2026-10-03

### Added

- Opt-in temporary GitHub authentication for `deploy`, `rerun` and `from-step`.
  `--github-token` prompts with hidden input; `--github-token-source env|gh`
  explicitly selects local environment credentials or the GitHub CLI login.
- Token account identification, repository visibility checks, account
  confirmation and replacement, and guidance for creating a read-only token.
- Standard GitHub SSH and HTTPS origins can fetch over HTTPS without saved
  server credentials or remote configuration changes. Tokens are restricted
  to the confirmed repository's preflight fetch and excluded from receipts,
  detached scripts and output. Existing credential stores, redirects, URL
  rewrites and recursive submodule fetches are disabled for this operation.
- Library APIs for discovering a GitHub repository and preparing a run with
  an explicit `GitHubToken` and pinned repository.

## [0.2.1] - 2026-10-02

### Fixed

- Log snapshots and deployment monitoring identify the root Laravel exception,
  including plain PHP `Error` and `Exception`, instead of picking up names
  from messages, middleware filenames, or previous exceptions in stack traces.
- Exception class names decode JSON escaping and use single namespace
  separators, so escaped and plain versions group together and detail headers
  remain readable. Original raw log entries remain unchanged.
- Middleware-only stack changes no longer make the same plain PHP error appear
  new relative to its baseline.

### Upgrade notes

- Corrected exception identities receive corrected group IDs. Deployment watch
  history saved under an incorrectly parsed class may report that error as new
  once after upgrading. Signatures for already-correct entries are preserved.

## [0.2.0] - 2026-10-02

### Added

- `slip logs ENV` reads a bounded snapshot over SSH and groups errors by
  exception class and application file. Open a group by ID or row number to
  see its message variants, latest entry, and stack trace.
- Filters for time, severity, and text; raw entries, channel inspection,
  all-group output, and configurable window byte limits.
- Automatic single-file and daily log channel discovery, with config options
  to hide, rename, or replace channels. Channels share the read budget fairly.
- Verified deploy, rerun, and from-step markers select the default log window,
  including runs in progress. Missing or stale markers fall back to checkout
  history, then 24 hours.
- A separately bounded pre-run baseline labels groups and variants `NEW`,
  `seen`, or `?`. Comparison spans channels; partial reads and missing or
  unusable baselines are disclosed.
- Timezone configuration, clock-change warnings, format recognition, and
  rotation/truncation checks. Log output escapes terminal control characters.
- Public `logs` library API and `LogChannel` settings.

### Changed

- Log summaries align IDs, counts, and status, with application paths,
  channels, and messages on separate lines. Partial counts use `≥`.
- Rust callers constructing `DeployTarget` must provide the new `timezone`
  and `logs` fields (`None` and `Default::default()` preserve defaults).
  Exhaustive `DeployEvent` matches must handle the new `Warning` variant.
  Existing project config and receipts remain compatible.

### Fixed

- Recipe commands inherit the server's permission mask instead of Shipslip's
  private `077` mask, preventing newly generated Laravel caches and compiled
  views from becoming unreadable by PHP-FPM. Shipslip run records remain
  private. Existing file permissions are not changed by upgrading.

## [0.1.1] - 2026-10-01

### Added

- Prebuilt binaries for macOS and Linux and a one-line installer; Rust is no
  longer required to install.
- `slip --version` (or `-V`) prints the installed version.

## [0.1.0] - 2026-10-01

First usable release. Version 0.0.1 only reserved the crate name.

### Added

- `slip` CLI with `init`, `trust`, `deploy`, `rerun`, `from-step`, `attach`,
  `break-lock`, and `up`.
- `slip init` writes a commented `.shipslip.toml` from a few questions, with
  the default Laravel recipe spelled out.
- Two-phase deploys: read-only preflight shows the exact commits and steps,
  and nothing changes on the server until you confirm. Production runs require
  typing the environment name.
- Preflight requires a clean checkout on the configured branch and a
  fast-forward, and rechecks the server right before the first change.
- Config trust: each environment's settings must be approved with
  `slip trust`, and approved again after they change.
- A deploy lock on the server with a heartbeat. `slip break-lock` shows the
  holder and clears a stale lock after you confirm.
- Steps run detached on the server, so a dropped connection does not stop
  them. `slip attach` resumes an unfinished run without relaunching its step.
- Optional maintenance mode around the steps (`php artisan down`/`up`), and
  `slip up` to bring the app back.
- Local receipts recording the plan, commits, step results, recent output,
  log watch, smoke check, and outcome.
- Post-deploy checks: the Laravel log is watched during the deploy and for
  120 seconds after it, reporting only errors not seen before, and an optional
  smoke URL is requested from your computer.
- Ctrl-C acts on the current stage: it cancels cleanly at the confirmation
  prompt, stops following a running step, or ends the log watch early. A
  second Ctrl-C quits at once.
- A Rust library API (`prepare`, `execute`, `attach`, `DeployEvent`, and
  more) for other front-ends. The library never reads stdin or writes to
  stdout or stderr.

### Requirements

- macOS or Linux with Rust 1.88 or newer, Git, and OpenSSH; `curl` for smoke
  checks.
- A Linux server with bash and a Git checkout that can `git fetch origin`
  without a prompt.

[0.9.1]: https://github.com/impruthvi/shipslip/releases/tag/v0.9.1
[0.9.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.9.0
[0.8.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.8.0
[0.7.1]: https://github.com/impruthvi/shipslip/releases/tag/v0.7.1
[0.7.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.7.0
[0.6.2]: https://github.com/impruthvi/shipslip/releases/tag/v0.6.2
[0.6.1]: https://github.com/impruthvi/shipslip/releases/tag/v0.6.1
[0.6.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.6.0
[0.5.3]: https://github.com/impruthvi/shipslip/releases/tag/v0.5.3
[0.5.2]: https://github.com/impruthvi/shipslip/releases/tag/v0.5.2
[0.5.1]: https://github.com/impruthvi/shipslip/releases/tag/v0.5.1
[0.5.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.5.0
[0.4.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.4.0
[0.3.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.3.0
[0.2.2]: https://github.com/impruthvi/shipslip/releases/tag/v0.2.2
[0.2.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.2.0
[0.1.1]: https://github.com/impruthvi/shipslip/releases/tag/v0.1.1
[0.1.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.1.0
