<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/impruthvi/shipslip/main/assets/logo-dark.png">
    <img src="https://raw.githubusercontent.com/impruthvi/shipslip/main/assets/logo-light.png" alt="Shipslip" width="400">
  </picture>
</h1>

<p align="center">
  <a href="https://crates.io/crates/shipslip"><img src="https://img.shields.io/crates/v/shipslip" alt="Crates.io version"></a>
  <a href="https://github.com/impruthvi/shipslip/actions/workflows/ci.yml"><img src="https://github.com/impruthvi/shipslip/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status on main"></a>
  <a href="https://github.com/impruthvi/shipslip/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/shipslip" alt="License"></a>
</p>

<p align="center"><strong>Deploy with a receipt.</strong></p>

You deploy by ssh-ing in, running `git pull` and a few artisan commands, and
watching the scrollback. When something goes wrong (a migration fails, the
connection drops mid-step, errors start showing up in the log), you're left
reconstructing what happened from memory.

Shipslip runs those same steps over your own `ssh`. First it shows exactly
which commits and commands will ship and asks you to confirm. Then it runs
them, watches the Laravel log for new errors, and saves a receipt of what
happened. If it can't tell how something ended, the receipt says _unknown_,
not a guess. There's no agent to install on the server, and your keys stay in
`~/.ssh`.

Shipslip is pre-1.0: a minor release may change the library API. See the
[changelog](https://github.com/impruthvi/shipslip/blob/main/CHANGELOG.md).

## What it looks like

A real staging deploy (command output trimmed, server address replaced):

```console
$ slip deploy staging
Taking the deploy lock…
Environment: staging
Path:        /var/www/laravel-staging
Current SHA: 26b4ad54b331b12a89b4f91faaeb6bb856c33165
Target SHA:  24971f3eada04c27e408f193aa3c23b98136372b
Commits to deploy:
  24971f3 Add /health route for the smoke check
Recipe steps:
  1. composer install --no-dev --no-interaction --prefer-dist --optimize-autoloader
  2. php artisan migrate --force
  3. php artisan optimize
  4. php artisan queue:restart
Maintenance: enabled
Log watch:   storage/logs/laravel.log (120 s after steps)
Smoke URL:   https://staging.example.com/
Proceed with this run? [y/N] y
Maintenance on: succeeded (exit 0) in 2s
Step 0: succeeded (exit 0) in 2s
Step 1: succeeded (exit 0) in 2s
Step 2: succeeded (exit 0) in 2s
Step 3: succeeded (exit 0) in 2s
Step 4: succeeded (exit 0) in 2s
Maintenance off: succeeded (exit 0) in 2s
Smoke check: HTTP 200 in 652 ms
Log watch: complete, no new errors
Deploy run succeeded.

$ slip receipts staging
ID        STARTED (IST)     ENV      FROM → TARGET                OUTCOME
18e7eb79  2026-11-14 18:41  staging  26b4ad54b331 → 24971f3eada0  Succeeded
```

## Is it for you?

Shipslip fits if:

- Your Laravel app is already cloned and running on a Linux server you reach
  with `ssh`.
- Deploying means updating that one checkout in place with `git`, not
  `releases/` folders with a `current` symlink.
- You work on macOS or Linux. Windows is not supported yet.

It does not provision servers, install PHP or databases, clone an app onto an
empty server, or do zero-downtime symlink releases. Starting from nothing?
[Start a new project](https://github.com/impruthvi/shipslip/blob/main/docs/new-project.md).

## Quick start

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/impruthvi/shipslip/releases/latest/download/shipslip-installer.sh | sh
```

Then, from your Laravel project's checkout, with an ssh alias for the server
in `~/.ssh/config`:

```sh
slip init               # a few questions; writes .shipslip.toml
slip trust staging      # review and approve the config
slip deploy staging     # preview, confirm, run
slip receipts           # what happened
```

**Read the recipe in `.shipslip.toml` before you deploy.** The default runs
`php artisan migrate --force`, and if your project builds assets with npm it
also runs `npm ci` and `npm run build` on the server.

Step by step, with what to do when something goes wrong:
[Try Shipslip](https://github.com/impruthvi/shipslip/blob/main/docs/try-shipslip.md),
a first staging deploy in about 15 minutes.

## Guarantees

- **Nothing runs until you confirm the exact preview**: commits, steps and
  server. Production asks you to type the environment name.
  ([Deploying](https://github.com/impruthvi/shipslip/blob/main/docs/deploying.md))
- **No retries.** Shipslip never re-sends a command that changes the server.
  After a dropped connection it re-attaches to watch the step; it never runs
  it again. ([Recovery](https://github.com/impruthvi/shipslip/blob/main/docs/deploying.md#recovery))
- **Unknown stays unknown.** When Shipslip can't tell how a step ended, the
  outcome is unknown, never a guessed success or failure.
  ([Receipts](https://github.com/impruthvi/shipslip/blob/main/docs/receipts.md))
- **The receipt comes first.** Each step is recorded on disk before it is
  sent, so a crash or lost connection still leaves a record.
  ([Receipts](https://github.com/impruthvi/shipslip/blob/main/docs/receipts.md))
- **One run per environment.** A deploy lock on the server keeps runs apart.
  Shipslip checks it owns the lock before every step and only releases its
  own. ([Deploying](https://github.com/impruthvi/shipslip/blob/main/docs/deploying.md))
- **Config changes need approval.** A new or changed server, path, branch or
  recipe blocks deploys until you approve it again.
  ([Approval](https://github.com/impruthvi/shipslip/blob/main/docs/config.md#approval))
- **Your ssh, your keys.** Strict host-key checking with no accept-all path,
  no stored secrets (GitHub tokens included), no agent forwarding, and
  Shipslip never writes your ssh files.
  ([SSH](https://github.com/impruthvi/shipslip/blob/main/docs/config.md#ssh))
- **Read-only commands stay read-only.** `slip status` and `slip receipts`
  never contact a server, `slip logs` never changes one, and the MCP server
  cannot deploy. ([MCP server](https://github.com/impruthvi/shipslip/blob/main/docs/mcp.md))

## More

- [Configuration](https://github.com/impruthvi/shipslip/blob/main/docs/config.md): `.shipslip.toml`, server requirements, log channels, approval.
- [Deploying](https://github.com/impruthvi/shipslip/blob/main/docs/deploying.md): kinds of run, GitHub tokens, log watch and smoke check, recovery.
- [Receipts and status](https://github.com/impruthvi/shipslip/blob/main/docs/receipts.md): past runs, Markdown to share, and the last recorded code of every app.
- [Logs](https://github.com/impruthvi/shipslip/blob/main/docs/logs.md): grouped Laravel errors since the last deploy, read-only.
- [MCP server](https://github.com/impruthvi/shipslip/blob/main/docs/mcp.md): let an agent read your receipts and logs.
- [Desktop app](https://github.com/impruthvi/shipslip/blob/main/docs/app.md): the same deploys in a macOS app. No download yet; build from source.
- [Start a new project](https://github.com/impruthvi/shipslip/blob/main/docs/new-project.md): `slip new`, `slip publish github`, `slip doctor`.
- [JSON reference](https://github.com/impruthvi/shipslip/blob/main/docs/json-reference.md) for scripts.
- `slip --help` lists every command.

## Contributing

See [CONTRIBUTING.md](https://github.com/impruthvi/shipslip/blob/main/CONTRIBUTING.md).
Shipslip is [MIT licensed](https://github.com/impruthvi/shipslip/blob/main/LICENSE).
