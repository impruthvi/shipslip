# Read logs

`slip logs` reads an environment's Laravel log over ssh and groups the errors.
It is read-only: it runs no PHP or recipe steps and changes nothing on the
server.

```sh
slip logs staging                    # grouped errors since the latest run
slip logs staging --since 7d          # override the default window
slip logs staging --channels          # files, formats and coverage per channel
slip logs staging qmkte               # latest full entry for a group ID
slip logs staging qmkte --full        # every stack frame, nothing folded
slip logs staging qmkte --md          # the same detail as Markdown to paste
slip logs staging --level warning --grep payment
slip logs staging --raw --since 30m   # entries in timestamp order
slip logs staging --all --max-bytes 8m
```

`logs` discovers `storage/logs/*.log` and reads the configured `log` /
`log_daily` files over SSH. A final `-YYYY-MM-DD.log` suffix identifies a daily
channel; other `.log` files are single channels. Single and daily files with
the same name merge into one channel. It needs no Laravel package, runs no PHP or recipe steps, and leaves app files, deploy
locks, receipts and signature history unchanged. The summary groups entries
by exception class and app file, including recurring errors. Copy a group's
ID to open its latest entry and stack trace; row numbers also work, but can
move between invocations. The first 20 groups are shown unless you use `--all`.

The summary header counts every entry in the window by level, for example
`in this window: 3 error · 12 warning · 40 info`, whatever `--level` and
`--grep` hide. In a group's detail, runs of three or more framework frames
(files under a `vendor` directory) fold into one line such as
`… 14 vendor frames (--full to show)`; the first frame, your app's frames and
lines slip can't read as frames always stay. `--full` shows every frame.

## Same-request entries

When the latest entry carries a request or job ID at the top level of its
Laravel context, the detail view lists other entries in the window with the
same ID, up to ten before and ten after, across all channels. Keys are tried
in this order: `request_id`, `trace_id`, `correlation_id`, `job_id`,
`batch_id`. This is correlation, not cause: the lines came from the same
request (or, for `trace_id`, the jobs it queued), not necessarily one from
another. Laravel adds no ID by default; a middleware that calls
`Context::add('request_id', (string) Str::uuid())` puts one on every line. When
more lines match, slip prints a `--raw --grep` command for the same window;
being a text search, it can also show the ID mentioned elsewhere.

## Markdown

`--md` prints a group's detail as Markdown for an issue or chat. It starts with
the environment, log timezone, window (start to the time it was read),
coverage, baseline, filters and slip version, so a paste still makes sense
later. Log text is escaped or fenced, so links, images and HTML in it show as
plain text. `--md` folds frames like the terminal unless you add `--full`.

## Window and anchor

The default window starts at the latest verified Shipslip run (deploy, rerun
or run from step). During a run, the header says `deploy in progress`. If the run
marker is missing or stale, the command uses the latest Git checkout change,
then falls back to 24 hours when no reflog is available. The header always
names the anchor; corrupt markers and Git failures are reported. `--since`
selects your own window instead.

Deploys atomically record `<git dir>/shipslip.last-run` at run start and update
it before releasing the owned lock. Re-attaching updates the same run.
Marker failures warn in output and receipts and never fail a deploy. The
reader uses inode, size and checksum checks to validate recorded byte
positions; rotation or copytruncate falls back to timestamps with a note.
A later checkout invalidates the run anchor, even if HEAD returns to the same
commit. An abandoned run or failed end update can show `outcome unknown`.

## New and seen

`NEW` means the group was not found in the bounded logs immediately before
that window. Comparison spans all channels, so an error moving between
channels stays known. `?` means there is no usable comparison for that group's
channels. New groups appear first; details also label new and seen message
variants. The summary aligns IDs, counts and status (`NEW`, `seen` or `?`),
with the application file, channels and message on separate lines. Baseline
coverage and channels without a baseline are disclosed;
`NEW` is limited to that readable span, rather than the app's entire history.
A partly recognized baseline can confirm known errors, but gives `?` for
otherwise unseen groups because its unparsed output is not a usable comparison.

## Filters and time zone

`--level` includes that level and anything more severe; it defaults to `error`
for groups and `debug` for `--raw`. `--grep` matches text without regard to
case. `--since` accepts `30m`, `6h`, `7d`, `YYYY-MM-DD` or
`"YYYY-MM-DD HH:MM"`, up to 30 days back. Set `timezone = "Asia/Kolkata"`
in the environment table if Laravel writes times in that zone; the default
is UTC. Clock times are interpreted in that zone. Raw output warns when a
clock change makes ordering across files approximate.

## Read limits and coverage

The default read limits are 4 MiB of uncompressed log per channel and 12 MiB
total. Channels share the total fairly, with unused shares redistributed;
quiet channels get their whole file when it fits. `--max-bytes` replaces the
total and per-channel window limits (`k`, `m` and `g` use binary units).
The comparison has a separate 2 MiB per-channel / 6 MiB total limit, shared
fairly. It walks backward from the anchor through up to seven earlier daily
files; `--channels` includes its coverage. At most
20 channels and 50 files are selected per snapshot; omissions are reported.
Partial coverage is printed in the header, and when any channel was partly
read or contains lines slip couldn't parse, counts are lower bounds: the
header line says `at least`, and group and variant counts use `≥`. Use
`--channels` to inspect each channel's files, total size, last write, format and
coverage. Files that rotate or are truncated during the snapshot are reported as changed. Individual entries
are limited to 256 KiB, with a note in the detail view when truncated.

## Channels and formats

Files resolving under `storage/logs`, including shared storage symlinks, need
no config approval. Reading another configured path requires approval with
`slip trust ENV`. To hide, rename or add channels, see
[log channels](config.md#log-channels).

Laravel headers and stack-trace lines identify recognized formats. Small
files need only one valid header. Mixed output keeps valid Laravel entries
in groups and reports the percentage recognized. JSON and other unsupported
formats are labeled and appended as file blocks by `--raw`. These blocks
retain file order and bypass `--level` and timestamp filtering; `--grep`
filters their lines. All output escapes terminal control characters. If no
file was written in the window, a hint explains that stderr, syslog and
service channels are invisible to this file reader.

The server needs Linux with `/proc`, bash, GNU coreutils, gzip and tzdata.
Log output can contain secrets; treat it as private.

## JSON

`slip logs ENV --json` returns every tracked error group as schema-1 JSON.
`slip logs ENV ID --json` returns full entry evidence, best-effort frames,
and same-request entries (correlation, not cause). IDs may be a row number,
a short ID, or a full ID from an earlier snapshot. JSON includes typed window
anchors, baseline badges, lower-bound counts, channel coverage, and warnings.
`--json` rejects `--raw`, `--channels`, and `--md`; `--full` has no effect.
See the [JSON reference](json-reference.md) for the schema. An agent can read
the same records through the [MCP server](mcp.md).
