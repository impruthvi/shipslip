# Shipslip roadmap: from CLI to desktop control panel

Status: proposal, October 2026. Shipslip is open source, so this plan
optimizes for usefulness, safety and low maintenance, not revenue.

## Goal

Let Laravel developers deploy and operate their apps from the desktop app
instead of the command line:

- deploy, rerun and read logs from the UI;
- manage queue workers, Supervisor and the scheduler;
- deploy to AWS EC2 without opening port 22;
- see and deploy Laravel Cloud environments alongside SSH servers.

## Where Shipslip is today (0.7.1)

- `slip` CLI (Rust): creates verified Laravel projects, publishes to GitHub,
  deploys an existing server checkout over SSH (git fast-forward + recipe
  steps), with preview, confirmation, typed production confirmation, deploy
  lock, config trust, receipts, grouped log reading (`slip logs`) and a
  release map (`slip status`).
- Desktop app (`app/`, Tauri, macOS): **read-only**. Shows the release map
  and receipts; never contacts a server.
- Engine is already two-phase: `engine::prepare*` (read-only, returns a
  `Preview`) then `engine::execute*` (streams `DeployEvent`s). This maps
  directly onto a UI preview → confirm → live output screen.
- `transport::Transport` means "run a bash script on a host". SSH (and
  SSH tunnelled through AWS) fits behind it. Laravel Cloud does **not**: it
  is an API where you trigger a deploy and poll its status.

## Why build it

- No desktop app found that deploys Laravel straight over SSH without a
  hosted panel. F-Bar, the closest, depends on Forge's API.
- Local-first: SSH keys and `.env` stay on the user's machine; receipts give
  an audit trail no hosted panel offers; open source makes a tool with
  production access easy to trust.
- Queue/Supervisor/scheduler handling is the most common everyday pain:
  workers keep old code after a deploy, `queue:restart` silently does
  nothing with the `array` cache driver, and one bad cron line stops every
  scheduled job.

Main competitor is Laravel Forge (relaunched October 2025 with health checks,
scheduled-job heartbeats, a log view and rollbacks). Shipslip wins on
local-first, receipts, previews and covering several providers in one view,
not on matching Forge feature for feature.

## Principles (keep for every phase)

1. **Preview before every change.** Anything that changes a server or a
   cloud resource shows exactly what it will do and asks first. Production
   still requires typing the environment name.
2. **Allow-listed actions only.** The UI never takes a free-form shell
   command. Every remote command comes from the trusted config or a fixed
   list in code, with arguments escaped.
3. **Config changes need trust.** New config sections (workers, scheduler,
   cloud targets) are part of the trust fingerprint, like recipes.
4. **Receipts for everything.** Every run, restart or retry writes a receipt.
5. **No secrets in output.** Mask `.env` values; never print the contents of
   `storage/framework/down` (it holds the bypass secret); treat failed-job
   payloads and worker logs as private.
6. **One engine, two front-ends.** The CLI and the desktop app call the same
   library functions and show the same wording.
7. **Keep fragile integrations isolated.** Laravel Cloud's API is young and
   changing; keep it in its own module so a break there cannot affect SSH
   deploys.

## Phases

### Phase 0: actions in the desktop app

Without this, everything else stays CLI-only.

Scope:
- Deploy, rerun and from-step from the UI: call `prepare*`, show the
  `Preview`, confirm (typed env name for production), call `execute_recorded`
  and stream `DeployEvent`s into a live output pane. Finish by opening the
  new receipt.
- Cancel before confirmation (`engine::cancel`), lock status and break lock
  (`engine::lock_status`, `engine::break_lock`) with confirmation.
- Logs screen backed by the same code as `slip logs` (grouped errors, NEW
  flags, open an entry by group ID).
- Trust screen: show the config diff and approve, like `slip trust ENV`.
- Introduce a target kind in config and engine: `ssh` (today) and, later,
  `laravel_cloud`. Only `ssh` is implemented in this phase.
- Widen Tauri capabilities only as far as needed and describe each one.

Done when: a staging deploy can be previewed, confirmed, watched and its
receipt opened entirely from the app, and the CLI behaves exactly as before.

### Phase 1: runtime panel for any SSH server

Scope, in order:
1. **Read-only status** (no confirmation needed):
   - `php artisan about --json` (Laravel version, env, debug, cache state,
     drivers, maintenance mode);
   - `supervisorctl status` (parse `name STATE pid, uptime` lines);
   - `php artisan horizon:status` (text; inactive exits 1);
   - `php artisan schedule:list --json`;
   - `php artisan queue:monitor CONN:QUEUE --max=N --json`;
   - failed-job count and list (`queue:failed --json` on Laravel 13; parse
     the table on older versions);
   - `migrate:status` (no JSON; parse or use `--pending`);
   - disk space, PHP version, whether the cron line for `schedule:run`
     exists, whether the `array` cache driver would break `queue:restart`.
2. **Restart after deploy**, recorded in the receipt:
   - Laravel 12.41+ / 13: `php artisan reload` (runs `queue:restart`,
     `schedule:interrupt`, and registered package hooks such as
     `horizon:terminate`, `octane:reload`, `reverb:restart`). The 12.41
     minimum comes from Deployer's recipe; confirm against the framework.
   - Older: `queue:restart`, `horizon:terminate` if Horizon is used,
     `schedule:interrupt`.
3. **Declared workers**: Shipslip writes Supervisor program files from
   `.shipslip.toml`, shows the diff, applies with `sudo -n supervisorctl
   reread` then `update`, and records a receipt. Start/stop/restart buttons
   only for declared programs. Clear error when passwordless sudo is missing.
4. **Failed jobs**: retry one (confirm), retry all (confirm, it repeats side
   effects), flush (typed confirmation).
5. **Scheduler**: install/check the cron line; pause/continue on Laravel 13
   (`schedule:pause`, `schedule:continue`).

Draft config (part of the trust fingerprint):

```toml
[runtime]
supervisor_conf_dir = "/etc/supervisor/conf.d"
use_sudo = true
reload_on_deploy = "auto"   # auto | reload | legacy | off

[[runtime.workers]]
name = "default"
kind = "queue"              # queue | horizon | daemon
connection = "redis"
queues = ["high", "default"]
processes = 4
tries = 3
timeout = 60                # must be below the connection's retry_after
max_time = 3600
stop_wait_secs = 3600       # must be above timeout and the longest job
user = "deploy"
log = "storage/logs/worker.log"

[[runtime.workers]]
name = "reverb"
kind = "daemon"
command = "php artisan reverb:start"

[runtime.scheduler]
mode = "cron"               # cron | supervisor

[runtime.monitor]
queues = ["redis:default"]
max = 100
```

Validate `timeout < retry_after < stop_wait_secs` locally before any SSH call.

Laravel's recommended Supervisor program for reference:

```ini
[program:laravel-worker]
process_name=%(program_name)s_%(process_num)02d
command=php /path/artisan queue:work --sleep=3 --tries=3 --max-time=3600
autostart=true
autorestart=true
stopasgroup=true
killasgroup=true
user=deploy
numprocs=8
redirect_stderr=true
stdout_logfile=/path/worker.log
stopwaitsecs=3600
```

### Phase 2: AWS EC2

Keep the SSH transport and let AWS provide the tunnel, so deploy, lock,
receipts and logs work unchanged.

Scope:
- **Sign-in**: read the user's `~/.aws/config` profiles (including IAM
  Identity Center/SSO). Show the account and role from
  `sts get-caller-identity`. When SSO has expired, show "run
  `aws sso login --profile X`".
- **Server picker**: region → running instances (Name tag, ID, state) →
  "SSM ready" badge from `ssm describe-instance-information`.
- **Tunnel**: write an SSH host block, shown as a diff before saving:
  - SSM: `ProxyCommand sh -c "aws ssm start-session --target %h
    --document-name AWS-StartSSHSession --parameters 'portNumber=%p'"`
    (needs the Session Manager plugin locally, the SSM agent and the
    `AmazonSSMManagedInstanceCore` role on the instance).
  - EC2 Instance Connect Endpoint: `ProxyCommand aws ec2-instance-connect
    open-tunnel --instance-id %h` (needs one endpoint in the VPC).
- **Doctor checks**: `aws` CLI present, Session Manager plugin present,
  instance reachable, instance role attached.
- Required IAM: describe calls and `ssm:StartSession` (or
  `ec2-instance-connect:OpenTunnel`) only. **No changes to AWS resources.**

Implementation note: call the `aws` CLI with `--output json` rather than
adding the AWS Rust SDK. Users need the CLI for the tunnel anyway, and the
SDK's current crates require a newer Rust than Shipslip's 1.88 minimum.

Skip: App Runner (closed to new customers since April 2026), Elastic
Beanstalk, Lambda/Bref and ECS. Each is a different deploy model.

Later in this phase: short-lived keys with `SendSSHPublicKey`, Lightsail
discovery, optional CloudWatch log source.

### Phase 3: Laravel Cloud

Add Cloud environments to the release map and deploy them with receipts.
Do not rebuild Laravel Cloud's own dashboard.

API facts (read from the official open-source `laravel/cloud-cli`, v0.6.x):
- Base URL `https://cloud.laravel.com/api`, `Authorization: Bearer TOKEN`,
  JSON:API (`application/vnd.api+json`), pagination via `links.next`.
- Tokens are per organization, created by Admin/Manager roles, expire after
  1 or 6 months. Store in the macOS Keychain, never in `.shipslip.toml`.
- Deploy: `POST /environments/{id}/deployments` (no body: deploys the head
  of the environment's branch), poll `GET /deployments/{id}` about every 3
  seconds, then `GET /deployments/{id}/logs`.
- Logs: `GET /environments/{id}/logs?from=&to=&cursor=&type=&query=`
  (types: access, application, exception, system).
- Commands: `POST /environments/{id}/commands`, poll `GET /commands/{id}`.
- Workers: `/instances/{id}/background-processes`.
- Failed jobs (managed queues): `GET /instances/{id}/failed-jobs`, retry,
  delete.

Scope:
- `laravel_cloud` target kind with environment ID; token from the Keychain.
- Preview: compare the branch head on GitHub with the environment's current
  deployment commit. Warn that Cloud deploys the branch head at trigger
  time, so the commit can move between preview and deploy; record the
  commit that actually deployed in the receipt.
- Deploy, poll, receipt; Cloud environments on the release map.
- Feed Cloud application/exception logs into the existing error grouping
  ("new since deploy").
- Later: allow-listed commands, workers, failed jobs.

Known gaps: no webhooks or live log stream (poll), no documented rollback or
cancel, no server-side lock (Shipslip's lock only covers Shipslip users), no
documented rate limits or API versioning. Handle 429s and unknown fields.

### Phase 4: later

- Zero-downtime releases (`releases/` + `current` symlink, shared
  `storage` and `.env`) with symlink rollback, including for existing sites.
- Masked `.env` editor with diff and backup.
- Notifications (Slack/email) after a run.
- Creating EC2 servers with cloud-init, using a scoped IAM policy shipped as
  a CloudFormation template.
- Linux build of the desktop app.

## Open questions

- Should the desktop app run the engine in-process, or drive the `slip`
  binary? (In-process matches the existing `app/src-tauri` design.)
- Minimum Laravel version to support for the runtime panel.
- Should declared workers be allowed when the server is managed by Forge,
  which writes its own Supervisor files?

## Sources

- Laravel docs (13.x): https://github.com/laravel/docs/blob/13.x/queues.md,
  https://github.com/laravel/docs/blob/13.x/scheduling.md,
  https://github.com/laravel/docs/blob/13.x/horizon.md,
  https://github.com/laravel/docs/blob/13.x/deployment.md
- Supervisor: https://github.com/Supervisor/supervisor/blob/main/docs/configuration.rst
- Deployer Laravel recipe: https://github.com/deployphp/deployer/blob/master/recipe/laravel.php
- Laravel Cloud API: https://cloud.laravel.com/docs/api/introduction,
  CLI source https://github.com/laravel/cloud-cli
- Laravel Cloud deployments and workers: https://cloud.laravel.com/docs/deployments,
  https://cloud.laravel.com/docs/compute
- AWS SSH over Session Manager: https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-getting-started-enable-ssh-connections.html
- EC2 Instance Connect Endpoint: https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/connect-using-eice.html
- App Runner availability change: https://docs.aws.amazon.com/apprunner/latest/dg/apprunner-availability-change.html
- Laravel Forge: https://laravel.com/forge/pricing,
  https://laravel.com/forge/docs/sites/deployments
