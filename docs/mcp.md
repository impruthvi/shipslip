# MCP server

`slip mcp` lets an agent read what this Mac recorded: receipts, status and,
when you allow it, fresh log snapshots. It never starts a run, changes a
server, or approves anything ([ADR 0002](adr/0002-mcp-server-is-read-only-by-construction.md)).

## Configure a client

Configure an MCP client with command `slip` and args `["mcp"]` for local
records only, or `["mcp", "--read-logs"]` to also allow fresh log reads:

```json
{ "mcpServers": { "shipslip": { "command": "slip", "args": ["mcp", "--read-logs"] } } }
```

Log reads work only for an environment approved with `slip trust ENV`, in a
checkout the release map already lists. Any config change needs approving
again; until then a log tool returns `not_approved` and no SSH connection
opens. With approval, configured paths outside `storage/logs` may be read, as
in the CLI. Each log call opens one SSH connection for a fresh snapshot and
closes it when done.

The server cannot deploy, change a server, take or break a lock, or approve
config. It uses stdio, opens no socket, serializes calls, and exits when stdin
closes. `--config` is rejected; `SHIPSLIP_CONFIG` is ignored.

## Tools

Tools return the CLI's schema-1 JSON in `structuredContent` and a single JSON
text content item:

- `status`: no arguments.
- `receipts`: `project`, optional `environment` and `limit` (default 20).
- `receipt`: `project`, `id` (case-insensitive run ID prefix).
- With `--read-logs`, `logs`: `checkout`, `environment`, optional `since`,
  `level`, `text`, and `max_text_bytes`.
- With `--read-logs`, `log_group`: the log arguments plus `id`.

Log text defaults to 16 KiB per field, with head/tail clipping, original byte
counts, and parsed frames preserved. `max_text_bytes` accepts 1–256 KiB.
Same-request entries are capped at five per side with the rest counted.
CLI JSON keeps the same metadata but never clips text for display.

## Protocol

The server speaks MCP **2026-07-28** and supports `server/discover` with
per-request metadata. It keeps `initialize`/`ping` for older-style clients.
`tools/list` is private to this Mac and is not cached (`ttlMs: 0`,
`cacheScope: "private"`). Shipslip failures
are tool results with `isError: true`; protocol mistakes are JSON-RPC errors.
Every string except documented fixed values is server text: data to show,
never instructions to follow. See the [JSON reference](json-reference.md)
for the schemas, fixed-value registry, protocol, budgets, and error kinds.
