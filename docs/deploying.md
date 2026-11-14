# Deploying

Every run starts with a preview: the environment, server and path, FROM and
TARGET commits, the commits to ship, the recipe steps, maintenance mode, log
watch and smoke URL. Nothing changes on the server until you confirm that
exact preview. Production asks you to type the environment name.

Before the preview, Shipslip takes the environment's deploy lock, so only one
run uses an environment at a time. It checks it still owns the lock before
every step, and only releases a lock it owns.

## Kinds of run

```sh
slip deploy staging          # fast-forward, then run every recipe step
slip rerun staging           # rerun every recipe step on the checked-out commit
slip from-step staging 2     # run recipe steps 2 through the end
```

`deploy` is the only kind of run that changes the server checkout. A rerun or
a run from step requires the server to already be on the clean target commit.
Recipe steps are numbered starting at 1; the built-in Git fast-forward is
step 0 and is not selectable. Use `slip rerun` only when repeating every
recipe step is safe.

Push the commit you want to ship to the branch named in `.shipslip.toml`
first: the server fetches that branch.

## After the steps: log watch and smoke check

Leave the terminal open for the deploy and the 120-second log watch; Shipslip
prints the time left every 30 seconds, and Ctrl-C ends the watch early. Check
the final deploy outcome, the `Log watch` line, and (if configured) the smoke
check's HTTP result. `complete` means Shipslip observed the log; `no log file
found`, `log could not be read`, or `partial` means you should inspect the log
path, permissions, or format.

Shipslip compares log entries against the last 2 MiB of the log and previously
observed error signatures. It watches during the deploy and for 120 seconds
after the steps finish. The receipt groups new errors by exception class and
app file; the displayed line number does not affect the signature. Numbers,
UUIDs, and long or spaced quoted values are normalized, so some distinct
errors may be grouped together. Short quoted identifiers remain distinct, so
request-specific short values may appear as separate variants. "New" means
first observed after this deploy, not caused by it.

When a log cannot be observed, the receipt records `Partial`, `Unavailable`, or
`NoLogSeen` rather than reporting zero new errors. The optional smoke check
follows at most three redirects, verifies TLS, and times out after 10 seconds.
Its HTTP status and latency are saved separately from the deploy outcome. A
failed smoke check does not undo a deploy.

## Use a local GitHub token

If the server has no GitHub credentials, use a token from your laptop for the
preflight fetch:

```sh
slip deploy staging --github-token
```

Shipslip first inspects the server's repository, then asks for a token with
hidden input. It checks the token's GitHub account and repository visibility,
shows the username and `OWNER/REPO`, and lets you continue, enter another
token, or cancel. The server's fetch verifies actual read access; the normal
deploy preview and production confirmation still follow. Your SSH identity
and personal GitHub CLI login do not select the account for this token.

Need a token? The prompt includes a GitHub creation link with the repository
owner and read permissions prefilled. Select the target repository and
**Contents: Read-only**, choose an expiry, and obtain any required organization
approval. Classic tokens may need SSO authorization. Follow your organization's
token policy; a personal account can be used if it has the required access.

Other credential sources must be selected explicitly:

```sh
slip deploy staging --github-token-source env  # local GH_TOKEN, then GITHUB_TOKEN
slip deploy staging --github-token-source gh   # active github.com GitHub CLI login
slip rerun staging --github-token
slip from-step staging 2 --github-token
```

`--github-token` always prompts, even if an environment token or GitHub CLI
login exists. Token flags accept a source name, never a token value. Hidden
entry requires an interactive terminal. Scripts must choose `env` or `gh`
explicitly and still answer the account and deploy confirmation prompts.

Temporary authentication supports standard `github.com` HTTPS and SSH origins.
It fetches over HTTPS without changing the saved `origin`; embedded credentials,
other hosts, Git URL rewrites and redirects are refused. The repository is
pinned to the one shown at account confirmation. The approved server receives
the token briefly in process memory over SSH. Shipslip does not save it on
either machine or include it in receipts, detached step scripts or output;
existing server credential helpers cannot save it. Existing `gh` credentials
stay managed by GitHub CLI. The token is used only for this fetch, not for
Composer, submodules or recipe steps; configure private Composer dependencies
and submodules separately. Local `curl` is required for account validation.
Without a token flag, server authentication works as before.

## Ctrl-C

Ctrl-C at the confirmation prompt cancels the run and releases the deploy
lock. Answering no, or mistyping a production environment name, releases the
lock too and exits 1: a command exits 0 only when it did what it was asked.
During the steps, Ctrl-C starts no new step and stops following the
running command. During the post-deploy log watch, it ends the watch early and
still finishes the run. While maintenance mode is being turned off, Shipslip
finishes that first. Press Ctrl-C again to quit at once.

## Recovery

Shipslip never retries a command that changes the server. When it cannot tell
how a step ended, the outcome is **unknown**, never a guess.

### Re-attach to an unfinished run

If `slip` exits while a command is running, that command continues on the
server. Re-attach to the unfinished run from the same project with:

```sh
slip attach staging
```

`attach` checks the saved config and asks for confirmation. It observes the
active remote command without launching it again, then continues the remaining
steps if the run still owns the deploy lock. It requires the same local receipt
and project checkout. Only one process can use a receipt at a time. If the
server cannot be reached, the receipt stays unfinished so you can try again.

### Clear a stale lock and bring the app up

If the result is unknown and the deploy lock becomes stale, first inspect the
server over SSH to confirm no command from the old run is still running. The
receipt's run ID identifies its directory under `~/.shipslip/runs/`. Then
clear the stale lock and, if maintenance mode may still be on, bring the app up:

```sh
slip break-lock staging
slip up staging
```

`break-lock` shows who holds the lock and its run ID, and only clears a lock
whose heartbeat is at least two minutes old; it asks you to type the
environment name. `up` takes a new deploy lock and runs
`php artisan up`; it asks for confirmation. `up` saves its own receipt
before sending the command, so if it is interrupted, `slip attach staging`
follows it again without sending it twice. Neither command continues the old
deploy. Check the receipt and server state before starting a new run.

### A step failed

`slip receipts show ID` says which step failed and what to check next. If the
cause was on the server, fix it there and run `slip from-step staging N`. If
it needs a code change, push the fix and run `slip deploy staging`.
