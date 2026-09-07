#!/usr/bin/env python3
"""Generate a fixed 10,000-line source workspace and measure the stdio LSP.
Run: python3 scripts/bench-lsp.py [target/release/ruddy-ls] [iterations=100]
No downloaded dependencies. Includes a source path dependency, structural
records/effects, higher-order functions and mutually recursive groups.
"""
import hashlib
import json
import pathlib
import platform
import queue
import subprocess
import sys
import tempfile
import threading
import time
binary = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/ruddy-ls').resolve()
iterations = int(sys.argv[2]) if len(sys.argv) > 2 else 100

def module(dependency=False):
    source = ['effect Log = { write: Real -> () }', 'type Item = { value: Real, label: String }', 'let first = fn x => second x', 'let second = fn x => first x']
    if dependency:
        source[2:4] = ['let normalize = fn value => value', 'let bridge = fn value => normalize value']
    for i in range(16):
        source += [f'let compute{i} = fn item => do', '  let selected = item.value' if dependency else '  let selected = dep::normalize item.value', '  let bumped = selected + 1.0', '  let recorded = handle !Log.write bumped with | !Log.write _ => () end', '  return { value: bumped, label: item.label }', 'end']
    return '\n'.join(source) + '\n'

def percentile(samples):
    return round(sorted(samples)[max(0, int(len(samples) * 0.95 + 0.999) - 1)], 3)
def benchmark(directory):
    tree = pathlib.Path(directory)
    root = tree / 'root'
    dep = tree / 'dep'
    root.mkdir()
    dep.mkdir()
    manifest = 'name = "{}"\nversion = "0.0.0"\nkind = "library"\nroot = "main.rud"\n[dependencies]\nstd = false\n'
    (dep / 'Ruddy.toml').write_text(manifest.format('dep'))
    (root / 'Ruddy.toml').write_text(manifest.format('root') + 'dep = "../dep"\n')
    corpus = {dep / 'main.rud': module(True), root / 'main.rud': '\n'.join([f'module M{i}' for i in range(98)] + ['let selected = M0::compute0 { value: 1.0, label: "x" }', 'let dependency = dep::compute0 selected']) + '\n'}
    corpus.update({root / f'M{i}.rud': module() for i in range(98)})
    assert sum((len(text.splitlines()) for text in corpus.values())) == 10000
    for path, text in corpus.items():
        path.write_text(text)
    digest = hashlib.sha256(''.join(corpus.values()).encode()).hexdigest()
    start = time.perf_counter()
    proc = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    inbox = queue.Queue()
    errors = []

    def reader():
        while True:
            headers = {}
            while (line := proc.stdout.readline()):
                if line == b'\r\n':
                    break
                key, value = line.decode().split(':', 1)
                headers[key.lower()] = value.strip()
            if not line:
                inbox.put((time.perf_counter(), {'error': 'server exited'}))
                return
            message = json.loads(proc.stdout.read(int(headers['content-length'])))
            inbox.put((time.perf_counter(), message))
    threading.Thread(target=reader, daemon=True).start()
    threading.Thread(target=lambda: errors.append(proc.stderr.read().decode()), daemon=True).start()

    def send(message):
        body = json.dumps({'jsonrpc': '2.0', **message}).encode()
        proc.stdin.write(f'Content-Length: {len(body)}\r\n\r\n'.encode() + body)
        proc.stdin.flush()

    def receive(predicate):
        while True:
            at, msg = inbox.get(timeout=60)
            if 'error' in msg:
                raise RuntimeError(msg)
            if predicate(msg):
                return (at, msg)
    send({'id': 1, 'method': 'initialize', 'params': {'rootUri': root.as_uri(), 'capabilities': {}}})
    receive(lambda msg: msg.get('id') == 1)
    send({'method': 'initialized', 'params': {}})
    active = root / 'M0.rud'
    uri = active.as_uri()
    original = corpus[active]
    send({'method': 'textDocument/didOpen', 'params': {'textDocument': {'uri': uri, 'languageId': 'ruddy', 'version': 1, 'text': original}}})
    at, msg = receive(lambda msg: msg.get('method') == 'textDocument/publishDiagnostics' and msg['params'].get('version') == 1)
    if msg['params']['diagnostics']:
        raise RuntimeError(msg['params']['diagnostics'][:3])
    cold = (at - start) * 1000
    background_at, _ = receive(lambda msg: msg.get('method') == 'textDocument/publishDiagnostics' and msg['params'].get('version') == 1)
    initial_background = (background_at - start) * 1000
    timings = {'hover': [], 'completion': [], 'diagnostics': []}
    rss = []
    request = 2
    for i in range(iterations):
        next_active = root / f'M{i % 98}.rud'
        if next_active != active:
            send({'method': 'textDocument/didClose', 'params': {'textDocument': {'uri': uri}}})
            active = next_active
            uri = active.as_uri()
            original = corpus[active]
            send({'method': 'textDocument/didOpen', 'params': {'textDocument': {'uri': uri, 'languageId': 'ruddy', 'version': i + 1, 'text': original}}})
        version = i + 2
        text = original.replace('selected + 1.0', f'selected + {2 + i % 8}.0', 1)
        start = time.perf_counter()
        send({'method': 'textDocument/didChange', 'params': {'textDocument': {'uri': uri, 'version': version}, 'contentChanges': [{'text': text}]}})
        for method in ['hover', 'completion']:
            send({'id': request, 'method': f'textDocument/{method}', 'params': {'textDocument': {'uri': uri}, 'position': {'line': 6, 'character': 20}}})
            request += 1
        awaiting = {request - 2: 'hover', request - 1: 'completion'}
        diagnostics = False
        while awaiting or not diagnostics:
            at, msg = receive(lambda _: True)
            if msg.get('id') in awaiting:
                timings[awaiting.pop(msg['id'])].append((at - start) * 1000)
                if msg.get('error'):
                    raise RuntimeError(msg)
            elif msg.get('method') == 'textDocument/publishDiagnostics' and msg['params'].get('version') == version and (msg['params'].get('uri') == uri):
                if not diagnostics:
                    timings['diagnostics'].append((at - start) * 1000)
                diagnostics = True
                if msg['params']['diagnostics']:
                    raise RuntimeError(msg['params']['diagnostics'][:3])
        status = pathlib.Path(f'/proc/{proc.pid}/status').read_text()
        rss.append(int(next((line.split()[1] for line in status.splitlines() if line.startswith('VmRSS:')))) * 1024)
    receive(lambda msg: msg.get('method') == 'textDocument/publishDiagnostics' and msg['params'].get('uri') == uri and (msg['params'].get('version') == version))
    widened = corpus[dep / 'main.rud'].replace('let normalize = fn value => value', 'let normalize = fn value => value + 0.0')
    start = time.perf_counter()
    send({'method': 'textDocument/didOpen', 'params': {'textDocument': {'uri': (dep / 'main.rud').as_uri(), 'languageId': 'ruddy', 'version': 1, 'text': widened}}})
    send({'id': request, 'method': 'textDocument/hover', 'params': {'textDocument': {'uri': uri}, 'position': {'line': 6, 'character': 20}}})
    wide_hover = None
    wide_diagnostics = []
    while wide_hover is None or len(wide_diagnostics) < 2:
        at, msg = receive(lambda _: True)
        if msg.get('id') == request:
            wide_hover = (at - start) * 1000
        elif msg.get('method') == 'textDocument/publishDiagnostics' and msg['params'].get('uri') == uri and (msg['params'].get('version') == version):
            if msg['params']['diagnostics']:
                raise RuntimeError(msg['params']['diagnostics'][:3])
            wide_diagnostics.append((at - start) * 1000)
    peak_status = pathlib.Path(f'/proc/{proc.pid}/status').read_text()
    peak_rss = int(next((line.split()[1] for line in peak_status.splitlines() if line.startswith('VmHWM:')))) * 1024
    request += 1
    send({'id': request, 'method': 'shutdown', 'params': None})
    receive(lambda msg: msg.get('id') == request)
    send({'method': 'exit', 'params': None})
    proc.stdin.close()
    proc.wait(timeout=10)
    print(json.dumps({'machine': platform.platform(), 'cpu': next((line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), ''), 'profile': 'release', 'benchmark_project': str(tree), 'corpus_lines': 10000, 'corpus_sha256': digest, 'dependencies': 'one local path dependency; std disabled; no downloads', 'iterations': iterations, 'samples_ms': timings, 'cold_ms': round(cold, 3), 'initial_background_ms': round(initial_background, 3), 'interface_edit_ms': {'hover': round(wide_hover, 3), 'background': round(wide_diagnostics[-1], 3)}, 'p95_ms': {key: percentile(values) for key, values in timings.items()}, 'rss_peak_mib': round(peak_rss / 1048576, 2), 'rss_max_mib': round(max(rss) / 1048576, 2), 'rss_first_mib': round(rss[0] / 1048576, 2), 'rss_last_mib': round(rss[-1] / 1048576, 2)}, indent=2))

# Retain every generated workspace for manual inspection and editor testing.
# A new directory per run avoids overwriting edits made to an earlier corpus.
projects = pathlib.Path(__file__).resolve().parents[1] / '.scratch' / 'salsa-query-architecture' / 'benchmarks'
projects.mkdir(parents=True, exist_ok=True)
benchmark(tempfile.mkdtemp(prefix='workspace-', dir=projects))
