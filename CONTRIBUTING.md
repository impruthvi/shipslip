# Contributing

Thanks for helping. Issues and specs live in
[GitHub Issues](https://github.com/impruthvi/shipslip/issues).

## Before your first commit

- Enable the hooks once per clone: `git config core.hooksPath .githooks`. The
  pre-commit hook refuses commits on `main`, so work on a branch
  (`feat/short-name`, `fix/short-name`, …).
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `test:`, `chore:`, `docs:`).
- Review holds changes to [CODING_STANDARDS.md](CODING_STANDARDS.md) and the
  "Rules that must not break" in [AGENTS.md](AGENTS.md#rules-that-must-not-break).
- Name things with the words in [GLOSSARY.md](GLOSSARY.md); read its _Avoid_
  lines first.

## Build and test

The build, test, lint and format commands for the library, CLI and app are in
[AGENTS.md → Commands](AGENTS.md#commands). CI runs all of them; keep each one
clean before you open a pull request. The desktop app's build steps are also
in [docs/app.md](docs/app.md#build-it).

## Try project creation from this checkout

To test `slip new` from source, build the local binary first. Run creation from
a normal non-Git directory, not from inside the Shipslip source checkout:

```sh
cargo build --locked --bin slip
slip_binary="$PWD/target/debug/slip"
mkdir -p "$HOME/shipslip-local-apps"
cd "$HOME/shipslip-local-apps"
"$slip_binary" new my-app --starter-kit react
cd my-app
php artisan serve
```

Use a new app name on subsequent runs. To publish this app with the same build,
run `"$slip_binary" publish github` from its directory. To configure deployment,
run `"$slip_binary" init` after preparing an existing server checkout.

## Check Laravel installer compatibility

From this source checkout, run the script regressions and real installer matrix:

```sh
bash scripts/installer-matrix-test.sh
bash scripts/installer-matrix.sh .context/installer-compat.md
```

The matrix creates eleven temporary apps: nine starter-kit/auth combinations
without Boost, plus plain Laravel and React with Boost. It runs application
checks, moves each scaffold to a path containing spaces, and rebuilds assets.
Boost cases also initialize MCP and list tools after
relocation. The report retains every result and per-case logs on failure.
This takes longer than unit tests and downloads Composer/npm dependencies;
it does not create GitHub repositories. Weekly/manual CI runs the same matrix
on macOS and Linux.

## Docs

README holds the pitch, fit check, quick start, guarantees and links. A
command's details live in its page under [docs/](docs/); when behaviour
changes, update that page and [CHANGELOG.md](CHANGELOG.md).
