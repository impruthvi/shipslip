<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.png">
    <img src="assets/logo-light.png" alt="Shipslip" width="400">
  </picture>
</h1>

<p align="center">
  <a href="https://crates.io/crates/shipslip"><img src="https://img.shields.io/crates/v/shipslip" alt="Crates.io version"></a>
  <a href="https://github.com/impruthvi/shipslip/actions/workflows/ci.yml"><img src="https://github.com/impruthvi/shipslip/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status on main"></a>
  <a href="LICENSE"><img src="https://img.shields.io/crates/l/shipslip" alt="License"></a>
</p>

Shipslip creates a verified Laravel project, optionally publishes it to GitHub,
and runs a configured deploy recipe over SSH. Each stage shows a preview and
asks for confirmation. Production deploys require typing the environment name.

First time? [Try Shipslip](docs/try-shipslip.md) walks through a first staging
deploy in about 15 minutes. New teammate? Follow the
[staging setup guide](docs/setup.md) for installation, SSH access, project
configuration, deployment, and recovery.

## Quick start

Install the CLI:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/impruthvi/shipslip/releases/latest/download/shipslip-installer.sh | sh
```

Open a new terminal, then check it with `slip --version`. The installer puts
`slip` in `~/.local/bin`. With Rust 1.88 or newer, `cargo install shipslip
--locked` also works.

Shipslip runs on macOS (primary) and Linux (supported, less tested). Windows
is not supported yet.

Project creation and GitHub publishing require Shipslip **0.3.0 or newer**;
`slip doctor`, `slip setup`, and in-place repair require **0.4.0 or newer**;
`slip receipts` requires **0.5.0 or newer**; `slip status` requires **0.6.0
or newer**.
Run the GitHub installer above to upgrade, or use the
[local build instructions](#try-project-creation-from-this-checkout).

Check your machine first. Doctor is read-only:

```sh
slip doctor
```

It lists every tool, PHP extension, Git identity, and GitHub login that
creation and publishing need, then exits 0 when ready or 3 when something is
missing. On macOS with Homebrew, `slip setup` fixes what it can: it shows the
exact commands, asks once, runs them, and checks again. See
[check and fix your machine](docs/setup.md#check-and-fix-your-machine).

Create a project from a directory outside any existing Git checkout:

```sh
slip new my-app --starter-kit react
cd my-app
php artisan serve
```

Open the local URL printed by Artisan. Shipslip asks for any missing choices,
checks your installed tools, and previews the destination and exact installer
command. If a tool is missing, it shows only what needs attention and offers
the same reviewed plan as `slip setup`; after one confirmation it continues
with your answers. Child commands use the tool executables shown in that preview, even
when multiple versions are installed. After confirmation, it installs
dependencies, builds assets, runs
`php artisan test`, and offers an initial commit after showing the files to
include. Git needs your `user.name` and `user.email` configured. Declining the
commit leaves the verified project and staged files local.

Project creation needs local PHP, Composer, Laravel installer **5.31.1 or
newer**, Node.js, npm, and Git; see [prerequisites](docs/setup.md#local-project-creation-prerequisites).
Starter kits are `none`, `react`, `vue`, `svelte`, and `livewire`. Kits support
Laravel authentication or `--auth none`; plain Laravel (`--starter-kit none`)
uses no authentication. SQLite, Pest, and branch `main` are the defaults.
Laravel Boost is recommended: its setup prompt defaults to Yes. Pass `--boost`
to enable it without the prompt, or `--no-boost` to skip it. WorkOS, community kits, teams,
and package managers other than npm are not supported.

Boost configures the AI clients it detects. To choose or add a client after
creation, run `php artisan boost:install` from the new app.

`slip new .` creates in the current directory only when it is completely empty,
including hidden files. Both creation modes refuse symlink destinations and
destinations inside another Git checkout. There is no force or overwrite
option. A failed installation keeps its operation-owned staging directory and
reports its path; the scaffold is its `project/` child. Cleanup removes only
that operation's staging directory and retains the private operation record.
If the destination changes before the move, staging is retained for inspection
and the conflicting entry is never replaced.

Publish when you are ready:

```sh
gh auth login
slip publish github
```

GitHub CLI (`gh`) is needed only for publishing. Shipslip shows the
authenticated account, owner, repository name, visibility, commit count, and
HEAD before a separate confirmation. Repositories default to private; use
`--owner OWNER --repo NAME --visibility public` to request other settings.
Publishing scans the entire history being pushed for sensitive filenames,
including `.env`, `auth.json`, and private keys; `.env.example` is allowed.
An existing `origin` must point to the selected repository. Shipslip never
overwrites it or force-pushes. It verifies that the remote branch matches local
HEAD, and an interrupted publication can resume by running the command again.
The preview uses an existing origin's actual transport. If the remote repository's
visibility differs from your reviewed choice, publication stops before pushing.
An ambiguous existing repository requires choosing another name.

Creation offers publishing after the initial commit and deploy configuration
at the end. You can decline either and run `slip publish github` or `slip init`
later. When you accept both offers during `slip new`, Shipslip shows the new
config's diff and offers a separate config-only commit, followed by another
publication preview and confirmation. Other changes are not included.

Configure deployment only after the server has an **existing checkout** of the
project. Shipslip does not clone an empty server destination or provision PHP,
databases, a web server, or a VPS. Server bootstrap is planned separately.

From your Laravel project's Git repository, run `slip init`. It asks for the
environment name, SSH host alias, server path, branch, whether the environment
is production, maintenance mode, and an optional smoke URL, then writes
`.shipslip.toml` at the repository root. It never replaces an existing file.
For projects with `package-lock.json` and an npm `build` script, the newly
generated recipe includes `npm ci` and `npm run build` after Composer and
before Artisan optimization. Other package-manager lockfiles produce a
warning without adding build steps. If the frontend manifest or lockfiles
change during setup, restart `slip init` to review the updated recipe. Existing
configs are unchanged.
Or write the file yourself:

```toml
[project]
name = "my-app"
stack = "laravel"

[env.staging]
ssh = "my-app-staging"
path = "/srv/my-app"
branch = "main"
production = false
maintenance = true
smoke_url = "https://staging.example.com/health"

[env.production]
ssh = "my-app-production"
path = "/srv/my-app"
branch = "main"
production = true
maintenance = true
log_daily = true
smoke_url = "https://example.com/health"

[recipe.deploy]
steps = [
  "composer install --no-interaction --no-dev --prefer-dist",
  "php artisan migrate --force",
  "php artisan config:cache",
]
```

`ssh` must name a host alias in your OpenSSH config, usually `~/.ssh/config`.
`path` is the absolute server path. `production` is required for every
environment. `steps` are shell commands run in order; if `[recipe.deploy]` is
omitted, Shipslip announces and uses its Laravel default recipe. When
`maintenance = true`, Shipslip runs `php artisan down` before the run and
`php artisan up` after all steps succeed.

Shipslip watches `storage/logs/laravel.log` by default. Set `log` to another
path. With `log_daily = true`, `log` is a directory and prefix (default
`storage/logs/laravel`), and Shipslip follows the newest `laravel-*.log`.
An optional `smoke_url` is requested from your Mac after the deploy steps.

From anywhere in the Laravel project repository, review and approve the config
before the first run:

```sh
slip trust staging
slip deploy staging
```

Install `slip` first using the [setup guide](docs/setup.md).
Shipslip searches from the current directory to the Git root for
`.shipslip.toml`. `--config FILE` selects a specific file; `SHIPSLIP_CONFIG`
sets a default override.

### Try project creation from this checkout

To test this source checkout, build the local binary first. Run creation from a
normal non-Git directory, not from inside the Shipslip source checkout:

```sh
cargo build --locked --bin slip
slip_binary="$PWD/target/debug/slip"
mkdir -p "$HOME/shipslip-local-apps"
cd "$HOME/shipslip-local-apps"
"$slip_binary" new my-app --starter-kit react
cd my-app
php artisan serve
```

Use a new app name on subsequent runs. To publish this app with the same build,
run `"$slip_binary" publish github` from its directory. To configure deployment,
run `"$slip_binary" init` after preparing an existing server checkout. The
installed binary can be checked with `slip --version`; creation and publishing
require 0.3.0 or newer.

### Check Laravel installer compatibility

From this source checkout, run the script regressions and real installer matrix:

```sh
bash scripts/installer-matrix-test.sh
bash scripts/installer-matrix.sh .context/installer-compat.md
```

The matrix creates eleven temporary apps: nine starter-kit/auth combinations
without Boost, plus plain Laravel and React with Boost. It runs application
checks, moves each scaffold to a path containing spaces, and rebuilds assets.
Boost cases also initialize MCP and list tools after
relocation. The report retains every result and per-case logs on failure.
This takes longer than unit tests and downloads Composer/npm dependencies;
it does not create GitHub repositories. Weekly/manual CI runs the same matrix
on macOS and Linux.

### Use a local GitHub token

If the server has no GitHub credentials, use a token from your laptop for the
preflight fetch:

```sh
slip deploy staging --github-token
```

Shipslip first inspects the server's repository, then asks for a token with
hidden input. It checks the token's GitHub account and repository visibility,
shows the username and `OWNER/REPO`, and lets you continue, enter another
token, or cancel. The server's fetch verifies actual read access; the normal
deploy preview and production confirmation still follow. Your SSH identity
and personal GitHub CLI login do not select the account for this token.

Need a token? The prompt includes a GitHub creation link with the repository
owner and read permissions prefilled. Select the target repository and
**Contents: Read-only**, choose an expiry, and obtain any required organization
approval. Classic tokens may need SSO authorization. Follow your organization's
token policy; a personal account can be used if it has the required access.

Other credential sources must be selected explicitly:

```sh
slip deploy staging --github-token-source env  # local GH_TOKEN, then GITHUB_TOKEN
slip deploy staging --github-token-source gh   # active github.com GitHub CLI login
slip rerun staging --github-token
slip from-step staging 2 --github-token
```

`--github-token` always prompts, even if an environment token or GitHub CLI
login exists. Token flags accept a source name, never a token value. Hidden
entry requires an interactive terminal. Scripts must choose `env` or `gh`
explicitly and still answer the account and deploy confirmation prompts.

Temporary authentication supports standard `github.com` HTTPS and SSH origins.
It fetches over HTTPS without changing the saved `origin`; embedded credentials,
other hosts, Git URL rewrites and redirects are refused. The repository is
pinned to the one shown at account confirmation. The trusted server receives
the token briefly in process memory over SSH. Shipslip does not save it on
either machine or include it in receipts, detached step scripts or output;
existing server credential helpers cannot save it. Existing `gh` credentials
stay managed by GitHub CLI. The token is used only for this fetch, not for
Composer, submodules or recipe steps. Local `curl` is required for account
validation. Without a token flag, server authentication works as before.

## Run plans

```sh
slip deploy staging          # fast-forward, then run every recipe step
slip rerun staging           # rerun every recipe step on the checked-out commit
slip from-step staging 2     # run recipe steps 2 through the end
```

`deploy` is the only plan that changes the server checkout. Reruns require the
server to already be on the clean target commit. Recipe steps are numbered
starting at 1; the built-in Git fast-forward is step 0 and is not selectable.

## Read logs

```sh
slip logs staging                    # grouped errors since the latest run
slip logs staging --since 7d          # override the default window
slip logs staging --channels          # files, formats and coverage per channel
slip logs staging qmkte               # latest full entry for a group ID
slip logs staging qmkte --full        # every stack frame, nothing folded
slip logs staging qmkte --md          # the same detail as Markdown to paste
slip logs staging --level warning --grep payment
slip logs staging --raw --since 30m   # entries in timestamp order
slip logs staging --all --max-bytes 8m
```

`logs` discovers `storage/logs/*.log` and reads the configured `log` /
`log_daily` files over SSH. A final `-YYYY-MM-DD.log` suffix identifies a daily
channel; other `.log` files are single channels. Single and daily files with
the same name merge into one channel. It needs no Laravel package, runs no PHP or recipe steps, and leaves app files, deploy
locks, receipts and signature history unchanged. The summary groups entries
by exception class and app file, including recurring errors. Copy a group's
ID to open its latest entry and stack trace; row numbers also work, but can
move between invocations. The first 20 groups are shown unless you use `--all`.

The summary header counts every entry in the window by level, for example
`in this window: 3 error · 12 warning · 40 info`, whatever `--level` and
`--grep` hide. In a group's detail, runs of three or more framework frames
(files under a `vendor` directory) fold into one line such as
`… 14 vendor frames (--full to show)`; the first frame, your app's frames and
lines slip can't read as frames always stay. `--full` shows every frame.

When the latest entry carries a request or job ID at the top level of its
Laravel context, the detail view lists other entries in the window with the
same ID, up to ten before and ten after, across all channels. Keys are tried
in this order: `request_id`, `trace_id`, `correlation_id`, `job_id`,
`batch_id`. This is correlation, not cause: the lines came from the same
request (or, for `trace_id`, the jobs it queued), not necessarily one from
another. Laravel adds no ID by default; a middleware that calls
`Context::add('request_id', (string) Str::uuid())` puts one on every line. When
more lines match, slip prints a `--raw --grep` command for the same window;
being a text search, it can also show the ID mentioned elsewhere.

`--md` prints a group's detail as Markdown for an issue or chat. It starts with
the environment, log timezone, window (start to the time it was read),
coverage, baseline, filters and slip version, so a paste still makes sense
later. Log text is escaped or fenced, so links, images and HTML in it show as
plain text. `--md` folds frames like the terminal unless you add `--full`.

The default window starts at the latest verified Shipslip run (deploy, rerun
or from-step). During a run, the header says `deploy in progress`. If the run
marker is missing or stale, the command uses the latest Git checkout change,
then falls back to 24 hours when no reflog is available. The header always
names the anchor; corrupt markers and Git failures are reported. `--since`
selects your own window instead.

`NEW` means the group was not found in the bounded logs immediately before
that window. Comparison spans all channels, so an error moving between
channels stays known. `?` means there is no usable comparison for that group's
channels. New groups appear first; details also label new and seen message
variants. The summary aligns IDs, counts and status (`NEW`, `seen` or `?`),
with the application file, channels and message on separate lines. Baseline
coverage and channels without a baseline are disclosed;
`NEW` is limited to that readable span, rather than the app's entire history.
A partly recognized baseline can confirm known errors, but gives `?` for
otherwise unseen groups because its unparsed output is not a usable comparison.

`--level` includes that level and anything more severe; it defaults to `error`
for groups and `debug` for `--raw`. `--grep` matches text without regard to
case. `--since` accepts `30m`, `6h`, `7d`, `YYYY-MM-DD` or
`"YYYY-MM-DD HH:MM"`, up to 30 days back. Set `timezone = "Asia/Kolkata"`
in the environment table if Laravel writes times in that zone; the default
is UTC. Clock times are interpreted in that zone. Raw output warns when a
clock change makes ordering across files approximate.

The default read limits are 4 MiB of uncompressed log per channel and 12 MiB
total. Channels share the total fairly, with unused shares redistributed;
quiet channels get their whole file when it fits. `--max-bytes` replaces the
total and per-channel window limits (`k`, `m` and `g` use binary units).
The comparison has a separate 2 MiB per-channel / 6 MiB total limit, shared
fairly. It walks backward from the anchor through up to seven earlier daily
files; `--channels` includes its coverage. At most
20 channels and 50 files are selected per snapshot; omissions are reported.
Partial coverage is printed in the header, and when any channel was partly
read or contains lines slip couldn't parse, counts are lower bounds: the
header line says `at least`, and group and variant counts use `≥`. Use
`--channels` to inspect each channel's files, total size, last write, format and
coverage. Files that rotate or are truncated during the snapshot are reported as changed. Individual entries
are limited to 256 KiB, with a note in the detail view when truncated.

Files resolving under `storage/logs`, including shared storage symlinks, need
no config approval. Reading another configured path requires `slip trust ENV`.
The server needs Linux with `/proc`, bash, GNU coreutils, gzip and tzdata.
Log output can contain secrets; treat it as private.

Deploys atomically record `<git dir>/shipslip.last-run` at run start and update
it before releasing the owned lock. Attach completion updates the same run.
Marker failures warn in output and receipts and never fail a deploy. The
reader uses inode, size and checksum checks to validate recorded byte
positions; rotation or copytruncate falls back to timestamps with a note.
A later checkout invalidates the run anchor, even if HEAD returns to the same
commit. An abandoned run or failed end update can show `outcome unknown`.

Override channels by their original name in config:

```toml
[env.staging.logs.worker]
hide = true

[env.staging.logs.payments]
rename = "billing"

[env.staging.logs.audit]
path = "storage/logs/audit-output.txt"
```

`hide` excludes a channel before reading; `rename` changes its display name.
`path` adds a channel, or replaces the discovered files for that name. A missing
pinned path remains visible in `--channels`. Hidden channels cannot also set
`rename` or `path`.

Laravel headers and stack-trace lines identify recognized formats. Small
files need only one valid header. Mixed output keeps valid Laravel entries
in groups and reports the percentage recognized. JSON and other unsupported
formats are labeled and appended as file blocks by `--raw`. These blocks
retain file order and bypass `--level` and timestamp filtering; `--grep`
filters their lines. All output escapes terminal control characters. If no
file was written in the window, a hint explains that stderr, syslog and
service channels are invisible to this file reader.

## Receipts and recovery

Each CLI run saves a local receipt before it starts a remote command. On macOS,
receipts live under `~/Library/Application Support/Shipslip/receipts/` in a
directory for the project and environment. The receipt records the approved
plan, exact commits, step results, recent step output, log watch, smoke result,
and final outcome. New receipts also record who started the run: the local
user and the checkout's Git name and email.
Treat receipts as private: command output can contain secrets.

List and read receipts without contacting the server:

```sh
slip receipts                 # newest 20 for this project; --all lists every one
slip receipts staging         # one environment
slip receipts --json          # a versioned JSON list; also accepts ENV and --all
slip receipts show 18dab54b   # one run, by the ID shown in the list
slip receipts show 18dab54b --md
slip receipts show 18dab54b --json
```

`receipts --json` prints `{"schema":1,"rows":[...],"older":0}`. Rows carry a
`state` of `receipt`, `newer`, or `unreadable`, so a damaged file does not hide
the rest of the list. Readable rows include the short unique ID, local path,
run kind, typed outcome and flags, and FROM/TARGET commits. `older` counts
receipts beyond the default limit of 20; `--all` removes that limit.

`show --json` prints the full record with `"schema": 1`. Times are absolute
milliseconds; an unobserved step end and its duration are `null`. Receipt text
and server output remain raw JSON strings. Lookup errors print
`{"schema":1,"error":{"kind":...}}`: `missing`, `ambiguous` (with `ids`), and
`newer_version` (with `version`) exit 2; `unreadable` (with `message`) exits 1,
the same as text mode. Within schema 1, fields are only added.

Each row shows the outcome with flags for anything that needs a look:
`⚠ N new error groups`, `⚠ smoke 500`, `ⓘ log not fully observed`,
`ⓘ log not recorded`, `ⓘ smoke not recorded`, and `⛔ app may be in maintenance mode`.
A run that never finished reads `Unfinished (last recorded: …)`, never as its
saved outcome. `show` prints the steps (including maintenance on/off) with exit
codes and saved output, the log watch and smoke results, and what to check
next. It only states what the receipt recorded. After a failed step, or one
whose result is unknown, Shipslip reads the server's commit and whether the
checkout has uncommitted changes (read-only, best effort); if that read fails,
`show` says `not recorded`.

`--md` prints Markdown with statuses, exit codes, commits, commands, and error
classes and counts, but no server output, log messages, warnings, or reasons,
and names who started the run without their email address.
`--md --with-details` adds them; review that output for secrets before sharing
it. Server text is escaped in both the terminal and Markdown.

Ctrl-C at the confirmation prompt cancels the run and releases the deploy
lock. Answering no, or mistyping a production environment name, releases the
lock too and exits 1: a command exits 0 only when it did what it was asked. During the steps, Ctrl-C starts no new step and stops following the
running command. During the post-deploy log watch, it ends the watch early and
still finishes the run. While maintenance mode is being turned off, Shipslip
finishes that first. Press Ctrl-C again to quit at once.

If `slip` exits while a command is running, that command continues on the
server. Resume the unfinished run from the same project with:

```sh
slip attach staging
```

`attach` checks the saved config and asks for confirmation. It observes the
active remote command without launching it again, then continues the remaining
steps if the run still owns the deploy lock. It requires the same local receipt
and project checkout. Only one process can use a receipt at a time. If the
server cannot be reached, the receipt stays unfinished so you can try again.

If the result is unknown and the deploy lock becomes stale, first inspect the
server over SSH to confirm no command from the old run is still running. The
receipt's run ID identifies its directory under `~/.shipslip/runs/`. Then
clear the stale lock and, if maintenance mode may still be on, bring the app up:

```sh
slip break-lock staging
slip up staging
```

`break-lock` shows who holds the lock and its run ID, and only clears a lock
whose heartbeat is at least two minutes old; it asks you to type the
environment name. `up` takes a new deploy lock and runs
`php artisan up`; it asks for confirmation. `up` saves its own receipt
before sending the command, so if it is interrupted, `slip attach staging`
follows it again without sending it twice. Neither command resumes the old
deploy. Check the receipt and server state before choosing a new run plan.

### What is on each server

`slip status` reads every project's receipts and approvals and prints, for each
checkout and environment, the last code they record. It works from any folder,
needs no config, and never contacts a server.

```sh
slip status
slip status --compare staging production   # commits between the two, per app
```

A commit is shown only when a receipt backs it: `code at …` when the
fast-forward succeeded (`deploy incomplete` if a later step did not), `checked
out at start` for reruns, and `observed …` when Shipslip read the server's
commit after a failed step. Otherwise the line says `server code not known`,
`no deploy receipt on this Mac`, or `target changed` when the environment now points at a
different server or path. Each line also shows the last run and its flags,
and what to do next when that run needs attention. It warns when a newer
receipt could not be read, and when another checkout ran a newer deploy on the
same server path, since that run may have replaced the code. It also warns
when the approved config now deploys to a different server or path than the
newest receipt, until a run uses the new one. `--compare` uses your local checkout to count commits and notes an
incomplete deploy or dirty tree on either side.

### Desktop app (local build)

The macOS app shows the same receipts and approvals as `slip status` in one
window: a grid of apps and environments, each cell holding the last code a
receipt records. Select a cell to read its receipt beside the grid. Like
`slip status`, it shows what this Mac recorded, not what a server is running.
It never contacts a server and never runs a deploy; comparing two environments
runs `git` in your local checkout, and next steps are commands you copy into
the terminal.

There is no download yet. To build it you need macOS with the Xcode Command
Line Tools, Rust 1.90 or newer, and Node 26:

```sh
cd app
npm ci
npm run tauri dev                    # run it
npm run tauri build -- --bundles app # build src-tauri/target/release/bundle/macos/Shipslip.app
npm test                             # frontend tests
```

The window refreshes when it gains focus; ⌘R refreshes now. Arrow keys move
through the grid, Enter opens the receipt, and Esc returns to the grid. Below
1200 px wide the receipt opens as a drawer, toggled with ⌘I.

## Post-deploy checks

Shipslip compares log entries against the last 2 MiB of the log and previously
observed error signatures. It watches during the deploy and for 120 seconds
after the steps finish. The receipt groups new errors by exception class and
app file; the displayed line number does not affect the signature. Numbers,
UUIDs, and long or spaced quoted values are normalized, so some distinct
errors may be grouped together. Short quoted identifiers remain distinct, so
request-specific short values may appear as separate variants. "New" means
first observed after this deploy, not caused by it.

When a log cannot be observed, the receipt records `Partial`, `Unavailable`, or
`NoLogSeen` rather than reporting zero new errors. The optional smoke check
follows at most three redirects, verifies TLS, and times out after 10 seconds.
Its HTTP status and latency are saved separately from the deploy outcome. A
failed smoke check does not undo a deploy.

## Config trust

The first use of an environment, or a change to its SSH alias, path, branch,
production setting, recipe, or other configured settings, blocks deploys
until you review it with `slip trust [ENV]`. That command shows changes since
the last approval and asks you to type each environment name. Approvals are
stored per Git repository and environment in a local `trust.json` file under
Shipslip's application support directory. They are not committed to Git.

Shipslip rejects unknown config keys and checks the syntax of each generated
step with local `bash -n` when it loads the config. The server repeats the
syntax check during preflight, before taking the deploy lock. If local bash is
older than 4, as on stock macOS, steps using bash 4 syntax such as `|&` are
left to the server's check. A declined
deploy confirmation releases its lock without running recipe steps.
