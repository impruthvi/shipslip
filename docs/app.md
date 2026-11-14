# Desktop app

The macOS app deploys over your own SSH connection and keeps receipts on this
Mac. Its grid shows apps and environments with the last code their receipts
record; select a cell to read its receipt beside the grid. Like `slip status`,
the grid shows what this Mac recorded, not what a server is running. Reading
receipts and comparing two environments stay local; comparisons run `git` in
your checkout.

## Deploy from the app

Select an environment to review and approve its configuration, then prepare a
deploy. Preparation connects to the server, fetches the branch, checks the
checkout and takes the deploy lock. The preview shows the server and path,
FROM and TARGET commits, commits to ship, recipe steps, maintenance mode and
log watch settings. Confirm that preview to run; production requires typing the
environment name. Configuration approval is separate from deploy confirmation.
While a run proceeds, the app shows step output and times and watches the
Laravel log when configured, then opens the saved receipt. You can stop after
the current step, stop the log watch, or return to the grid and leave the run
going in the app.

Deploys started with `slip` in a terminal on this Mac also show as Running in
the grid and under Deploys running. Follow them in that terminal: the app
disables Deploy and offers no recovery while the terminal owns the journal.
When the process exits, the app shows the finished receipt or the usual
unfinished-run recovery choices.

## Recovery

Recovery choices come from the saved receipt and each opens a review before
confirmation. Re-attach follows an unfinished run without starting its active
step twice. You can also rerun the recipe, run from a chosen step on the code
already checked out, or bring the app out of maintenance with `php artisan up`.
An unknown outcome stays unknown and asks you to check the server before
running anything. If another run holds the deploy lock, the app blocks the
deploy and offers a `slip break-lock` command to copy into Terminal; inspect
the old run on the server before clearing a stale lock.

## Build it

There is no download yet. To build it you need macOS with the Xcode Command
Line Tools, Rust 1.90 or newer, and Node 26:

```sh
cd app
npm ci
npm run tauri dev                    # run it
npm run tauri build -- --bundles app # build src-tauri/target/release/bundle/macos/Shipslip.app
npm test                             # frontend tests
```

## Keys

The grid refreshes when the window gains focus; ⌘R refreshes now. Arrow keys
move through the grid, Enter opens the receipt, and Esc returns focus to the
grid from the receipt. When the window is too narrow for the receipt beside
the grid, it opens as a drawer, toggled with ⌘I.
