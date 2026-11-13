# Host-key refusals fail closed

Shipslip offers a command for a host-key refusal in one case only: ssh said `No … host key is known for … and you have requested strict checking`. Then `ssh <alias> true` in Terminal shows the fingerprint and asks, as in ADR 0003. Any other refusal that fails host-key verification is a hard stop with no command: a changed key, a revoked key, or a bare `Host key verification failed.` with no reason. ssh prints only the bare line when `LogLevel` in the user's config is `FATAL`. With `QUIET` it prints nothing, so the refusal is an unclassified connect failure, which also offers no command. Reading a bare line as "unknown" would offer a command that accepts a key that may have changed or been revoked.

## Considered Options

- **Pin `LogLevel` with `-o` so ssh always says why.** Not possible: the `openssh` crate's `SessionBuilder` has no way to pass `-o`.
- **A `-F` wrapper config that sets `LogLevel` and `Include`s the user's config.** Rejected: ssh takes the first value it finds, so the wrapper's `LogLevel` would win only if it came first, and then it overrides a setting the user chose for every host. Putting it after the `Include` lets the user's value win, which fixes nothing.
- **Patch `openssh` to accept `-o`.** Rejected: heavier to carry, and it fixes one cause of a reasonless refusal, not all of them. Failing closed covers every cause.
