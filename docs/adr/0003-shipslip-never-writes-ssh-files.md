# Shipslip never writes ssh files

Setting up a project in the app reads `~/.ssh/config` to list its `Host` aliases (wildcard patterns left out) and never writes it. When no alias fits, the app shows a `Host` block built from the user's answers for them to paste themselves, then checks again with `ssh -G <alias>`, so ssh resolves `Include` and `Match` itself and Shipslip never re-implements its rules. A host whose key is not in `known_hosts` is not accepted in the app: the app gives `ssh <alias> true` to run in Terminal, where ssh shows the fingerprint, asks, and writes `known_hosts` itself. A host whose key changed stays a hard stop with no command offered. The ssh config and `known_hosts` belong to the user and every ssh tool on the Mac reads them, so a bad write from Shipslip would break ssh for more than Shipslip.

## Considered Options

- **Append the `Host` block to `~/.ssh/config` after showing it, with a backup.** Rejected: ssh takes the first value it finds for each setting, so where the block lands next to `Include`, `Match` and `Host *` changes what it means. Getting that wrong is silent.
- **A Shipslip-owned config pulled in with `Include`.** Rejected for the same ordering reason, and it still asks the user to edit their config once.
- **Fetch the key with `ssh-keyscan`, show the fingerprint, and add it on a click.** Rejected: the key arrives over the same unverified network, so the click looks like a check without being one. It is an accept-all path with an extra step.
