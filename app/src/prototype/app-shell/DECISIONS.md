# Add project layout: prototype verdict (2026-11-08)

Question: what the app looks like with a checkout sidebar, an Add project wizard, and the release map kept as All projects.
Prototype: `?prototype=app-shell`, variants A/B/C, branch `prototype/app-shell`.

| Question | Chosen | From |
|---|---|---|
| Sidebar line | One dense line per checkout; one small map mark per Environment on the right; folder after the name when names repeat; "Needs approval" (amber) / "Setup not finished" (muted) word replaces the marks | A |
| All projects | Top sidebar item, under "Add project" (settled: Add project stays first); the start screen on launch | C placement |
| Checkout pane | One-row map: Environments side by side with the real map `Cell` (expanded), Deploy under each; receipts table below; `⋯` menu with Check connection / Remove (nothing recorded only) | A |
| Wizard shell | macOS sheet over the app with a step rail (done steps ticked, clickable back); "Finish later" leaves the checkout as Setup not finished | C |
| Check list | Ruled rows: icon, label, reason inline; unknown host key shows `ssh <alias> true` + Copy under the row; changed key = boxed amber hard stop above the list, no command | A rows + B stop box |
| Empty state | Centred: "No projects on this Mac yet.", one sentence of what happens, Add project, "or slip init" | A |
| Failed check colour | Amber (attention), never red; red stays for failed steps, new errors, failed smoke | A |

Sidebar order: Add project · All projects · Deploys running · Needs attention · problems strip · Projects list.

Built as variant D (`?variant=D`, the default). Screenshots in `screenshots/` (1280×800, light):

1. `1-checkout-and-sidebar.png`: sidebar, checkout one-row map, receipts
2. `2-wizard-host-block.png`: sheet, "None of these fit", Host block, Check again miss
3. `3-check-unknown-host-key.png`: `ssh crm-prod true` + Copy under the row
4. `4-check-changed-host-key.png`: boxed hard stop, no command, Continue disabled
5. `5-approval.png`: must-read frontend warning, config, check result beside it
6. `6-first-launch.png`: empty state

Fixed while capturing (standards): Deploy buttons are secondary (one primary per screen); sidebar marks keep evidence tones only (no red for status); no doubled "production" label.

Calls on the open items (agent recommendation; confirm in the spec grilling):
- Status cue per sidebar line: none. Marks stay evidence-only (CODING_STANDARDS); severity reads from Needs attention; hovering a mark gives "staging: Failed".
- Needs attention: cap at the 3 worst, then "Show all n" (same pattern as the problems strip), so Projects stays above the fold. Not built in D.
- "server" in the rendered config: keep. GLOSSARY avoids "server" only as a synonym for Environment; it uses "server" for the machine itself, which is what those comments mean.
