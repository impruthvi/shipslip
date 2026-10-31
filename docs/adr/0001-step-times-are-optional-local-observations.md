# Step times are optional local observations, added without a receipt version bump

Each step (and maintenance down/up) records `started_at_ms` and `finished_at_ms` as optional fields on the existing receipt versions (deploy 1, Up 2), not under a new version. Older releases ignore the fields and keep reading new receipts; receipts without them read as "no step times recorded". Both times come from this Mac's clock: the start is stamped in the journal write that happens before the step is sent, and the end when Shipslip sees the result. When the end was not seen in the same run (outcome unknown, or the result read by a later attach) the end stays empty instead of being guessed.

## Considered Options

- **New receipt version.** Rejected: older releases would call every new receipt unreadable, for a field they can safely skip.
- **Store `duration_ms` only.** Rejected: it says nothing for a step whose end is not known, and a start time is still useful then.
- **Server clock (for example the exit file's mtime).** Rejected: it mixes two clocks in one receipt and reads remote output as fact. Receipts say what this Mac recorded.
- **Stamp the end when a later attach sees it.** Rejected: a two-minute migration seen three hours later would read as taking three hours.
