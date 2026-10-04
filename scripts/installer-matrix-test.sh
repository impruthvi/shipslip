#!/usr/bin/env bash
set -euo pipefail
umask 077
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
php_binary=$(command -v php)
work=$(mktemp -d "${TMPDIR:-/tmp}/shipslip-matrix-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/temp"
ln -s "$php_binary" "$work/bin/php"
for tool in composer git node npm; do
    cat > "$work/bin/$tool" <<'TOOL'
#!/usr/bin/env bash
printf 'fake version 1\n'
TOOL
    chmod +x "$work/bin/$tool"
done
cat > "$work/bin/laravel" <<'INSTALLER'
#!/usr/bin/env bash
set -eu
if [ "$1" = --version ]; then printf 'Laravel Installer 5.31.1\n'; exit 0; fi
if [ "$2" = --help ]; then
    printf '%s\n' --react --vue --svelte --livewire --no-authentication --database --pest --npm --boost --no-boost --no-interaction
    exit 0
fi
for token in GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN SHIPSLIP_GITHUB_TOKEN; do
    if [ -n "${!token:-}" ]; then echo 'FAIL: inherited token' >&2; exit 91; fi
done
if read -r unexpected; then echo 'FAIL: inherited stdin' >&2; exit 92; fi
mkdir -p "$2"
if [ "$MATRIX_FIXTURE" = missing-lock ]; then exit 1; fi
printf '{broken lock' > "$2/composer.lock"
exit "$INSTALLER_EXIT"
INSTALLER
chmod +x "$work/bin/laravel"
for fixture in malformed-failed malformed-success missing-lock; do
    report="$work/$fixture.md"
    installer_exit=1
    expected_stage=installer
    if [ "$fixture" = malformed-success ]; then installer_exit=0; expected_stage=framework-version; fi
    if env PATH="$work/bin:$PATH" TMPDIR="$work/temp" MATRIX_FIXTURE="$fixture" INSTALLER_EXIT="$installer_exit" \
        GH_TOKEN=fixture-token GITHUB_TOKEN=fixture-token GH_ENTERPRISE_TOKEN=fixture-token \
        GITHUB_ENTERPRISE_TOKEN=fixture-token SHIPSLIP_GITHUB_TOKEN=fixture-token \
        /bin/bash "$repo_root/scripts/installer-matrix.sh" "$report" < /dev/null > "$work/$fixture.out" 2>&1; then
        echo "$fixture unexpectedly passed" >&2
        exit 1
    fi
    [ "$(grep -c '| unknown | FAIL |' "$report")" = 11 ]
    [ "$(grep -c "| $expected_stage |" "$report")" = 11 ]
    grep -F 'Result: **0/11 passed**.' "$report" > /dev/null
    grep -F 'Running react-laravel-boost...' "$work/$fixture.out" > /dev/null
    logs=$(sed -n 's/^Logs: `\(.*\)`$/\1/p' "$report")
    [ "$(grep -l -F -- ' --boost' "$logs/"*.argv | wc -l | tr -d ' ')" = 2 ]
    [ "$(grep -l -F -- ' --no-boost' "$logs/"*.argv | wc -l | tr -d ' ')" = 9 ]
    if grep -R -F 'FAIL: inherited' "$work"/installer-matrix-logs.* > /dev/null; then
        echo 'Installer inherited a token or stdin' >&2
        exit 1
    fi
    if [ "$fixture" != missing-lock ]; then
        count=$(grep -l 'Cannot decode framework version' "$work"/installer-matrix-logs.*/*.log | wc -l | tr -d ' ')
        # Previous fixture logs are retained; both malformed cases must log every row.
        if [ "$fixture" = malformed-failed ]; then [ "$count" = 11 ]; else [ "$count" = 22 ]; fi
    fi
    printf 'PASS: %s reports all eleven rows, preserves diagnostics, and completes tally\n' "$fixture"
done
mkdir "$work/mcp-before"
cat > "$work/mcp-before/.mcp.json" <<'CONFIG'
{"mcpServers":{"laravel-boost":{"command":"php","args":["artisan","boost:mcp"]}}}
CONFIG
cat > "$work/mcp-before/artisan" <<'PHP'
<?php
if (basename(getcwd()) !== 'relocated mcp fixture' || ($argv[1] ?? '') !== 'boost:mcp') exit(4);
while (($line = fgets(STDIN)) !== false) {
    $request = json_decode($line, true, 512, JSON_THROW_ON_ERROR);
    if ($request['method'] === 'initialize') {
        $result = ['protocolVersion' => '2024-11-05'];
    } elseif ($request['method'] === 'tools/list') {
        $result = ['tools' => getenv('MATRIX_EMPTY_TOOLS') ? [] : [['name' => 'fixture-tool']]];
    } else {
        continue;
    }
    echo json_encode(['jsonrpc' => '2.0', 'id' => $request['id'], 'result' => $result])."\n";
    fflush(STDOUT);
}
PHP
mv "$work/mcp-before" "$work/relocated mcp fixture"
php "$repo_root/scripts/installer-matrix-mcp.php" "$work/relocated mcp fixture" > "$work/mcp.out"
grep -F 'initialized and listed 1 tools' "$work/mcp.out" > /dev/null
if MATRIX_EMPTY_TOOLS=1 php "$repo_root/scripts/installer-matrix-mcp.php" "$work/relocated mcp fixture" > "$work/mcp-empty.out" 2>&1; then
    echo 'Empty MCP tools unexpectedly passed' >&2
    exit 1
fi
grep -F 'Boost MCP listed no tools' "$work/mcp-empty.out" > /dev/null
printf 'PASS: generated relative MCP command runs in relocated directory; empty tools rejected\n'
rm "$work/relocated mcp fixture/.mcp.json"
mkdir "$work/relocated mcp fixture/.vscode"
cat > "$work/relocated mcp fixture/.vscode/mcp.json" <<'CONFIG'
{"servers":{"laravel-boost":{"command":"php","args":["artisan","boost:mcp"]}}}
CONFIG
php "$repo_root/scripts/installer-matrix-mcp.php" "$work/relocated mcp fixture" > "$work/mcp-vscode.out"
grep -F 'initialized and listed 1 tools' "$work/mcp-vscode.out" > /dev/null
rm "$work/relocated mcp fixture/.vscode/mcp.json"
mkdir "$work/relocated mcp fixture/.codex"
cat > "$work/relocated mcp fixture/.codex/config.toml" <<'CONFIG'
[mcp_servers.laravel-boost]
command = "php"
args = ["artisan", "boost:mcp"]
CONFIG
php "$repo_root/scripts/installer-matrix-mcp.php" "$work/relocated mcp fixture" > "$work/mcp-codex.out"
grep -F 'initialized and listed 1 tools' "$work/mcp-codex.out" > /dev/null
rm "$work/relocated mcp fixture/.codex/config.toml"
cat > "$work/relocated mcp fixture/boost.json" <<'CONFIG'
{"agents":[],"mcp":true}
CONFIG
php "$repo_root/scripts/installer-matrix-mcp.php" "$work/relocated mcp fixture" > "$work/mcp-no-client.out"
grep -F 'No AI client selected' "$work/mcp-no-client.out" > /dev/null
grep -F 'initialized and listed 1 tools' "$work/mcp-no-client.out" > /dev/null
cat > "$work/relocated mcp fixture/boost.json" <<'CONFIG'
{"mcp":true}
CONFIG
php "$repo_root/scripts/installer-matrix-mcp.php" "$work/relocated mcp fixture" > "$work/mcp-omitted-clients.out"
grep -F 'No AI client selected' "$work/mcp-omitted-clients.out" > /dev/null
grep -F 'initialized and listed 1 tools' "$work/mcp-omitted-clients.out" > /dev/null
cat > "$work/relocated mcp fixture/boost.json" <<'CONFIG'
{"agents":["claude_code"],"mcp":true}
CONFIG
if php "$repo_root/scripts/installer-matrix-mcp.php" "$work/relocated mcp fixture" > "$work/mcp-missing.out" 2>&1; then
    echo 'Missing selected-client MCP configuration unexpectedly passed' >&2
    exit 1
fi
grep -F 'Missing generated Laravel Boost MCP configuration' "$work/mcp-missing.out" > /dev/null
printf 'PASS: VS Code/Codex configurations and no-client server check; missing selected-client config rejected\n'
printf 'PASS: Bash %s; stdin null and all five token variables removed\n' "$BASH_VERSION"
