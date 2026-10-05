# Set up Shipslip for a Laravel project

This guide covers creating a local Laravel app, optionally publishing it, and
making a first staging deploy. Shipslip runs on your macOS or Linux computer
and connects to a Linux server over SSH. Start with staging; configure
production separately after the staging flow works.

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

### Local project creation prerequisites

Creation and GitHub publishing require Shipslip **0.3.0 or newer**;
`slip doctor`, `slip setup`, and in-place repair require **0.4.0 or newer**.
Run the GitHub installer above to upgrade. To test from source, build with
`cargo build --locked --bin slip` and use that `target/debug/slip` binary from
a non-Git parent directory. The README has a
[complete local example](../README.md#try-project-creation-from-this-checkout).

Skip this section when you already have a Laravel checkout. `slip new` needs:

- PHP with the extensions Laravel uses, including `pdo_sqlite`, which the new
  app's tests use. The driver for another selected database (`pdo_mysql`,
  `pdo_pgsql`, `pdo_sqlsrv`) is reported as a warning only. The compatibility
  matrix was verified with PHP 8.4.
- Composer.
- Laravel installer 5.31.1 or newer. Shipslip also probes the installer's help
  output for the required non-interactive flags.
- Node.js and npm, including for plain Laravel's Vite asset build. Other
  package managers are not supported.
- Git, with an author name and email configured for the initial commit.

Publishing also needs the GitHub CLI (`gh`) logged in to github.com, or
`GH_TOKEN` set.

### Check and fix your machine

Run doctor from any directory. It changes nothing:

```sh
slip doctor
```

Doctor reports every requirement at once and exits `0` when ready, `3` when a
blocking requirement is missing, too old, or broken, `1` if doctor itself
fails, and `2` for invalid input. `--for create` or `--for publish` narrows
the check; `--json` prints versioned output for scripts. The GitHub login check
gives up after 10 seconds and shows a warning instead of waiting.

A tool installed outside your `PATH`, such as the Laravel installer in
Composer's global bin directory, still counts: Shipslip uses it and shows the
exact line to add to your shell startup file so your own terminal finds it too.

On macOS, fix what is missing with:

```sh
slip setup
```

Setup prints a plan of exact commands, asks once, runs them, and checks again:

- Missing tools are installed with your existing Homebrew, with automatic
  updates and upgrades of installed formulae turned off. Homebrew may still
  install the new formula's own dependencies.
- The Laravel installer is installed with `composer global require
  laravel/installer`.
- A missing Git name or email is asked for before the plan and saved first.

Shipslip never installs Homebrew, never runs `curl | bash`, never edits shell
startup files, and never changes tools owned by Herd, php.new, nvm, fnm, asdf,
or mise; for those it prints the manager's own command. Tools that are present
but too old or broken get instructions instead of upgrades, because upgrading a
shared Homebrew formula affects every project using it. If Homebrew is missing
or not writable by you, setup prints guidance and changes nothing.

When there is work to do, setup keeps a private record of the plan, command
output, and exit codes under Shipslip's operations directory. Press Ctrl-C once to stop after the
current command; press it again to exit immediately. Setup exits `0` when
ready, `3` when something still needs you (including a declined plan), `1`
when a command fails or another setup is running, and `130` when
interrupted. Run `slip setup` again to continue; it re-checks first.

`slip new` and `slip publish github` run the same checks. When something is
missing they show only what needs attention, offer the same plan with one
confirmation, and then continue with the answers you already gave. If you
decline or the repair fails, they print the exact command to run again.
`slip new` also warns before installing when publishing will later need `gh`
or a GitHub login.

On Linux, doctor works and setup prints manual guidance. Install the tools with
your distribution's packages, then the Laravel installer and Git identity:

```sh
composer global require laravel/installer
git config --global user.name "Your Name"
git config --global user.email "you@example.com"
```

### Create and run an app locally

From a normal directory outside any Git checkout:

```sh
slip new my-app --starter-kit react
cd my-app
php artisan serve
```

Open the URL printed by Artisan. Choose any missing options at the Shipslip
prompts and review the destination, tool versions, and installer command.
Shipslip invokes the Laravel installer without interactive prompts, runs the
app's tests, verifies built assets, then shows the files before the initial
Git commit. Installation and verification happen in a staging directory before
the app is moved into place without replacing files.

Available starter kits are `none`, `react`, `vue`, `svelte`, and `livewire`.
Authentication is `laravel` or `none`; kit `none` requires auth `none`.
Databases are `sqlite`, `mysql`, `mariadb`, `pgsql`, and `sqlsrv`; testing is
`pest` or `phpunit`. Defaults are SQLite, Pest, and branch `main`.
Laravel Boost is recommended and its setup prompt defaults to Yes (Enter).
Answer No to skip it. Pass `--boost` or `--no-boost` to set that choice without
being prompted, and `--branch NAME` for another initial branch.
Boost configures detected AI clients. Run `php artisan boost:install` in the
new app to choose or add a client after creation.
WorkOS, community kits, teams, and VPS provisioning are outside this flow.

Use `slip new .` for an existing empty directory. Hidden files count as
contents, symlink destinations are refused, and either mode refuses a target
inside another Git repository. There is no force or overwrite option. Missing
choices in a non-interactive shell fail immediately with the flag to supply.
Creation and publishing still require an interactive terminal even when all
flags are supplied: there is no unattended confirmation flag.

The staging container is `<parent>/.slip-new-<operation-id>/`; its private
ownership marker sits beside a `project/` child containing the actual Laravel
scaffold. A failed installer or verification, or cancellation during those
stages, marks the journal `scaffold_failed`, reports the container path, and
offers to delete only that operation's staging container. The private operation
record remains. A move conflict retains staging for inspection without offering
automatic cleanup; no conflicting destination entry is replaced.

The CLI prints the operation record path. Records live under
`~/Library/Application Support/Shipslip/operations/` on macOS or
`~/.local/share/shipslip/operations/` on Linux and record phases, paths, and
choices without tokens or `.env` contents. An initial Git failure can leave
the verified app locally; read the reported fix before completing Git setup.
Declining the initial commit also leaves the verified app and staged files
local, and skips the immediate publishing offer.

### Optional: publish to GitHub

Publishing needs the GitHub CLI on your computer and an authenticated account
with permission to create a repository under the selected owner. It is not
required for local creation or existing deployment flows:

```sh
gh --version
gh auth login
slip publish github --repo my-app
```

Shipslip shows the actual authenticated account, requested owner/name,
visibility, HEAD, and commit count. Review and separately confirm publication.
Visibility defaults to `private`; use `--visibility public` explicitly to make
a public repository and `--owner OWNER` for a different owner. `slip new` also
offers this flow after local creation, and declining leaves the app local.
Publishing also requires an interactive terminal to review and confirm the
account and intent, including when all publishing flags are supplied.

Before pushing, Shipslip scans every path in the pushed history for `.env*`
(except `.env.example`), `auth.json`, `*.pem`, `*.key`, and `id_rsa*`. A secret
filename in an older commit still blocks even if it has since been deleted;
the error identifies its path and commit. Remove the sensitive file from the
history you intend to publish before trying again. This filename check does
not replace reviewing your code for credentials.

An existing `origin` must identify the selected repository; Shipslip will not
replace it, and the preview shows its actual SSH or HTTPS transport. The
active GitHub account is used without switching accounts. Before resuming or
pushing, Shipslip checks the remote repository's current visibility against
your reviewed choice; a mismatch stops publication. Publication never force-pushes or deletes a remote repository,
and verifies the remote branch SHA against local HEAD. If interrupted, run
`slip publish github` again: a saved operation can retry its push, or adopt
its own marked, still-empty repository after an uncertain create response.
An existing repository with a different marker or unexpected refs is unresolved
and requires a new name. Shipslip does not switch GitHub accounts for you.

Installer, Composer, and npm processes do not receive `GH_TOKEN`,
`GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`, or
`SHIPSLIP_GITHUB_TOKEN`. GitHub publishing uses your local
GitHub CLI authentication; it does not send publishing credentials to a server
or change global Git credential settings.

### Prepare the server checkout before deployment

The remaining steps require an existing Laravel Git checkout on the Linux
server at the configured path. Clone and configure that app separately with
your normal server procedure, including PHP, Composer, the database, `.env`,
permissions, and web server. For a frontend build recipe the server also needs
Node.js and npm. Shipslip currently neither bootstraps an empty server path
nor provisions a VPS. Publishing a GitHub repository does not deploy it.

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
Laravel's log. Its Git checkout must be clean and on the configured branch.
For GitHub access, either configure the server to `git fetch origin` without
an interactive prompt, or use a local token as described below.

Recipe commands use the server's permission mask. Choose ownership, group
access, and a mask that let PHP-FPM read the files Composer and Artisan create.
Shipslip 0.2.0 fixes earlier versions imposing `077` on recipe commands; the
upgrade does not repair existing file permissions. If an earlier deploy left
unreadable files, review access to `vendor`, `bootstrap/cache`, and
`storage/framework`, then regenerate Laravel's caches with your recipe.

## 3. Add the project config

From the **Laravel project's Git checkout**, run:

```sh
slip init
```

It asks a few questions and writes `.shipslip.toml` at the Git root, with the
default Laravel recipe written out so you can edit it. Review the file before
the first deploy: the recipe includes `php artisan migrate --force`.

When the local project has `package-lock.json` and a `build` script in
`package.json`, the generated recipe adds `npm ci` and `npm run build` after
Composer and before `php artisan optimize`. Other package-manager lockfiles
produce a warning without an automatic build step. This affects only newly
written configs; `slip init` still refuses to replace an existing config and
requires a Git checkout. If `package.json` or a package-manager lockfile changes
while the wizard is open, no config is written; run `slip init` again to review
the current recipe.

The creation flow can hand off to this wizard after local verification and
optional publishing. When you publish and then configure in the same
`slip new` flow, Shipslip shows the new config diff and offers a separate
config-only commit, then previews the resulting publication and asks again
before pushing. Declining leaves the config local-only. Other changes are not
committed. Running `slip init` separately writes local config only; review and
commit it yourself before publishing the new commit.

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

If the server has no GitHub credentials, use:

```sh
slip deploy staging --github-token
```

Paste a GitHub personal access token at the hidden prompt. Shipslip shows its
actual account username and the server's `OWNER/REPO` before asking you to use
it. Enter `r` to replace a token from the wrong account, or cancel. This works
without a GitHub CLI login and does not silently use your personal `gh` account.
The prompt includes token-creation guidance: select the repository with
Contents: Read-only and an expiry; your organization may require approval or
SSO authorization. Repository visibility is checked locally; the server fetch
verifies read access before showing the normal deployment preview.

To use existing local credentials explicitly, choose `--github-token-source
env` (`GH_TOKEN`, then `GITHUB_TOKEN`) or `--github-token-source gh` (the active
github.com GitHub CLI login). Both still show the actual account and require
confirmation. Token authentication also works with `rerun` and `from-step`.

This flow requires local `curl` and a standard github.com HTTPS or SSH origin.
The token reaches the trusted server briefly in memory over SSH and is not
saved by Shipslip. The saved Git remote remains unchanged; receipts and
detached scripts do not contain credentials. The token is used only for the
application repository's fetch. Configure private Composer dependencies and
submodules separately. See the [README](../README.md#use-a-local-github-token)
for source selection and supported remotes.

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

## 5. Read logs

From the same local project checkout, inspect logs without deploying:

```sh
slip logs staging
slip logs staging --since 7d
slip logs staging --channels
slip logs staging --raw --grep payment
```

The command discovers `storage/logs/*.log` channels and also reads configured
`log` / `log_daily` paths. Single and daily files with the same channel name
merge. It shows grouped errors since the latest verified run by default,
falling back to the latest checkout change or 24 hours. Use `--since` to choose
a window. `NEW` means absent from the bounded comparison before that window;
`?` means no usable baseline. The header discloses that coverage. Copy a group
ID into `slip logs staging ID` to see its latest stack trace.
Set `timezone` in `[env.staging]` to Laravel's log timezone, for example
`timezone = "Asia/Kolkata"`; it defaults to UTC. The server must have Linux
`/proc`, bash, GNU coreutils, gzip and tzdata installed.

Check coverage warnings: `≥` means only part of the window was read. Use
`--channels` to inspect each channel's files, format and coverage. The default
window read limits are 4 MiB per channel and 12 MiB total, shared fairly.
A separate comparison budget allows 2 MiB per channel and 6 MiB total.
`--channels` shows both coverages.
`--max-bytes 20m` changes the window limits. Mixed or unsupported formats stay
visible through `--raw`, where their file blocks bypass timestamp and level
filtering. Configure `hide`, `rename` or `path` under
`[env.staging.logs.CHANNEL]` to override discovery (see the README). Files outside
`storage/logs` require config approval through `slip trust staging`.
Reading logs runs no deploy steps and writes no receipts or signature history.
Deploys record the run anchor in `<git dir>/shipslip.last-run`; failures to write
it are warnings, and deployment and lock release continue.
Log output can contain secrets.

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
