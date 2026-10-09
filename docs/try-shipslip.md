# Try Shipslip: your first deploy in about 15 minutes

Shipslip (`slip`) deploys a Laravel app to a server you already manage, over
your own `ssh`. Before anything changes it shows exactly which commits and
commands will run, asks you to confirm, runs the steps, watches the Laravel
log for new errors, and saves a receipt of what happened.

This guide gets you from install to a first **staging** deploy. Full details
are in the [setup guide](setup.md).

## What you need

- macOS or Linux with Git, OpenSSH, `bash`, and `curl` (Windows is not
  supported yet).
- A Laravel app in a local Git checkout, pushed to a branch the server can
  fetch.
- A **staging** server where the app is already cloned and running, in one
  directory updated in place (not `releases/` + `current` symlink), and that
  you can reach with `ssh` as the user who owns the app.

Shipslip does not provision servers, store secrets, or forward your SSH agent.
Your keys stay in `~/.ssh`.

## 1. Install (1 minute)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/impruthvi/shipslip/releases/latest/download/shipslip-installer.sh | sh
```

Open a new terminal and check:

```sh
slip --version
```

## 2. Give the server an SSH alias (2 minutes)

Add to `~/.ssh/config` with your real host, user, and key:

```sshconfig
Host my-app-staging
  HostName staging.example.com
  User deploy
  IdentityFile ~/.ssh/id_ed25519
```

Run `ssh my-app-staging` once and verify the host key. Shipslip requires the
host to be in `known_hosts`.

## 3. Describe the deploy (3 minutes)

From your Laravel project's Git checkout:

```sh
slip init
```

Answer the questions (environment name, SSH alias, app path on the server,
branch, production or not, maintenance mode, optional smoke URL). It writes
`.shipslip.toml` with the deploy steps spelled out. **Read the steps before
continuing**: the default recipe runs `php artisan migrate --force`, and if
your project has a `package-lock.json` with a `build` script it also runs
`npm ci` and `npm run build`, which need Node.js and npm **on the server**.
Edit the steps to match how you deploy today.

Then approve the config. Shipslip shows every setting and step; type the
environment name (`staging`) to trust it. Anything else leaves it untrusted,
and deploys stay blocked until you trust it:

```sh
slip trust staging
```

## 4. Deploy staging (5 minutes)

```sh
slip deploy staging
```

Read the preview: current and target commit, the commits being shipped, every
step, maintenance mode, log path, smoke URL. Type `y` to go ahead.

Shipslip then turns maintenance mode on (if enabled), fast-forwards the
checkout, runs each step, turns maintenance mode off, checks the smoke URL,
and watches the log for 120 seconds. Leave the terminal open until it prints
the outcome.

If the server cannot fetch from GitHub, add `--github-token` and paste a
read-only token at the hidden prompt; it is not saved.

## 5. Read the receipt (2 minutes)

```sh
slip receipts                 # every run for this project, newest first
slip receipts show <ID>       # one run: steps, exit codes, output, log watch, smoke
slip receipts show <ID> --md  # a Markdown summary to paste into a PR or chat
slip status                   # last recorded code of every app and env, from any folder
```

Flags in the list tell you what needs a look: `⚠ N new error groups`,
`⚠ smoke 500`, `ⓘ log not fully observed`, `⛔ app in maintenance mode`.

## If something goes wrong

- **A step failed:** `slip receipts show <ID>` says which step and what to do.
  If the cause was on the server, fix it there and run
  `slip from-step staging <N>`. If it needs a code change, push the fix and
  run `slip deploy staging`.
- **The app is still in maintenance mode:** `slip up staging`.
- **You pressed Ctrl-C or lost the connection mid-step:** the step keeps
  running on the server. `slip attach staging` follows it from where it is.
- **"deploy lock is held by …":** another run is active or ended without
  releasing it. If it was yours, `slip attach staging`; otherwise check the
  server, then `slip break-lock staging` if the lock is stale.

Shipslip never retries a command that changes the server, and never reports a
guess as success or failure: when it cannot tell, it says "unknown".

## Tell us how it went

We want the moments where you hesitated, got confused, or reached for plain
`ssh`. Open an issue at <https://github.com/impruthvi/shipslip/issues> or
reply to whoever sent you this guide. Paste `slip receipts show <ID> --md` if
a run surprised you; it leaves out server output and error text by default.
