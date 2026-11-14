# Configuration

Shipslip reads `.shipslip.toml` at the root of your Laravel project's Git
checkout. Each `[env.NAME]` table is one environment: a server reached through
an ssh alias, a path on it, and a branch.

## Before you configure

Configure deployment only after the server has an **existing checkout** of the
project. Clone and configure that app with your normal server procedure,
including PHP, Composer, the database, `.env`, permissions, and web server.
For a frontend build recipe the server also needs Node.js and npm. Shipslip
does not clone an empty server path or provision PHP, databases, a web server,
or a VPS.

The checkout is one directory updated in place (not `releases/` folders with a
`current` symlink). On the server:

- `bash` is installed.
- You SSH in as the user who owns the app directory and can run
  `php artisan` there.
- Laravel writes its log to a file under `storage/logs` that this user can
  read.
- The Git checkout is clean and on the configured branch, and can
  `git fetch origin` without an interactive prompt, or you use a
  [local GitHub token](deploying.md#use-a-local-github-token).

Recipe commands use the server's permission mask. Choose ownership, group
access, and a mask that let PHP-FPM read the files Composer and Artisan create.

## SSH

Add an alias to `~/.ssh/config`, using the server's real host, user, and your
own private key path:

```sshconfig
Host my-app-staging
  HostName staging.example.com
  User deploy
  IdentityFile ~/.ssh/id_ed25519
```

Check the connection with `ssh my-app-staging`. Verify the server's host key
before accepting it. Shipslip uses strict host key checking, so the host must
be in your `known_hosts` file; it never accepts an unknown or changed key for
you ([ADR 0004](adr/0004-host-key-refusals-fail-closed.md)). Shipslip never
writes ssh files ([ADR 0003](adr/0003-shipslip-never-writes-ssh-files.md)),
stores no secrets, and does not forward your agent. Keep your private key out
of Git and out of messages to teammates.

## `slip init`

From your Laravel project's Git repository, run `slip init`. It asks for the
environment name, SSH host alias, server path, branch, whether the environment
is production, maintenance mode, and an optional smoke URL, then writes
`.shipslip.toml` at the repository root with the default Laravel recipe written
out so you can edit it. It never replaces an existing file. Review the file
before the first deploy: the recipe includes `php artisan migrate --force`.

For projects with `package-lock.json` and an npm `build` script, the newly
generated recipe includes `npm ci` and `npm run build` after Composer and
before Artisan optimization. Other package-manager lockfiles produce a
warning without adding build steps. If the frontend manifest or lockfiles
change during setup, restart `slip init` to review the updated recipe. Existing
configs are unchanged.

## The file

Or write the file yourself. Replace every example value, and use the exact
deploy steps that are safe for this app:

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
omitted, Shipslip announces and uses its Laravel default recipe.

When `maintenance = true`, Shipslip runs `php artisan down` before the run and
`php artisan up` after all steps succeed. If a step fails, the app stays down
until you recover it; use `false` if that is not your deploy procedure.

An optional `smoke_url` is requested from **your computer** after the deploy
steps. Shipslip considers an HTTP 2xx response a pass.

Shipslip watches `storage/logs/laravel.log` by default. Set `log` to another
path. With `log_daily = true`, `log` is a directory and prefix (default
`storage/logs/laravel`), and Shipslip follows the newest `laravel-*.log`. Set
`timezone` to the zone Laravel writes log times in (default UTC); see
[logs](logs.md#filters-and-time-zone).

You can commit `.shipslip.toml` so teammates share the same deploy settings.
Each teammate maps the same ssh alias to their own key locally.

## Log channels

`slip logs` discovers channels under `storage/logs`. Override channels by their
original name:

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

## Finding the config

Shipslip searches from the current directory to the Git root for
`.shipslip.toml`. `--config FILE` selects a specific file; `SHIPSLIP_CONFIG`
sets a default override.

## Approval

The first use of an environment, or a change to its SSH alias, path, branch,
production setting, recipe, or other configured settings, blocks deploys
until you approve it with `slip trust [ENV]`. That command shows changes since
the last approval and asks you to type each environment name. Approvals are
stored per Git repository and environment in a local `trust.json` file under
Shipslip's application support directory. They are not committed to Git.

Shipslip rejects unknown config keys and checks the syntax of each generated
step with local `bash -n` when it loads the config. The server repeats the
syntax check during preflight, before taking the deploy lock. If local bash is
older than 4, as on stock macOS, steps using bash 4 syntax such as `|&` are
left to the server's check. A declined
deploy confirmation releases its lock without running recipe steps.
