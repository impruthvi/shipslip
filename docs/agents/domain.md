# Domain Docs

This repo uses a single-context layout.

## Before exploring, read these

- `GLOSSARY.md` at the repo root.
- Relevant ADRs under `docs/adr/`.

If they don't exist, proceed silently. Don't flag their absence or
suggest creating them upfront. `/domain-modeling` creates them lazily
when terms or decisions get resolved.

## File structure

- `/GLOSSARY.md`: domain glossary and context.
- `/docs/adr/`: numbered architecture decision records.
- `/src/`: implementation.

## Adding a field the app shows

A new field crosses every layer, in this order:

1. Library record: `src/records/` (`ReceiptRecord`, `AttemptSummary`). New JSON fields take `skip_serializing_if = "Option::is_none"` so the record schema test holds.
2. App view: `app/src-tauri/src/view.rs`, `receipt_view.rs`, or `deploy_session.rs` for deploy screens.
3. UI type: `app/src/types.ts`, by hand.
4. Fixture: a new state gets an export in `app/src-tauri/src/ui_fixtures.rs` (deploy screens: `sample_deploy_views` in `deploy_session.rs`), then `UPDATE_UI_FIXTURES=1 cargo test ui_fixtures`. tsc now checks step 3 against what Rust sends.
5. Component, and a Vitest test that renders the fixture.

## Use the glossary's vocabulary

Use terms from `GLOSSARY.md` in issue titles, proposals, hypotheses,
and tests. Avoid synonyms the glossary explicitly excludes.

If a needed concept is missing, reconsider whether it belongs or
note the gap for `/domain-modeling`.

## Flag ADR conflicts

Surface contradictions explicitly, identifying the ADR and why
reopening the decision may be warranted.
