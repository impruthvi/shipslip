# Coding standards

Read during review. The rules in `AGENTS.md` under "Rules that must not break" also apply, and a breach of one blocks the change.

Rules marked _(enforced)_ have a check that fails CI; review can skip them.

## Domain language

- Names, comments, UI text and tests use the words in `GLOSSARY.md`. Each entry's _Avoid_ line lists the words that mean something else here: "approval", not "trust", in prose; "environment", not "env".

## App UI

- Colours come only from `app/src/styles.css` `:root` variables. _(enforced: ESLint rejects raw hex)_ Light and dark follow macOS.
- Status colour is earned: red only for a failed step, new errors, or a failed smoke check. Stopped, unknown, unfinished and "maintenance?" are amber. Success is quiet ink, not green.
- Evidence marks say where the code came from (recorded, incomplete, observed, not known, no code); the status word carries severity. Keep the two apart.
- The app says what this Mac recorded and when, never what is on the server right now. _(enforced for "live" wording: `LiveState.test.tsx`)_
- Motion is minimal and off under reduced motion; the running indicator is the only continuous animation.
- The Rust view types are the source of status; the UI maps `OutcomeKind` / `FlagKind` and leaves strings unparsed.

## Tests

- Every failure path the change adds has a test. An untested branch the change adds is a blocker, not a note.
- A check that something is absent targets the element or exact text it would appear in, not a loose pattern over the whole screen: `/only$/` also matches `git merge --ff-only`.
- A workspace flow that moves to another screen asserts that screen's `h1` on arrival. The heading takes focus on every screen change, so a stale one is what a screen reader announces.

## Code

- Small, explicit code over clever abstractions; a few lines over a new dependency.
- Comments only where the code can't say it.
