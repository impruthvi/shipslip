# Changelog

All notable changes to Shipslip are listed here. Shipslip follows
[Semantic Versioning](https://semver.org/); before 1.0, a minor release may
change the library API.

## [Unreleased]

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
  package and generated MCP command after moving the scaffold.

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

[0.2.2]: https://github.com/impruthvi/shipslip/releases/tag/v0.2.2
[0.2.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.2.0
[0.1.1]: https://github.com/impruthvi/shipslip/releases/tag/v0.1.1
[0.1.0]: https://github.com/impruthvi/shipslip/releases/tag/v0.1.0
