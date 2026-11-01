# The MCP server is read-only by construction

`slip mcp` lets an agent ask what is deployed where and what broke after the last run. It must never start a run, change a server, take or break a deploy lock, or approve a config. Here is where each part of that guarantee sits.

- **API boundary (types).** The server is written against one facade, `shipslip::read_only::Reader`. It offers only `status`, `receipts`, `receipt`, `logs` and `log_group`. No method takes a script, a `Confirmation`, a lock or a trust write. The log methods need an approved-target value. That value can only be built when the environment's current config matches its approval record and its checkout is one the release map already lists.
- **Import boundary (test).** A unit test reads the MCP source files and fails on any `shipslip::` or `crate::` path other than `read_only`. It is an allow-list, so a mutating function added to the library later is blocked without anyone remembering to list it.
- **Runtime (real sshd).** The integration suite records the remote app tree, its `.git` directory (deploy lock, last-run marker) and the log files' inode, size and mtime. It also records the local receipts and trust store. It then calls every MCP tool and checks that nothing changed.

Reading a server is opt-in. By default the server answers from what this Mac recorded. It opens ssh only when started with `--read-logs`, and only for approved environments. Without approval it returns `not_approved` and never connects. The server speaks MCP over stdio only. It opens no socket and keeps no daemon.

Not guaranteed: what the server does on login (shell startup files, a forced command). This is the same for every `slip` read.

## Considered Options

- **Convention and review.** Rejected: nothing would catch a new tool that calls `execute()`.
- **A crate split, or the mutating API behind a default-off Cargo feature.** Rejected: workspace feature unification turns the feature back on when the MCP is built next to the CLI. A crate split also means a large reshuffle of the library for a guarantee the allow-list test already gives.
- **Clippy `disallowed_methods`.** Rejected: it is a deny-list, so new mutating functions are allowed until someone adds them.
- **Same log rule as the CLI (`storage/logs` needs no approval).** Rejected for the MCP: an agent that can edit `.shipslip.toml` could aim the read at a new host. Approval is where a person already said the alias and path are theirs.
