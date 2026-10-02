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

Shipslip runs a configured deploy recipe over SSH. It prepares and displays a
preview before asking for confirmation. Production runs require typing the
environment name.

New teammate? Follow the [staging setup guide](docs/setup.md) for installation,
SSH access, project configuration, deployment, and recovery.

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

From your Laravel project's Git repository, run `slip init`. It asks for the
environment name, SSH host alias, server path, branch, whether the environment
is production, maintenance mode, and an optional smoke URL, then writes
`.shipslip.toml` at the repository root. It never replaces an existing file.
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
slip logs staging                    # grouped errors from the last 24 hours
slip logs staging --since 7d          # includes earlier daily files
slip logs staging --channels          # files, formats and coverage per channel
slip logs staging qmkte               # latest full entry for a group ID
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
total and per-channel limits (`k`, `m` and `g` use binary units). At most
20 channels and 50 files are selected per snapshot; omissions are reported.
Partial coverage is printed in the header and affected counts use `≥`. Use
`--channels` to inspect each channel's files, total size, last write, format and
coverage. Files that rotate or are truncated during the snapshot are reported as changed. Individual entries
are limited to 256 KiB, with a note in the detail view when truncated.

Files resolving under `storage/logs`, including shared storage symlinks, need
no config approval. Reading another configured path requires `slip trust ENV`.
The server needs Linux with `/proc`, bash, GNU coreutils, gzip and tzdata.
Log output can contain secrets; treat it as private. This snapshot command
does not yet compare errors against a deploy.

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
and final outcome.
Treat receipts as private: command output can contain secrets.

Ctrl-C at the confirmation prompt cancels the run and releases the deploy
lock. During the steps, Ctrl-C starts no new step and stops following the
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
`php artisan up`; it asks for confirmation. Neither command resumes the old
deploy. Check the receipt and server state before choosing a new run plan.

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
