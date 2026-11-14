# Start a new project

Already have a Laravel app on a server? Skip this page and
[try a first deploy](try-shipslip.md). Starting from nothing, Shipslip can
create a verified Laravel app, publish it to GitHub, and hand off to
`slip init`.

## Prerequisites

`slip new` needs:

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

## Check and fix your machine

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

## Create and run an app locally

From a normal directory outside any Git checkout:

```sh
slip new my-app --starter-kit react
cd my-app
php artisan serve
```

Open the URL printed by Artisan. Choose any missing options at the Shipslip
prompts and review the destination, tool versions, and installer command.
Child commands use the tool executables shown in that preview, even when
multiple versions are installed. Shipslip invokes the Laravel installer without
interactive prompts, runs the app's tests, verifies built assets, then shows
the files before the initial Git commit. Installation and verification happen
in a staging directory before the app is moved into place without replacing
files.

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

## Publish to GitHub

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
and requires a new name.

Installer, Composer, and npm processes do not receive `GH_TOKEN`,
`GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`, or
`SHIPSLIP_GITHUB_TOKEN`. GitHub publishing uses your local
GitHub CLI authentication; it does not send publishing credentials to a server
or change global Git credential settings.

## Hand off to deploy config

Creation offers publishing after the initial commit and deploy configuration
at the end. You can decline either and run `slip publish github` or `slip init`
later. When you accept both offers during `slip new`, Shipslip shows the new
config's diff and offers a separate config-only commit, followed by another
publication preview and confirmation. Declining leaves the config local-only.
Other changes are not included. Running `slip init` separately writes local
config only; review and commit it yourself before publishing the new commit.

Publishing a GitHub repository does not deploy it. Before
[configuring deployment](config.md), clone and set up the app on the server
with your normal procedure.
