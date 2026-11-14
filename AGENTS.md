# Shipslip

Deploy with a receipt. A local-first macOS tool for Laravel developers who deploy to servers they manage themselves: preview what ships, run the deploy steps over the user's own `ssh`, watch the Laravel log, and save a receipt of what happened.

Two front-ends share one library: the `slip` CLI and a Tauri app in `app/`. Run `cargo run -- --help` for the CLI commands.

## Commands

Library and CLI (repo root):

```bash
cargo build
cargo run -- <args>                # runs the `slip` CLI
cargo test                         # unit tests (fake Transport, no network)
cargo test --features integration --test integration  # real sshd (needs Docker)
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt
```

App (`app/`):

```bash
npm run tauri dev                  # run the app
npm run typecheck
npm run lint                       # ESLint, zero warnings
npm test                           # Vitest
```

App backend (`app/src-tauri/`): `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
After changing a view type in `app/src-tauri/src/view.rs` or `receipt_view.rs`, regenerate the UI fixtures with `UPDATE_UI_FIXTURES=1 cargo test ui_fixtures` (writes `app/src/fixtures.gen.ts`). Adding a field the app shows: follow the path in `docs/agents/domain.md`.

CI runs all of the above; keep each one clean before committing.

## Layout

```
src/lib.rs            public API: prepare() -> Preview, Confirmation::from(&Preview), execute()
src/config.rs         .shipslip.toml parse/validate, trust store, bash -n
src/transport/        Transport trait; openssh native-mux impl; detached steps + re-attach
src/lock.rs           deploy lock in the remote .git dir, ownership checks
src/engine.rs         preflight, recheck, step 0, steps, maintenance, rerun
src/logs/, logs.rs    log discovery, tail, Laravel log parser, signatures
src/receipt/          receipt schema, durable journal, badge rule, history, markdown
src/bin/slip.rs       CLI: renders DeployEvents, calls only the public API
src/bin/slip/         one file per CLI command
tests/integration/    real-sshd suite (Docker)
app/src-tauri/src/    Tauri commands; view.rs turns library state into UI view types
app/src/              React UI; styles.css holds every colour and size token
```

## Rules that must not break

- The library never reads stdin or prints; front-ends render `DeployEvent`s.
- A deploy cannot run without a `Confirmation` built from the exact `Preview` (env, FROM/TARGET SHA, recipe hash).
- Never auto-retry a remote mutation. Reconnecting to observe a detached step is allowed; re-running it is not.
- Lost connection or unrecoverable step result = outcome `unknown`, never success or failure by guess.
- Check deploy-lock ownership before every step; only release a lock this run owns.
- Write the receipt journal durably (temp file + fsync + rename) before sending each remote step.
- Host keys: runner uses `StrictHostKeyChecking=yes`; no accept-all path anywhere.
- No secrets stored by Shipslip. Keys stay in `~/.ssh` / ssh-agent. No agent forwarding.
- Remote output is untrusted text; never render it as HTML (`react/no-danger` is an ESLint error).
- Every failure path gets a test. Transport behavior (exit codes, disconnect, Ctrl-C, lock break) must be proven against a real sshd before any production deploy.

## App UI

Before UI work, read the App UI section of `CODING_STANDARDS.md`: colour, status and wording rules review holds the change to.

## Conventions

- Work on a feature branch; `.githooks/pre-commit` refuses commits on `main`. Enable it once per clone: `git config core.hooksPath .githooks`.
- Conventional Commits (`feat:`, `fix:`, `test:`, `chore:`, `docs:`).
- Review standards: `CODING_STANDARDS.md`.
- User docs: README holds pitch, fit check, quick start, guarantees and links only. A command's details go in its `docs/<topic>.md` page; a behaviour change updates that page and CHANGELOG, not README.
- README's Guarantees list mirrors "Rules that must not break"; changing one means checking the other.

## Agent docs

- Issues and specs: GitHub Issues for `impruthvi/shipslip`. See `docs/agents/issue-tracker.md`.
- Triage labels: see `docs/agents/triage-labels.md`.
- Domain words: `GLOSSARY.md`. Read its _Avoid_ lines before naming anything. Decisions: `docs/adr/`.
