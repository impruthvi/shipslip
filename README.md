# Shipslip

Shipslip runs a configured deploy recipe over SSH. It prepares and displays a
preview before asking for confirmation. Production runs require typing the
environment name.

## Quick start

Create `shipslip.json` in the current directory:

```json
{
  "environments": {
    "staging": {
      "production": false,
      "ssh_alias": "my-app-staging",
      "path": "/srv/my-app",
      "branch": "main",
      "steps": [
        "composer install --no-interaction --no-dev --prefer-dist",
        "php artisan migrate --force",
        "php artisan config:cache"
      ],
      "maintenance": true
    },
    "production": {
      "production": true,
      "ssh_alias": "my-app-production",
      "path": "/srv/my-app",
      "branch": "main",
      "steps": [
        "composer install --no-interaction --no-dev --prefer-dist",
        "php artisan migrate --force",
        "php artisan config:cache"
      ],
      "maintenance": true
    }
  }
}
```

The `ssh_alias` must be configured in OpenSSH, usually in `~/.ssh/config`.
`steps` contains shell commands run in order from the configured project path.
When `maintenance` is true, Shipslip runs `php artisan down` before the recipe
and `php artisan up` after it succeeds.

Run a deploy from the repository root, or build and invoke the binary directly:

```sh
cargo run --bin slip -- deploy staging
# Or select a config file:
cargo run --bin slip -- --config ./ops/shipslip.json deploy staging
```

## Run plans

```sh
slip deploy staging          # fast-forward, then run every recipe step
slip rerun staging           # rerun every recipe step on the checked-out commit
slip from-step staging 2     # run recipe steps 2 through the end
```

`deploy` is the only plan that changes the server checkout. Reruns require the
server to already be on the clean target commit. Recipe steps are numbered
starting at 1; the built-in Git fast-forward is step 0 and is not selectable.

The config path defaults to `./shipslip.json`. Set `SHIPSLIP_CONFIG` to use a
different default path. Every environment must explicitly set `production`.

## Safety behavior

Shipslip runs read-only preflight, obtains the deploy lock, and prints the
environment, plan, branch, commit range, and target SHA before prompting. If
you decline, it releases the lock without running recipe steps. Production
requires typing the exact environment name. Reruns are refused unless the
remote checkout is clean and matches the fetched target branch.
