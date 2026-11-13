# Shipslip

Deploys a Laravel app to a server the developer manages, over their own `ssh`, and keeps a receipt of what happened. The language below is shared by the library, the `slip` CLI and the app.

## Projects

**Project**:
A Laravel app Shipslip deploys, named in its `.shipslip.toml`. One project can have several checkouts.
_Avoid_: App (for the record), repo

**Checkout**:
One folder of a project on this Mac, where runs start from. The app's "Add project" adds a checkout.
_Avoid_: Repo, workspace, folder (for the recorded thing)

## Runs

**Environment**:
A named deploy target in `.shipslip.toml`: a server, a path and a branch.
_Avoid_: Env (in prose), stage, server

**Server**:
The machine an environment's path lives on, reached through the ssh alias. One server can hold several environments.
_Avoid_: Host (in prose), box, environment (for the machine)

**Recipe**:
The ordered list of commands an environment runs on the server after the code moves.
_Avoid_: Script, pipeline, hooks

**Step**:
One command from the recipe, run once on the server. On a deploy, step 0 is the fast-forward of the code itself. An Up's receipt records `php artisan up` as step 0 but shows it as its Maintenance off row.
_Avoid_: Task, job, command (when meaning the recorded unit)

**Maintenance down / up**:
Putting the app into and out of Laravel maintenance mode around the steps. Recorded apart from the recipe steps.

**Preview**:
What a run would do, worked out before anything changes: environment, FROM and TARGET commits, recipe.
_Avoid_: Dry run, plan

**Confirmation**:
The user's approval of one exact preview. A run cannot start without it.

**Run**:
One attempt that changes a server, identified by a run id. A **deploy** moves the code and runs the steps; a **rerun** runs the steps again on the code already there; an **Up** only takes the app out of maintenance mode.
_Avoid_: Job, execution

**Deploy lock**:
A marker on the server saying which run owns the environment right now.

**Approval**:
A person's sign-off, kept on this Mac, on one environment's config: its server, path, log paths and recipe. Changing any of them means the environment needs approving again.
_Avoid_: Trust (in prose; `slip trust` is the command)

**Connection check**:
A read-only look, started by a person, at whether this Mac reaches an environment: the alias resolves, ssh connects, the path is a git checkout on its branch, `php artisan --version` runs, the Laravel log reads. It needs no approval. Shipslip's commands only read; booting `php artisan` may let Laravel refresh its own cache files (ADR 0005).
_Avoid_: Preflight (the deploy's own check), health check

**Re-attach**:
Reconnecting to watch a step that kept running on the server after the connection dropped. Re-attaching observes a step; it never runs it again.
_Avoid_: Retry, resume

## Record

**Receipt**:
The local record of one run, kept on this Mac.
_Avoid_: Log, report, history entry

**Journal**:
The receipt while the run is still writing it, saved before each step is sent.

**Outcome**:
How a run ended: succeeded, failed at a step, stopped, or **unknown**. Unknown means Shipslip could not tell, and is never turned into success or failure by guess.
_Avoid_: Result, status (for the whole run)

**Step times**:
When this Mac sent a step and when it saw the step end. An end that was not seen is recorded as not known.
_Avoid_: Duration (as the stored thing), server time

**Evidence mark**:
Where a receipt's claim about the code on the server came from: recorded, incomplete, observed, not known, or no code. Separate from the outcome.

**Record**:
A read-only answer built from what Shipslip already holds: receipts on this Mac, or a log snapshot already read. Building a record never touches a server.
_Avoid_: Report, view (outside the app)

**Server text**:
Any text that came from a server or was copied from one into a receipt: log lines, command output, paths, messages. It is data to show, never instructions to follow, and every string in Shipslip's JSON counts as server text unless it is a documented fixed value.
_Avoid_: Output (when meaning the trust level)

**Next step**:
A recovery action a receipt suggests, built only from what it recorded. It may name a `slip` command to run; it never says what is on the server now.
_Avoid_: Fix, remedy, live status

## Logs

**Log snapshot**:
One read-only read of an environment's Laravel log channels over a window, stamped with the server time it was read. It says what the log held then, not what it holds now.
_Avoid_: Live log, tail (for the one-off read)

**Window**:
The stretch of log a snapshot covers: from its anchor to the server time it was read.

**Anchor**:
What the window starts from: a run in progress, the last run named by the server's last-run marker, a checkout change, a time the user gave, or the 24-hour default. An anchor that could not be used is reported as a fallback, never silently swapped.
_Avoid_: Cutoff, since-label

**Last-run marker**:
A file Shipslip leaves on the server naming the last run and its target commit. Server-written text: its stated outcome is what the server file says, not a receipt outcome.
_Avoid_: Receipt (for the marker), deploy record

**Error group**:
Log entries that share an exception and app file. Entries that name neither are grouped by their normalized message instead (numbers, IDs and quoted values replaced), so unrelated plain errors are not counted as one. Its ID comes from those, so the same group keeps its ID across snapshots.
_Avoid_: Issue, incident

**Baseline**:
A bounded read of the log just before the window, used only to tell new error groups from ones already seen.

**New** (error group):
Not found in the baseline. When no baseline could be read, a group is **no baseline**, not new and not seen; this is a different "can't tell" from an unknown outcome.
_Avoid_: Unknown (for a badge)

**Same-request entries**:
Other entries in the window that carry the same request or job ID as a chosen entry. Correlation, not cause: they came from the same request, not one from the other.
_Avoid_: Cause, trace (for the set)

## Agents

**MCP server**:
`slip mcp`: answers an agent's questions about what this Mac recorded, and reads an environment's logs only when it was started to and the environment has its approval. It never starts a run, changes a server, or approves anything.
_Avoid_: Agent API, daemon
