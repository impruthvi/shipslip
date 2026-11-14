# Receipts and status

Each run saves a local receipt, written to disk before each remote step is
sent. Reading receipts and status never contacts a server.

## Where receipts live

On macOS, receipts live under `~/Library/Application Support/Shipslip/receipts/`;
on Linux, under `~/.local/share/shipslip/receipts/`, in a directory for the
project and environment. The receipt records the approved preview, exact
commits, step results, recent step output, log watch, smoke result, and
outcome. New receipts also record who started the run: the local user and the
checkout's Git name and email. Treat receipts as private: command output can
contain secrets.

## List and read receipts

```sh
slip receipts                 # newest 20 for this project; --all lists every one
slip receipts staging         # one environment
slip receipts --json          # a versioned JSON list; also accepts ENV and --all
slip receipts show 18dab54b   # one run, by the ID shown in the list
slip receipts show 18dab54b --md
slip receipts show 18dab54b --json
```

Each row shows the outcome with flags for anything that needs a look:
`⚠ N new error groups`, `⚠ smoke 500`, `ⓘ log not fully observed`,
`ⓘ log not recorded`, `ⓘ smoke not recorded`, and `⛔ app may be in maintenance mode`.
A run that never finished reads `Unfinished (last recorded: …)`, never as its
saved outcome. `show` prints the steps (including maintenance on/off) with exit
codes and saved output, the log watch and smoke results, and what to check
next. It only states what the receipt recorded. After a failed step, or one
whose result is unknown, Shipslip reads the server's commit and whether the
checkout has uncommitted changes (read-only, best effort); if that read fails,
`show` says `not recorded`.

An outcome Shipslip could not determine, such as a step whose result was lost
with the connection, is recorded as **unknown**. It is never turned into
success or failure by guess.

## Share a receipt

`--md` prints Markdown with statuses, exit codes, commits, commands, and error
classes and counts, but no server output, log messages, warnings, or reasons,
and names who started the run without their email address.
`--md --with-details` adds them; review that output for secrets before sharing
it. Server text is escaped in both the terminal and Markdown.

## JSON

`receipts --json` prints `{"schema":1,"rows":[...],"older":0}`. Rows carry a
`state` of `receipt`, `newer`, or `unreadable`, so a damaged file does not hide
the rest of the list. Readable rows include the short unique ID, local path,
run kind, typed outcome and flags, and FROM/TARGET commits. `older` counts
receipts beyond the default limit of 20; `--all` removes that limit.

`show --json` prints the full record with `"schema": 1`. Times are absolute
milliseconds; an unobserved step end and its duration are `null`. Receipt text
and server output remain raw JSON strings. Lookup errors print
`{"schema":1,"error":{"kind":...}}`: `missing`, `ambiguous` (with `ids`), and
`newer_version` (with `version`) exit 2; `unreadable` (with `message`) exits 1,
the same as text mode. Within schema 1, fields are only added. See the
[JSON reference](json-reference.md).

## What is on each server

`slip status` reads every project's receipts and approvals and prints, for each
checkout and environment, the last code they record. It works from any folder,
needs no config, and never contacts a server.

```sh
slip status
slip status --json                         # versioned local status record
slip status --compare staging production   # commits between the two, per app
```

A commit is shown only when a receipt backs it: `code at …` when the
fast-forward succeeded (`deploy incomplete` if a later step did not), `checked
out at start` for reruns, and `observed …` when Shipslip read the server's
commit after a failed step. Otherwise the line says `server code not known`,
`no deploy receipt on this Mac`, or `target changed` when the environment now points at a
different server or path. Each line also shows the last run and its flags,
and what to do next when that run needs attention. It warns when a newer
receipt could not be read, and when another checkout ran a newer deploy on the
same server path, since that run may have replaced the code. It also warns
when the approved config now deploys to a different server or path than the
newest receipt, until a run uses the new one. `--compare` uses your local checkout to count commits and notes an
incomplete deploy or dirty tree on either side.

`--json` prints the same local facts with `"schema": 1`, structured evidence
marks and outcomes, and absolute epoch-millisecond timestamps rather than
relative wording. Errors are JSON too and keep the text command's exit code.
`--json` cannot be combined with `--compare`, which stays a separate local-git
comparison.
