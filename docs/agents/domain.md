# Domain Docs

This repo uses a single-context layout.

## Before exploring, read these

- `CONTEXT.md` at the repo root.
- Relevant ADRs under `docs/adr/`.

If they don't exist, proceed silently. Don't flag their absence or
suggest creating them upfront. `/domain-modeling` creates them lazily
when terms or decisions get resolved.

## File structure

- `/CONTEXT.md`: domain glossary and context.
- `/docs/adr/`: numbered architecture decision records.
- `/src/`: implementation.

## Use the glossary's vocabulary

Use terms from `CONTEXT.md` in issue titles, proposals, hypotheses,
and tests. Avoid synonyms the glossary explicitly excludes.

If a needed concept is missing, reconsider whether it belongs or
note the gap for `/domain-modeling`.

## Flag ADR conflicts

Surface contradictions explicitly, identifying the ADR and why
reopening the decision may be warranted.
