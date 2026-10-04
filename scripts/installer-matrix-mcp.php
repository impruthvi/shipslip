<?php
// Run Boost's generated command after moving its project, with a bounded wait.
$process = null;
$pipes = [];
try {
    $path = $argv[1];
    $config = json_decode(file_get_contents($path.'/.mcp.json'), true, 512, JSON_THROW_ON_ERROR);
    $server = $config['mcpServers']['laravel-boost'] ?? null;
    if (!is_array($server) || !is_string($server['command'] ?? null) || !is_array($server['args'] ?? null)) {
        throw new RuntimeException('Missing generated laravel-boost MCP command');
    }
    foreach ($server['args'] as $arg) {
        if (!is_string($arg)) throw new RuntimeException('Invalid generated MCP argument');
    }
    $command = array_merge([$server['command']], $server['args']);
    $process = proc_open($command, [['pipe', 'r'], ['pipe', 'w'], ['pipe', 'w']], $pipes, $path);
    if (!is_resource($process)) throw new RuntimeException('Cannot start generated Boost MCP command');
    stream_set_blocking($pipes[1], false);
    stream_set_blocking($pipes[2], false);
    $send = function (array $message) use ($pipes): void {
        $line = json_encode($message, JSON_THROW_ON_ERROR)."\n";
        if (fwrite($pipes[0], $line) !== strlen($line)) throw new RuntimeException('Cannot write Boost MCP request');
        fflush($pipes[0]);
    };
    $buffer = '';
    $errors = '';
    $receive = function (int $id) use ($pipes, $process, &$buffer, &$errors): array {
        $deadline = microtime(true) + 20;
        do {
            $buffer .= stream_get_contents($pipes[1]);
            $errors .= stream_get_contents($pipes[2]);
            while (($end = strpos($buffer, "\n")) !== false) {
                $line = substr($buffer, 0, $end);
                $buffer = substr($buffer, $end + 1);
                $message = json_decode($line, true, 512, JSON_THROW_ON_ERROR);
                if (($message['id'] ?? null) !== $id) continue;
                if (isset($message['error'])) throw new RuntimeException('Boost MCP returned an error: '.$line);
                return $message['result'] ?? throw new RuntimeException('Missing Boost MCP result');
            }
            if (!proc_get_status($process)['running']) {
                throw new RuntimeException('Boost MCP exited before replying: '.$errors);
            }
            usleep(10000);
        } while (microtime(true) < $deadline);
        throw new RuntimeException('Boost MCP timed out after relocation: '.$errors);
    };
    $send(['jsonrpc' => '2.0', 'id' => 1, 'method' => 'initialize', 'params' => [
        'protocolVersion' => '2024-11-05', 'capabilities' => new stdClass(),
        'clientInfo' => ['name' => 'shipslip-installer-matrix', 'version' => '1'],
    ]]);
    $initialize = $receive(1);
    if (empty($initialize['protocolVersion'])) throw new RuntimeException('Missing MCP protocol version');
    $send(['jsonrpc' => '2.0', 'method' => 'notifications/initialized']);
    $send(['jsonrpc' => '2.0', 'id' => 2, 'method' => 'tools/list', 'params' => new stdClass()]);
    $tools = $receive(2)['tools'] ?? [];
    if (!$tools) throw new RuntimeException('Boost MCP listed no tools');
    echo 'Relocated Boost MCP initialized and listed '.count($tools).' tools'.PHP_EOL;
} catch (Throwable $error) {
    fwrite(STDERR, $error->getMessage().PHP_EOL);
    $failed = true;
} finally {
    foreach ($pipes as $pipe) fclose($pipe);
    if (is_resource($process)) {
        proc_terminate($process);
        $deadline = microtime(true) + 2;
        while (proc_get_status($process)['running'] && microtime(true) < $deadline) usleep(10000);
        if (proc_get_status($process)['running']) proc_terminate($process, 9);
        proc_close($process);
    }
}
exit(isset($failed) ? 1 : 0);
