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
After changing a view type in `app/src-tauri/src/view.rs` or `receipt_view.rs`, regenerate the UI fixtures with `UPDATE_UI_FIXTURES=1 cargo test ui_fixtures` (writes `app/src/fixtures.gen.ts`).

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

## App UI rules

- Colours come only from `app/src/styles.css` `:root` variables; ESLint rejects raw hex values elsewhere. Light and dark follow macOS.
- Status colour is earned: red only for a failed step, new errors, or a failed smoke check. Stopped, unknown, unfinished and "maintenance?" are amber. Success is quiet ink, not green.
- Evidence marks say where the code came from (recorded, incomplete, observed, not known, no code); the status word carries severity. Don't merge the two.
- Never claim the app knows what is on the server right now. No "live" wording (`LiveState.test.tsx` enforces it); say what this Mac recorded and when.
- Motion is minimal and off under reduced motion; the running indicator is the only continuous animation.
- Keep the Rust view types as the source of status; the UI maps `OutcomeKind` / `FlagKind`, it doesn't parse strings.

## Conventions

- Rust 2021, `cargo fmt` + `clippy -D warnings` clean before commit.
- Conventional Commits (`feat:`, `fix:`, `test:`, `chore:`, `docs:`).
- Small, explicit code over clever abstractions; no new dependency when a few lines will do.
- Comment only where the code can't say it.

## Agent docs

- Issues and specs: GitHub Issues for `impruthvi/shipslip`. See `docs/agents/issue-tracker.md`.
- Triage labels: see `docs/agents/triage-labels.md`.
- Domain vocabulary: root `CONTEXT.md` and `docs/adr/` when they exist. See `docs/agents/domain.md`.
