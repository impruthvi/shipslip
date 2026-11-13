# Connection check: "read only" means Shipslip's commands

The connection check promises it is read only. That promise covers the commands Shipslip runs, not everything the server does while they run. Every probe Shipslip sends only reads: git runs with `GIT_OPTIONAL_LOCKS=0` so it never refreshes the index, and the rest are `test`/`[ -f ]`, `command -v` and a read of the log file. `no_probe_writes_anything_on_the_server` proves this against a fake `php`.

The one accepted side effect is booting the app with `php artisan --version`. Laravel may write `bootstrap/cache/*.php` (for example `packages.php` when it is missing), and a service provider may log while booting. Shipslip does not control either. It keeps the probe because "the app boots with this PHP and this `vendor`" is the signal worth having, and a deploy runs artisan anyway. The banner says so: Shipslip's commands only read; starting `php artisan` can let Laravel refresh its own cache files.

## Considered Options

- **`php -v` plus a read of `vendor/composer/installed.json`.** Rejected: proves PHP runs and dependencies are installed, but loses the signal that the app itself boots.
- **Keep the claim "nothing on the server changes".** Rejected: it promises more than Shipslip controls, and the tests can't see it because they use a fake `php`.
