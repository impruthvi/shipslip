# Issue tracker: GitHub

Issues and specs live in GitHub Issues for `impruthvi/shipslip`.
Use the `gh` CLI, which infers the repo from the git remote.

## Conventions

- Create: `gh issue create --title "..." --body-file <file>`
- Read: `gh issue view <number> --comments`; fetch labels too.
- List: `gh issue list --state open --json number,title,body,labels,comments`
  with appropriate label and state filters.
- Comment: `gh issue comment <number> --body-file <file>`
- Apply or remove labels: `gh issue edit <number> --add-label "..."` or
  `--remove-label "..."`
- Close: `gh issue close <number> --comment "..."`

Use temporary files with actual newlines for multiline bodies.

## Pull requests as a triage surface

**PRs as a request surface: no.**

GitHub shares issue and PR numbers. If the type is ambiguous, resolve
with `gh pr view <number>` and fall back to `gh issue view <number>`.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.

## Wayfinding operations

Used by `/wayfinder`.

- Map: one issue labelled `wayfinder:map`, holding Notes,
  Decisions-so-far, and Fog.
- Child ticket: link as a GitHub sub-issue. If unavailable, add it to
  a task list in the map and put `Part of #<map>` in its body.
  Use `wayfinder:<type>` labels: research, prototype, grilling, or task.
- Blocking: use native GitHub issue dependencies. Add an edge with
  `gh api --method POST repos/impruthvi/shipslip/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`.
  Obtain the numeric database ID with
  `gh api repos/impruthvi/shipslip/issues/<number> --jq .id`.
  If dependencies are unavailable, use a `Blocked by: #<number>` line.
- Frontier: choose the first open child in map order with no open
  blockers and no assignee. Native `issue_dependencies_summary.blocked_by`
  counts open blockers.
- Claim: assign the child to `@me` before starting work.
- Resolve: comment with the answer, close the child, and append a
  summary and link to the map's Decisions-so-far.
