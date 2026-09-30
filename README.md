# Shipslip

Shipslip runs a configured deploy recipe over SSH. It prepares and displays a
preview before asking for confirmation. Production runs require typing the
environment name.

## Quick start

Put `.shipslip.toml` at the root of your Laravel project's Git repository:

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

[env.production]
ssh = "my-app-production"
path = "/srv/my-app"
branch = "main"
production = true
maintenance = true

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

From anywhere in the project repository, review and approve the config before
the first run:

```sh
cargo run --bin slip -- trust staging
cargo run --bin slip -- deploy staging
```

If installed as a binary, use `slip` in place of `cargo run --bin slip --`.
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

## Receipts and recovery

Each CLI run saves a local receipt before it starts a remote command. On macOS,
receipts live under `~/Library/Application Support/Shipslip/receipts/` in a
directory for the project and environment. The receipt records the approved
plan, exact commits, step results, recent step output, and final outcome.
Treat receipts as private: command output can contain secrets.

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

## Config trust

The first use of an environment, or a change to its SSH alias, path, branch,
production setting, recipe, or other configured settings, blocks deploys
until you review it with `slip trust [ENV]`. That command shows changes since
the last approval and asks you to type each environment name. Approvals are
stored per Git repository and environment in a local `trust.json` file under
Shipslip's application support directory. They are not committed to Git.

Shipslip rejects unknown config keys and checks the syntax of each generated
step with local `bash -n` when it loads the config. The server repeats the
syntax check during preflight, before taking the deploy lock. A declined
deploy confirmation releases its lock without running recipe steps.

The config schema also accepts `log`, `log_daily`, and `smoke_url` for future
log watching and smoke checks. Those checks are not active in this CLI yet.
