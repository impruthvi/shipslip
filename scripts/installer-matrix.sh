#!/usr/bin/env bash
set -euo pipefail
umask 077

# Run from any directory; retain diagnostics, but delete only our own temp tree.
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
report=${1:-"$repo_root/.context/installer-compat.md"}
mkdir -p "$(dirname "$report")"
report=$(cd "$(dirname "$report")" && pwd)/$(basename "$report")
logs=$(mktemp -d "$(dirname "$report")/installer-matrix-logs.XXXXXX")
work=$(mktemp -d "${TMPDIR:-/tmp}/shipslip-installer-matrix.XXXXXX")
finished=false
cleanup() {
    if [ "$finished" = true ]; then
        rm -rf "$work"
    else
        printf '\nInterrupted or failed before completion. Temp scaffolds retained at `%s`.\n' "$work" >> "$report"
        printf 'Temp scaffolds retained at %s\n' "$work" >&2
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# These must never reach the installer or any Composer/npm child process.
unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN SHIPSLIP_GITHUB_TOKEN

printf '# Laravel installer compatibility matrix\n\nRun: `%s`\n\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" > "$report"
printf 'Platform: `%s`\n\nLogs: `%s`\n\n' "$(uname -sm)" "$logs" >> "$report"
for tool in php composer laravel git node npm; do
    if ! command -v "$tool" > /dev/null 2>&1; then
        printf 'Prerequisite missing: `%s`. Matrix not run.\n' "$tool" >> "$report"
        printf 'Missing prerequisite: %s\n' "$tool" >&2
        exit 1
    fi
done

installer_version=$(laravel --version < /dev/null)
laravel new --help < /dev/null > "$logs/installer-help.txt"
printf 'Installer: `%s`\n\n' "$installer_version" >> "$report"
for tool in php composer git node npm; do
    "$tool" --version < /dev/null > "$logs/$tool-version.txt" 2>&1
    printf '%s: `%s`\n\n' "$tool" "$(head -n 1 "$logs/$tool-version.txt")" >> "$report"
done
for flag in --react --vue --svelte --livewire --no-authentication --database --pest --npm --boost --no-boost --no-interaction; do
    if ! grep -F -- "$flag" "$logs/installer-help.txt" > /dev/null; then
        printf 'Required installer flag missing: `%s`. Upgrade the Laravel installer.\n' "$flag" >> "$report"
        printf 'Required installer flag missing: %s\n' "$flag" >&2
        exit 1
    fi
done

cat >> "$report" <<'TEXT'
The nine baseline calls use `--no-boost`; two additional plain/React calls use `--boost`. All eleven calls use `-n`, stdin `/dev/null`, SQLite, explicit `--pest`, and `--npm`. No call passes `--git` or `--github`. GitHub token variables are removed for every child process.

Scaffold checks: no `.git`; Artisan, Composer lock/autoloader, npm lock/build script, nonempty Vite manifest and its assets; expected kit packages and auth dependencies; Pest installed; `php artisan about` and `php artisan test`. Each scaffold is then moved to a different directory (including spaces) and must pass `php artisan about`, `npm run build`, and manifest/asset validation again. Boost cases must lock `laravel/boost` and initialize MCP/list tools from the relocated project, using a generated client command when configured or the installed server directly when Boost selected no client.

| Kit | Auth | Boost | Framework | Result | Failed stage |
| --- | --- | --- | --- | --- | --- |
TEXT

validate_scaffold() {
    local path=$1 kit=$2 auth=$3 boost=$4
    [ ! -e "$path/.git" ] && [ ! -L "$path/.git" ] || { echo 'Installer created .git'; return 1; }
    php /dev/stdin "$path" "$kit" "$auth" "$boost" <<'PHP'
<?php
[$script, $path, $kit, $auth, $boost] = $argv;
function fail(string $message): never {
    fwrite(STDERR, $message.PHP_EOL);
    exit(1);
}
function jsonFile(string $path): array {
    return json_decode(file_get_contents($path), true, 512, JSON_THROW_ON_ERROR);
}
foreach (['artisan', 'composer.json', 'composer.lock', 'vendor/autoload.php', 'package.json', 'package-lock.json', 'public/build/manifest.json'] as $file) {
    if (!is_file($path.'/'.$file)) fail('Missing scaffold file: '.$file);
}
$composer = jsonFile($path.'/composer.json');
$npm = jsonFile($path.'/package.json');
$phpPackages = array_merge($composer['require'] ?? [], $composer['require-dev'] ?? []);
$nodePackages = array_merge($npm['dependencies'] ?? [], $npm['devDependencies'] ?? []);
$lock = jsonFile($path.'/composer.lock');
$installed = array_column(array_merge($lock['packages'], $lock['packages-dev']), 'version', 'name');
if (!isset($installed['laravel/framework'])) fail('Missing locked Laravel framework');
if (!isset($installed['pestphp/pest'])) fail('Explicit Pest selection was not installed');
if (isset($installed['laravel/boost']) !== ($boost === 'on')) fail('Boost lock does not match requested selection');
if (empty($npm['scripts']['build'])) fail('Missing npm build script');
if ($kit === 'none') {
    if (($composer['name'] ?? '') !== 'laravel/laravel') fail('No-kit scaffold is not the Laravel skeleton');
    foreach (array_keys(array_merge($phpPackages, $nodePackages)) as $package) {
        if (preg_match('~^(react(?:-dom)?|vue|svelte|(?:@inertiajs|inertiajs)/.*|@vitejs/plugin-(?:react.*|vue)|@sveltejs/.*|livewire/.*|laravel/(?:fortify|.*starter-kit))$~', $package)) {
            fail('No-kit scaffold contains kit package: '.$package);
        }
    }
} else {
    $markers = match ($kit) {
        'react' => ['react', 'react-dom', '@inertiajs/react'],
        'vue' => ['vue', '@inertiajs/vue3'],
        'svelte' => ['svelte', '@inertiajs/svelte'],
        'livewire' => ['livewire/livewire'],
    };
    foreach ($markers as $marker) {
        if (!isset($phpPackages[$marker]) && !isset($nodePackages[$marker])) fail('Missing kit marker: '.$marker);
    }
    if (isset($installed['laravel/fortify']) !== ($auth === 'laravel')) fail('Auth dependencies do not match requested auth');
}
$manifest = jsonFile($path.'/public/build/manifest.json');
if (!$manifest) fail('Empty Vite manifest');
foreach ($manifest as $entry) {
    foreach (array_merge([$entry['file'] ?? ''], $entry['css'] ?? [], $entry['assets'] ?? []) as $asset) {
        if ($asset === '' || !is_file($path.'/public/build/'.$asset)) fail('Missing built asset: '.$asset);
    }
}
echo 'Validated Laravel '.$installed['laravel/framework'].PHP_EOL;
PHP
}

run_case() {
    local kit=$1 auth=$2 boost=${3:-off} case_name="$1-$2"
    if [ "$boost" = on ]; then case_name="$case_name-boost"; fi
    cases=$((cases + 1))
    local source="$work/$case_name" moved="$work/relocated projects/$case_name"
    local log="$logs/$case_name.log" framework=unknown status=PASS stage=installer
    local args=(new "$source" -n)
    if [ "$kit" != none ]; then args+=("--$kit"); fi
    if [ "$auth" = none ]; then args+=(--no-authentication); fi
    args+=(--database=sqlite --pest --npm)
    if [ "$boost" = on ]; then args+=(--boost); else args+=(--no-boost); fi
    printf 'Running %s...\n' "$case_name"
    printf 'laravel' > "$logs/$case_name.argv"
    printf ' %q' "${args[@]}" >> "$logs/$case_name.argv"
    printf '\n' >> "$logs/$case_name.argv"
    if ! laravel "${args[@]}" < /dev/null > "$log" 2>&1; then
        status=FAIL
    fi
    if [ -f "$source/composer.lock" ]; then
        if ! framework=$(php -r '$lock = json_decode(file_get_contents($argv[1]), true, 512, JSON_THROW_ON_ERROR); foreach ($lock["packages"] as $package) { if ($package["name"] === "laravel/framework") { echo $package["version"]; exit; } } echo "unknown";' "$source/composer.lock" 2>> "$log"); then
            framework=unknown
            printf 'Cannot decode framework version from composer.lock.\n' >> "$log"
            if [ "$status" = PASS ]; then status=FAIL; stage=framework-version; fi
        fi
    fi
    if [ "$status" = PASS ]; then
        stage=scaffold-validation
        if ! validate_scaffold "$source" "$kit" "$auth" "$boost" >> "$log" 2>&1; then status=FAIL; fi
    fi
    if [ "$status" = PASS ]; then
        stage=artisan-about
        if ! (cd "$source" && php artisan about --no-interaction < /dev/null) >> "$log" 2>&1; then status=FAIL; fi
    fi
    if [ "$status" = PASS ]; then
        stage=artisan-test
        if ! (cd "$source" && php artisan test < /dev/null) >> "$log" 2>&1; then status=FAIL; fi
    fi
    if [ "$status" = PASS ]; then
        stage=move
        if ! mv "$source" "$moved" >> "$log" 2>&1; then status=FAIL; fi
    fi
    if [ "$status" = PASS ]; then
        stage=relocated-artisan-about
        if ! (cd "$moved" && php artisan about --no-interaction < /dev/null) >> "$log" 2>&1; then status=FAIL; fi
    fi
    if [ "$status" = PASS ]; then
        stage=relocated-npm-build
        rm -rf "$moved/public/build"
        if ! (cd "$moved" && npm run build < /dev/null) >> "$log" 2>&1; then status=FAIL; fi
    fi
    if [ "$status" = PASS ]; then
        stage=relocated-validation
        if ! validate_scaffold "$moved" "$kit" "$auth" "$boost" >> "$log" 2>&1; then status=FAIL; fi
    fi
    if [ "$status" = PASS ] && [ "$boost" = on ]; then
        stage=relocated-boost-mcp
        if ! php "$repo_root/scripts/installer-matrix-mcp.php" "$moved" >> "$log" 2>&1; then status=FAIL; fi
    fi
    if [ "$status" = PASS ]; then stage=none; else failures=$((failures + 1)); fi
    printf '| %s | %s | %s | %s | %s | %s |\n' "$kit" "$auth" "$boost" "$framework" "$status" "$stage" >> "$report"
    printf '%s: %s (%s)\n' "$case_name" "$status" "$stage"
}

mkdir -p "$work/relocated projects"
failures=0
cases=0
run_case none none
for kit in react vue svelte livewire; do
    for auth in laravel none; do
        run_case "$kit" "$auth"
    done
done
run_case none none on
run_case react laravel on
printf '\n## Exact installer commands\n\n```sh\n' >> "$report"
cat "$logs/"*.argv >> "$report"
printf '```\n\nResult: **%s/%s passed**.\n' "$((cases - failures))" "$cases" >> "$report"
if [ "$failures" -ne 0 ]; then
    printf '\nFailed scaffolds retained at `%s`. See per-combination logs for diagnostics.\n' "$work" >> "$report"
    printf 'Matrix failed: %s combinations. Report: %s\n' "$failures" "$report" >&2
    # The EXIT trap also preserves scaffolds on failures and interruptions.
    exit 1
fi
cat >> "$report" <<'TEXT'

The no-kit invocation produced plain Laravel without a starter-kit package. All eleven scaffolds survived relocation and rebuilt successfully, including two Boost MCP checks. This run proves compatibility for the recorded installer version only; it does not establish support for older releases.
TEXT
finished=true
printf 'Matrix passed: %s/%s. Report: %s\n' "$cases" "$cases" "$report"
