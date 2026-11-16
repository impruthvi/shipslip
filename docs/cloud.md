# Laravel Cloud targets

A Cloud target is an environment that points at one Laravel Cloud environment
instead of a server ([configuration](config.md#cloud-targets)). Shipslip only
reads it: it pulls the deployments Laravel Cloud ran into receipts and never starts,
cancels or changes one ([ADR 0006](adr/0006-cloud-targets-are-record-only.md)).

## Pull deployments

```sh
slip trust production   # approve the Cloud target once
slip pull production    # asks for the Cloud API token, then pulls deployments
slip receipts           # Cloud receipts are listed next to server runs
```

`slip pull ENV` refuses a server target and a Cloud target that is not
approved. It then asks for a Laravel Cloud API token at a hidden prompt, reads
the ten most recent deployments of the Cloud environment and writes one
receipt for each that succeeded. A deployment already held is not written
again. For now, deployments that are still running or did not succeed are
listed as skipped and get no receipt.

Each receipt is written whole (temp file, fsync, rename) before the next is
read, so stopping a pull with Ctrl-C keeps every receipt already written.

## The token

Create a token in the Laravel Cloud dashboard with read access
(`environment.view`) to this one environment. Shipslip never stores it: it is
held in memory for one pull and passed to `curl` through its config on stdin,
so it does not appear in the process list, in shell history or in any
receipt. Requests go only to `cloud.laravel.com`; no setting changes the host.

## Cloud receipts

A Cloud receipt holds the Cloud deployment ID, the Cloud environment ID, the
status Laravel Cloud reported, the branch, commit, commit message and author,
Laravel Cloud's start and finish times, and when this Mac pulled it. Laravel
Cloud's times are kept as Cloud-reported values, apart from this Mac's own
observations. `slip receipts` shows the start with `(Cloud)` and marks each
row `Laravel Cloud ran it, not Shipslip`. Everything in a Cloud receipt came
from Laravel Cloud and is treated as server text.

`slip receipts show` does not show a Cloud receipt yet.
