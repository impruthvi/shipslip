# Set up Shipslip for a Laravel project

This guide is for a teammate making a first staging deploy. Shipslip runs on
your macOS or Linux computer and connects to a Linux server over SSH. Start
with staging; configure production separately after the staging flow works.

## 1. Get access and install Shipslip

You need a local Git checkout of the Laravel project, SSH access to its
staging server, and permission for the server to fetch the project's Git
repository. The server must already have the project checked out, in one
directory updated in place (not `releases/` folders with a `current`
symlink).

On the server:

- `bash` is installed.
- You SSH in as the user who owns the app directory and can run
  `php artisan` there.
- Laravel writes its log to a file under `storage/logs` that this user can
  read.

On your computer, you need macOS or Linux with Git, OpenSSH, `bash`, and
`curl`. Windows is not supported yet. Install the Shipslip CLI:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/impruthvi/shipslip/releases/latest/download/shipslip-installer.sh | sh
```

Open a new terminal, then check the install:

```sh
slip --version
```

The installer puts `slip` in `~/.local/bin` and adds that directory to your
`PATH` in your shell's startup files. If a new terminal still cannot find
`slip`, add `~/.local/bin` to your `PATH` yourself. Run the installer again to
upgrade.

If you have Rust 1.88 or newer, you can install from
[crates.io](https://crates.io/crates/shipslip) instead with
`cargo install shipslip --locked` (add `--force` to upgrade). To try
unreleased changes, install from the
[source repository](https://github.com/impruthvi/shipslip):
`cargo install --git https://github.com/impruthvi/shipslip.git --branch main --locked --bin slip`.

## 2. Configure SSH

Add an alias to `~/.ssh/config`, using the staging server's real host, user,
and your own private key path:

```sshconfig
Host my-app-staging
  HostName staging.example.com
  User deploy
  IdentityFile ~/.ssh/id_ed25519
```

Check the connection with `ssh my-app-staging`. Verify the server's host key
before accepting it. Shipslip uses strict host key checking, so the host must
be in your `known_hosts` file. Keep your private key out of Git and out of
messages to teammates.

The server's deploy user needs permission to run the recipe commands and read
Laravel's log. Its Git checkout must be clean, on the configured branch, and
able to `git fetch origin` without an interactive prompt.

## 3. Add the project config

From the **Laravel project's Git checkout**, run:

```sh
slip init
```

It asks a few questions and writes `.shipslip.toml` at the Git root, with the
default Laravel recipe written out so you can edit it. Review the file before
the first deploy: the recipe includes `php artisan migrate --force`.

To write the file by hand instead, create `.shipslip.toml` in the **Laravel
project's Git root**. Replace every example value below. Use the exact deploy
steps that are safe for this app; the example includes a database migration.

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

[recipe.deploy]
steps = [
  "composer install --no-interaction --no-dev --prefer-dist",
  "php artisan migrate --force",
  "php artisan config:cache",
]
```

`ssh` names the alias from step 2. `path` is the absolute path of the Git
checkout on the server. `maintenance = true` takes the app down before the
steps and brings it up after they succeed. If a step fails, the app stays
down until you recover it; use `false` if that is not your deploy procedure.
`smoke_url` is optional and must be reachable from **your computer**. Shipslip
considers an HTTP 2xx response a pass.

Shipslip watches `storage/logs/laravel.log` by default. Set `log` to a
different path if your app uses one. For Laravel daily logs, set
`log_daily = true`; its default prefix is `storage/logs/laravel`, which follows
the newest `laravel-*.log`. If you omit `[recipe.deploy]`, Shipslip uses its
default Laravel recipe; review that recipe before deploying.

You can commit `.shipslip.toml` so teammates share the same deploy settings.
Each teammate can map the same SSH alias to their own key locally.

## 4. Review and deploy staging

Push the intended app commit to the branch named in `.shipslip.toml`. From
anywhere inside your **local Laravel project checkout**, run:

```sh
slip trust staging
slip deploy staging
```

`trust` shows the config and asks you to approve it. `deploy` fetches the
configured branch on the server, previews the exact commits and steps, and
asks for confirmation before making changes. Read that preview. If you change
the config later, run `slip trust staging` again before deploying.

Leave the terminal open for the deploy and the 120-second log watch; Shipslip
prints the time left every 30 seconds, and Ctrl-C ends the watch early. Check
the final deploy outcome, the `Log watch` line, and (if configured) the smoke
check's HTTP result. `complete` means Shipslip observed the log; `no log file
found`, `log could not be read`, or `partial` means you should inspect the log
path, permissions, or format. A failed smoke check is recorded separately and does not roll back
the deploy.

Shipslip saves a receipt on your computer. On macOS, look under
`~/Library/Application Support/Shipslip/receipts/`; on Linux, under
`~/.local/share/shipslip/receipts/`. Receipts can contain command output and
should be treated as private.

## If the command is interrupted

Ctrl-C at the confirmation prompt cancels cleanly. During the steps, it stops
following the running command, which keeps running on the server. To resume,
from the **same local project checkout**, run:

```sh
slip attach staging
```

This resumes observation of an unfinished run without relaunching its active
server command. Use `slip rerun staging` only when the server is already on
the intended clean commit and repeating every recipe step is safe.

If `attach` cannot resolve an unknown outcome and the lock has gone stale,
connect with `ssh my-app-staging` and check that no command from the old run is
still running. After that, run `slip break-lock staging` to clear the stale
lock. If the app may still be in maintenance mode, run `slip up staging` to
bring it back. Both commands require the trusted project config and ask for
confirmation; `up` takes a new deploy lock.
