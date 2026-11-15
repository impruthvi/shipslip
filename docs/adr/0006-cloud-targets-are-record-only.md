# Cloud targets are record-only

Shipslip can point an environment at a Laravel Cloud environment (a Cloud target). For a Cloud target it only pulls: it reads the deployments Laravel Cloud ran and writes a receipt for each. It never starts a deployment, runs a command, or changes anything on Laravel Cloud.

The reason is the confirmation rule. A deploy cannot run without a Confirmation built from the exact Preview, including the TARGET commit. Laravel Cloud's deploy endpoint (`POST /environments/{environment}/deployments`) takes no body, so it deploys whatever the branch head is when Cloud picks the request up. A person could confirm one commit and Cloud could deploy another. Shipslip could only notice afterwards, from the commit Cloud reports.

What follows from record-only:

- **Token.** A read-only, resource-scoped Cloud API token is enough. Shipslip takes it from a hidden prompt or `LARAVEL_CLOUD_TOKEN` for each pull and never writes it anywhere, so "no secrets stored by Shipslip" holds unchanged.
- **Host.** The API host is fixed at `cloud.laravel.com`. Config names only the Cloud environment ID, so a changed `.shipslip.toml` cannot send the token somewhere else. Changing the ID needs approval again.
- **Receipts.** A Cloud deployment's receipt is what Laravel Cloud reported, so all of it is server text. A status Shipslip does not recognise, or a deployment still in progress when Shipslip stops looking, is outcome unknown.
- **Step times.** Left as not known (ADR 0001: step times are what this Mac saw). Cloud's own times are kept as Cloud-reported text.
- **Retries.** Pull requests are reads and may be retried. The rule against auto-retrying a remote mutation is not touched, because a Cloud target has no mutation.

## Considered Options

- **Drive deploys through the deploy endpoint and check the commit afterwards.** Rejected: the confirmation rule becomes detect, not prevent, for one kind of target.
- **Drive deploys through the deploy hook URL, which accepts `commit_hash`.** Rejected for now: the hook URL is a second secret per environment, and nothing yet shows Cloud users want Shipslip to deploy for them. This is the route to revisit if they do.
- **Store the token in the macOS Keychain.** Rejected for now: it changes a guarantee the README makes, for a feature with no users yet. Revisit if entering a token per pull proves to be the thing that stops people.
- **Use the Cloud CLI's saved login (`~/.config/cloud/config.json`).** Rejected: `cloud auth` saves a full-access token, and record-only needs read access to one environment.
