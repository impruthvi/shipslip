# Shipslip

Deploys a Laravel app to a server the developer manages, over their own `ssh`, and keeps a receipt of what happened. The language below is shared by the library, the `slip` CLI and the app.

## Runs

**Environment**:
A named deploy target in `.shipslip.toml`: a server, a path and a branch.
_Avoid_: Env (in prose), stage, server

**Recipe**:
The ordered list of commands an environment runs on the server after the code moves.
_Avoid_: Script, pipeline, hooks

**Step**:
One command from the recipe, run once on the server. Step 0 is the fast-forward of the code itself.
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
