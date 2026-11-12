# Changelog

All notable changes to Shipslip are listed here. Shipslip follows
[Semantic Versioning](https://semver.org/); before 1.0, a minor release may
change the library API.

## [Unreleased]

## [0.13.0] - 2026-11-12

### Added

- Connection gaps are recorded. When the connection drops while Shipslip is
  watching a step, maintenance or the log watch, the receipt lists when this
  Mac last heard from the server and when it re-attached. A step that ended
  during a gap has no end time or duration, only the time this Mac saw its
  result. `slip` prints a line when the connection is lost and when it
  re-attaches. The desktop app shows the gap in amber on the run screen and
  the "Deploys running" sidebar row, and lists gaps in the receipt. Raw ssh
  error text stays out of receipt warnings.
- Receipts record why this Mac stopped watching a run that is still open:
  detached, app closed or connection lost. The desktop app's reopened panel
  says that cause, or only that the run did not finish when nothing was
  recorded. Receipt JSON rows gain the optional `unfinished_cause` field.
- A bring-up (`slip up` and the app's "bring the app up") makes the
  configured smoke check after `php artisan up` succeeds. A newer smoke
  result replaces an older one on the release map for the same target.
- Desktop app: the run screen leads with the outcome as soon as it is known.
  A failed step's number, exit code and command show in red at once, with
  its output expanded; a failed or stopped run stays on its outcome until
  the receipt is opened. A stop request is acknowledged on the disabled
  button.
- Desktop app: the deploy lock screen tells another run that is deploying
  (holder, short target SHA, heartbeat age, Check again) apart from a stale
  lock, and only offers the break-lock command for a stale lock.
- Desktop app: the attach review names the command that was running (to be
  watched, never run again) apart from the steps after it. The attached run
  screen shows maintenance and earlier steps with their recorded status and
  times.
- Desktop app: the bring-up review lists the recipe steps the run never
  sent and offers that run's run-from-step instead.
- Desktop app: the release map re-reads receipts when the local receipt
  directories change, and shows runs that a `slip` command on this Mac is
  driving, with Deploy and Attach unavailable while they run. The footer
  shows when receipts were last read.
- Desktop app: one typed confirmation for production deploys, configuration
  approval and recovery: exact, case-sensitive match, with a hint when the
  text does not match. Approving settings from the deploy flow continues to
  the review instead of returning to the map.
- Step 0 has one label everywhere, with the exact command as detail. A
  run-from-step shows its step range ("steps 4–5 only") on the release map
  and the receipt header.
- Library: `DeployEvent::ConnectionLost`, `Reattached`, `ChecksSkipped` and
  `OutcomeKnown`, with `ConnectionTarget`; `ExecutionHandle::closing_hard()`;
  `step_zero_label()` and `step_zero_command()`; `CheckSkipReason`,
  `UnfinishedCause` and `ConnectionGap` on receipts and records;
  `ReceiptJournal::is_claimed()` and `connection_event()`;
  `records::env_order()` and `describe_steps_run()`;
  `ErrorGroup::normalized_message`.

### Changed

- A run that leaves the app in maintenance mode skips the smoke check and
  the log watch, says why, and releases its deploy lock at once. Receipts
  store the optional `checks_skip_reason` without changing their version. A
  run that has not finished records no check results.
- Each log watch result has its own wording and flag. Receipt JSON
  `receipt.flag.kind` replaces `log_not_fully_observed` with
  `log_watch_interrupted` and `log_unreadable` (attention) and
  `no_log_file_yet`, `log_watch_stopped_early` and
  `log_watch_skipped_in_maintenance` (information).
- The release map and `slip status` list production first, staging second,
  then the other environments by name. With a compare active, the map
  scrolls the compared columns into view.
- Desktop app: success is shown in ink on the receipt slip, and "Quit now"
  is no longer red.
- Library: `StepTimes` gains `result_seen_at_ms`; receipt steps gain
  `result_seen_at_ms` and `connection_gaps`. `Flag::LogNotFullyObserved` is
  replaced by one variant per log watch result.
  `ReceiptJournal::observation()` takes optional watch and smoke results
  and a skip reason.

### Fixed

- The log watch recovers after an SSH interruption: reconnect waits are
  bounded at 15 seconds, and a watch that recovers every line from its saved
  position stays complete. Only observation is retried; steps are never run
  again.
- Errors with no exception class and no app file are grouped by their
  message, so several unrelated errors no longer count as one group. The
  Markdown receipt shows each group's message with `--details`.
- A log watch that starts after re-attach is no longer marked partial or
  given restart warnings.
- The receipt's log watch time is measured from the end of the steps, not
  from the start of the run.
- A bring-up receipt's row is labelled "Maintenance off" instead of step 0.
- Desktop app: colour codes are stripped from step output while a run is
  going, and the count of dropped output lines includes lines dropped
  between batches.
- Desktop app: approving configuration is no longer refused with "The map
  changed" when the map refreshes in the background.
- Desktop app: UI text says "environment" instead of "env".

## [0.12.0] - 2026-11-01

### Added

- `slip logs ENV --json` returns every tracked error group as schema-1 JSON,
  including stable full IDs, typed window anchors and fallbacks, baseline
  badges, lower-bound counts, channel coverage, filters and warnings.
  `slip logs ENV ID --json` adds the latest entry's full text, best-effort
  stack frames and same-request entries (correlation, not cause). Group
  lookup accepts a row number, short ID or full ID from an earlier snapshot.
  JSON rejects `--raw`, `--channels` and `--md`; `--full` has no effect.
- Log JSON failures have typed error kinds, including SSH refusals with a
  reason and connections lost during a snapshot. Invalid input and ambiguous
  IDs exit 2; other failures exit 1. A successful read with no log files
  exits 0 with empty groups and channels.
- `slip mcp` serves local `status`, `receipts` and `receipt` records over
  stdio. Starting it with `--read-logs` also exposes `logs` and `log_group`
  for fresh snapshots, only in checkout roots already listed in the release
  map and for environments whose current config matches their approval.
  Missing or changed approval returns `not_approved` before SSH. The server
  cannot deploy, change a server, take or break a lock, or approve config.
  `--config` is rejected and `SHIPSLIP_CONFIG` is ignored.
- MCP uses protocol `2026-07-28`, with `server/discover`, per-request
  metadata and compatibility `initialize`/`ping`. Tools return the same
  schema-1 record as structured content and one JSON text item; Shipslip
  failures are tool errors and protocol mistakes are JSON-RPC errors.
- MCP log text defaults to 16 KiB per field, configurable from 1 to 256 KiB,
  with explicit head/tail clipping, original byte counts and frames parsed
  before clipping. Same-request entries are limited to five per side, with
  omitted entries counted. CLI JSON keeps all same-request entries and does
  not clip text for display. The [JSON reference](https://github.com/impruthvi/shipslip/blob/v0.12.0/docs/json-reference.md)
  documents the schemas, text metadata, fixed values and error kinds.
- Library: `records::logs_summary()`, `logs_detail()`, `find_log_group()`,
  serializable log record/error types and `limit_log_text()` build records
  from already-read snapshots. `read_only::Reader` supplies the five MCP
  queries, with `ApprovedTarget` and an injectable `Connector` enforcing the
  approval boundary for log connections.

### Changed

- Library: `logs::Snapshot::since_label` is replaced by `anchor: Anchor`
  and `anchor_fallbacks: Vec<AnchorFallback>`, with typed run checks and
  fallback reasons. Callers constructing snapshots must provide these fields;
  front ends now word the window from the types.
- Library: log rendering moved to the CLI, which renders the same records
  used by JSON. The `logs::render_*` functions and `DetailFilter` were
  removed. `Related::more` was removed; `logs::related()` now returns all
  matching entry indices, and front ends apply their own display limits.
  `Since` is serializable and gains `validate()` for checking explicit
  windows before opening SSH.
- ARM64 macOS release builds use the supported `macos-15` runner.

### Fixed

- Log text renderers escape server text, including anchor and channel fields,
  and shorten commit text safely. JSON retains the original server text.
- Impossible calendar dates and windows older than 30 days are rejected
  before SSH. Empty log JSON omits placeholder channels, and a lost snapshot
  session is distinguished from an initial SSH refusal.

## [0.11.0] - 2026-11-01

### Added

- `slip status --json`, `slip receipts [ENV] --json` and
  `slip receipts show ID --json` print what this Mac recorded as versioned
  JSON with a top-level `"schema": 1`. Within a schema number, fields are
  only added. Outcomes, check flags, evidence marks and step states are
  kinds, not English text; an unknown outcome stays `unknown`. Times are
  absolute milliseconds, and a step whose end was not seen has no end time
  and no duration. Next steps arrive as text and command parts (each command
  as its argument list) with a flag saying whether the commands are safe to
  copy. Server and receipt text is the raw value, encoded by JSON.
- Errors under `--json` print `{"schema": 1, "error": {"kind": ...}}` on
  stdout and exit with the same code as the text command. Kinds: `missing`,
  `ambiguous` (with the matching ids), `newer_version`, `unreadable` and
  `invalid_input` (bad arguments or config).
- Library: a `records` module with three read-only queries, `status()`,
  `receipts()` and `receipt()`, returning serializable records. `Untrusted`
  holds server and receipt text raw and has no `Display`, so a front end must
  choose an escape to print it.

### Changed

- Library: the receipt module no longer supplies display text. `Badge`,
  `Flag`, the outcome summary, next steps, started-by, `describe_plan` and the
  overview types (`EnvCell`, `CodeState`, `Mark`, ...) moved to `records`.
  Headlines, flag and cell text, qualifiers and relative times (`code_text`,
  `sentence`, `age`, `headline`, `Flag::text`, `describe_smoke`,
  `describe_watch` and similar) were removed; front ends word them from the
  kinds. Terminal text, Markdown receipts and the desktop app read the same
  as before.
- The desktop app reads the release map, receipts and a deploy entry's recent
  runs through the same queries as the CLI, so copyable recovery commands
  follow the CLI's rule.

## [0.10.0] - 2026-10-31

### Added

- Receipts record when each recipe step and each maintenance down/up command
  was sent and when this Mac saw it end, by this Mac's clock. `slip receipts
  show`, the Markdown receipt and live deploy output show how long each one
  took (`in 12s`). A step or maintenance command whose end was not seen (lost
  connection, detach, or a result first seen by a later `slip attach`) shows
  `started <time>, end not known`; a duration is never guessed. Older receipts
  read as before, and receipt versions are unchanged.
- Library: `StepTimes`, `ReceiptStep::times()`, and
  `maintenance_down_times()` / `maintenance_up_times()` on the receipt;
  `config::trust_settings()` returns each trusted setting with its previous
  and proposed value (`trust_changes()` now formats from it, so `slip trust`
  prints the same).
- Desktop app (local build): preview, run and follow a deploy from the
  release map over your own ssh, with trust review, refusal screens, recovery
  actions (bring the app up, run from step, attach) and a Dock signal when a
  run ends. The app was redesigned: sidebar, one-line board, receipt
  inspector, step times, and light and dark mode following macOS.

### Changed

- Library: `DeployEvent::StepFinished` and `DeployEvent::MaintenanceFinished`
  carry `times: StepTimes`, the values the receipt journal saved.
- An Up run detached before `php artisan up` was sent now records the step
  as not started, with no start time.

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

[0.13.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.13.0
[0.12.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.12.0
[0.11.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.11.0
[0.10.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.10.0
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
