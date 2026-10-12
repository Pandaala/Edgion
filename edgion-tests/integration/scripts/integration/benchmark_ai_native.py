#!/usr/bin/env python3
"""Bounded local H1 native-Chat measurements inside the canonical shared topology."""
import argparse
import asyncio
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import socket
import subprocess
import sys
import threading
import time
import uuid

import httpx

MODE = "release-integration"
FRAME_PAYLOAD = 16384
MAX_SAMPLES = 4096
MAX_REQUESTS = 10000
COHORT_SECONDS = 5
TIMED_REPEATS = 5
TERMINAL = b'data: {"id":"perf","object":"chat.completion.chunk","created":0,"model":"perf-model","choices":[],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}\n\ndata: [DONE]\n\n'


class MeasurementFailure(RuntimeError):
    """Fixed fixture-owned diagnostic, safe to persist without request material."""


def require(condition, message):
    if not condition:
        raise MeasurementFailure(message)


def frame(payload):
    require(1 <= payload <= FRAME_PAYLOAD, "Invalid frame size")
    return (b'data: {"id":"perf","object":"chat.completion.chunk","created":0,"model":"perf-model","choices":[{"index":0,"delta":{"content":"'
            + b'X' * payload + b'"},"finish_reason":null}]}\n\n')


def expected(streaming, payload, frames):
    require(1 <= frames <= (1024 ** 3 - 4096) // (payload + 1024), "Invalid frame count")
    digest = hashlib.sha256()
    if streaming:
        chunk = frame(payload)
        for _ in range(frames):
            digest.update(chunk)
        digest.update(TERMINAL)
        count = len(chunk) * frames + len(TERMINAL)
    else:
        chunk = (b'{"id":"perf","object":"chat.completion","created":0,"model":"perf-model","choices":[{"index":0,"message":{"role":"assistant","content":"'
                 + b'X' * payload + b'"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}')
        digest.update(chunk)
        count = len(chunk)
    return count, digest.hexdigest()


def percentile(values, quantile):
    require(bool(values) and all(math.isfinite(v) and v >= 0 for v in values), "Invalid measurement samples")
    ordered = sorted(values)
    pos = (len(ordered) - 1) * quantile
    lo, hi = math.floor(pos), math.ceil(pos)
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (pos - lo)


def distribution(values):
    return {"n": len(values), "p50_ms": percentile(values, .5), "p95_ms": percentile(values, .95), "p99_ms": percentile(values, .99)}


def cpu_seconds(value):
    days, sep, value_part = value.partition('-')
    if not sep:
        value_part, days = days, '0'
    parts = value_part.split(':')
    require(2 <= len(parts) <= 3, "Invalid ps CPU time")
    result = int(days) * 86400
    for i, component in enumerate(reversed(parts)):
        result += float(component) * 60 ** i
    require(math.isfinite(result) and result >= 0, "Invalid ps CPU value")
    resolution = 10 ** (-len(parts[-1].split('.')[1])) if '.' in parts[-1] else 1.0
    return result, resolution


def validate_sample_line(line):
    columns = line.split()
    require(len(columns) == 3, "Invalid ps sample")
    pid, rss = int(columns[0]), int(columns[1])
    require(pid > 1 and rss >= 0, "Invalid process sample")
    cpu, resolution = cpu_seconds(columns[2])
    return pid, {"rss_kib": rss, "cpu_seconds": cpu, "cpu_resolution_seconds": resolution}


class Sampler:
    def __init__(self, pids):
        self.pids = pids
        self.rows = []
        self.error = None
        self.delivered = 0
        self.stop_event = threading.Event()
        self.thread = threading.Thread(target=self.loop, daemon=True)
        self.started = time.monotonic()

    def sample(self):
        result = subprocess.run(['ps', '-p', ','.join(str(p) for p in self.pids.values()), '-o', 'pid=,rss=,time='],
                                capture_output=True, text=True, timeout=2, check=True)
        require(len(result.stdout) < 4096, "Oversized ps output")
        parsed = dict(validate_sample_line(line) for line in result.stdout.splitlines() if line.strip())
        require(set(parsed) == set(self.pids.values()), "An owned measurement process disappeared")
        require(len(self.rows) < MAX_SAMPLES, "Sampler history cap exceeded")
        self.rows.append({"elapsed_seconds": time.monotonic() - self.started,
                          "delivered_bytes": self.delivered,
                          "processes": {role: parsed[pid] for role, pid in self.pids.items()}})

    def loop(self):
        try:
            while not self.stop_event.wait(.1):
                self.sample()
        except Exception:
            self.error = "Process sampling failed"

    def __enter__(self):
        self.sample()
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.stop_event.set()
        self.thread.join(timeout=3)
        require(not self.thread.is_alive(), "Sampler failed to stop")
        require(self.error is None, self.error or "Sampler failed")
        self.sample()

    def summary(self):
        out = {}
        for role in self.pids:
            rows = [r['processes'][role] for r in self.rows]
            delta = rows[-1]['cpu_seconds'] - rows[0]['cpu_seconds']
            require(delta >= 0, "Non-monotonic process CPU")
            resolution = max(r['cpu_resolution_seconds'] for r in rows)
            out[role] = {"cpu_seconds_delta": delta, "cpu_resolution_seconds": resolution,
                         "cpu_resolution_limited": delta < resolution * 10,
                         "rss_start_kib": rows[0]['rss_kib'], "rss_peak_kib": max(r['rss_kib'] for r in rows),
                         "rss_end_kib": rows[-1]['rss_kib']}
        return out


def body(streaming):
    data = {"model": "perf-model", "messages": [{"role": "user", "content": "hello"}], "stream": streaming}
    if streaming:
        data['stream_options'] = {"include_usage": True}
    return json.dumps(data, separators=(',', ':')).encode()


def headers(route, trace=None):
    result = {"Host": 'perf-' + route + '.example.com', "Content-Type": 'application/json', "Accept-Encoding": 'identity'}
    if trace:
        result.update({"traceparent": '00-' + trace + '-0123456789abcdef-00', 'x-trace-id': trace,
                       'access_log': 'test_store', 'x-ai-performance-observe': 'true'})
    return result


def url(port, route, payload, frames):
    path = '/ai-performance/v1/chat/completions' if route == 'direct' else '/v1/chat/completions'
    return f'http://127.0.0.1:{port}{path}?payload={payload}&frames={frames}'


async def exchange(client, port, route, streaming, payload, frames, wanted, sampler=None, rate=None, trace=None):
    started = time.monotonic()
    first = None
    count, digest = 0, hashlib.sha256()
    async with client.stream('POST', url(port, route, payload, frames), content=body(streaming), headers=headers(route, trace)) as response:
        require(response.status_code == 200, f"Benchmark response status failed ({response.status_code})")
        require(response.http_version == 'HTTP/1.1', "Benchmark transport is not H1")
        require(response.headers.get('content-encoding', 'identity') == 'identity', "Benchmark response was encoded")
        require(response.headers.get('content-type', '').startswith('text/event-stream' if streaming else 'application/json'), "Invalid benchmark content type")
        async for chunk in response.aiter_raw(chunk_size=16384 if rate else None):
            if not chunk:
                continue
            if first is None:
                first = time.monotonic()
            count += len(chunk)
            require(count <= wanted[0], "Benchmark wire byte cap exceeded")
            digest.update(chunk)
            if sampler:
                sampler.delivered = count
            if rate:
                await asyncio.sleep(max(0, count / rate - (time.monotonic() - started)))
    require(count == wanted[0] and digest.hexdigest() == wanted[1], "Benchmark wire digest/count mismatch")
    require(first is not None, "Benchmark returned no body")
    return (time.monotonic() - started) * 1000, (first - started) * 1000, count


async def timed_cohort(port, route, streaming, concurrency, pids):
    wanted = expected(streaming, 1024, 8)
    limits = httpx.Limits(max_connections=concurrency, max_keepalive_connections=concurrency)
    async with httpx.AsyncClient(timeout=10, trust_env=False, follow_redirects=False, http2=False, limits=limits) as client:
        for _ in range(8):
            await exchange(client, port, route, streaming, 1024, 8, wanted)
        latencies, firsts = [], []
        issued = 0
        started = time.monotonic()
        until = started + COHORT_SECONDS
        async def worker():
            nonlocal issued
            while time.monotonic() < until and issued < MAX_REQUESTS:
                issued += 1
                latency, first, _ = await exchange(client, port, route, streaming, 1024, 8, wanted)
                latencies.append(latency)
                firsts.append(first)
        with Sampler(pids) as sampler:
            await asyncio.wait_for(asyncio.gather(*(worker() for _ in range(concurrency))), timeout=15)
        elapsed = time.monotonic() - started
        require(len(latencies) == issued and issued > 0, "Incomplete timed cohort")
        return {"route": route, "stream": streaming, "concurrency": concurrency, "duration_seconds": elapsed,
                "requests": issued, "request_cap_reached": issued == MAX_REQUESTS,
                "closed_loop_rps": issued / elapsed, "bytes": wanted[0] * issued,
                "latency": distribution(latencies), "first_body_byte": distribution(firsts), "processes": sampler.summary()}


async def memory_cohort(port, route, mib, wanted, pids):
    frames = mib * 1024 ** 2 // FRAME_PAYLOAD
    async with httpx.AsyncClient(timeout=10, trust_env=False, http2=False) as client:
        with Sampler(pids) as sampler:
            latency, first, count = await asyncio.wait_for(
                exchange(client, port, route, True, FRAME_PAYLOAD, frames, wanted, sampler, rate=16 * 1024 ** 2), timeout=35)
            completed_at = time.monotonic() - sampler.started
            await asyncio.sleep(.5)
        return {"route": route, "payload_mib": mib, "concurrency": 1, "consumer_bytes_per_second": 16 * 1024 ** 2,
                "delivered_bytes": count, "duration_ms": latency, "first_body_byte_ms": first,
                "completed_at_seconds": completed_at, "processes": sampler.summary(), "series": sampler.rows}


def api_json(client, address, method='GET', token=None):
    kwargs = {'headers': {'Authorization': 'Bearer ' + token}} if token else {}
    with client.stream(method, address, **kwargs) as response:
        require(response.status_code in (200, 204, 404), "Measurement API status failed")
        if response.status_code == 204:
            return None
        chunks, size = [], 0
        for chunk in response.iter_raw():
            size += len(chunk)
            require(size <= 1024 ** 2, "Measurement API byte cap exceeded")
            chunks.append(chunk)
        return json.loads(b''.join(chunks))


def observation(client, mock_port, trace):
    state = api_json(client, f'http://127.0.0.1:{mock_port}/ai-long-stream/observation/{trace}')
    require(state.get('found'), "Missing producer observation")
    item = state['observation']
    require(not item['provider_completed'] and not item['deadline_expired'], "Producer completed/expired during blocked probe")
    return item


def plateau(client, mock_port, trace):
    until = time.monotonic() + 3
    last = None
    stable = 0
    series = []
    while time.monotonic() < until:
        item = observation(client, mock_port, trace)
        count = item['bytes_queued']
        stable = stable + 1 if count == last and item['pending_send'] and count > 0 else 0
        last = count
        series.append({"bytes_queued": count, "pending_send": item['pending_send']})
        if stable >= 5:
            return count, series
        time.sleep(.1)
    raise MeasurementFailure("Upstream did not plateau under a stopped socket")


def ledger(client, admin_port, token, trace, route, cancelled):
    address = f'http://127.0.0.1:{admin_port}/api/v1/testing/access-log/{trace}'
    entry = None
    until = time.monotonic() + 4
    while time.monotonic() < until:
        entry = api_json(client, address, token=token).get('data')
        if entry:
            break
        time.sleep(.1)
    require(entry is not None, "Missing canonical access log")
    logs = api_json(client, f'http://127.0.0.1:{admin_port}/api/v1/testing/access-logs', token=token)['data']
    matching = [item for item in logs if item['trace_id'] == trace]
    require(len(matching) == 1 and matching[0]['store_count'] == 1, "Canonical finalization wrote more than once")
    ai = entry['genai']
    attempts = ai.get('attempts', [])
    require(len(attempts) == 1 and attempts[0]['number'] == 1 and attempts[0]['status'] == 200, "Invalid canonical attempt ledger")
    if cancelled:
        require(attempts[0].get('failure') == 'incomplete_response', "Missing incomplete attempt outcome")
    else:
        require(not attempts[0].get('failure'), "Complete benchmark attempt failed")
    quality = ai['usage_source']
    require(quality == attempts[0]['usage_source'], "Attempt/request quality mismatch")
    if route == 'off':
        require(quality == 'notObserved', "Off observation unexpectedly parsed usage")
        for owner in [ai, attempts[0]]:
            require(not any(key in owner for key in ['input_tokens', 'output_tokens', 'total_tokens']), "Off observation invented token counts")
    elif cancelled:
        require(quality in ('estimated', 'unknown'), "Incomplete provider response claimed reported usage")
    else:
        require(quality == 'reported' and ai['input_tokens'] == 2 and ai['output_tokens'] == 3, "Provider usage mismatch")
    return {"store_count": 1, "attempt_count": 1, "usage_source": quality,
            "failure": attempts[0].get('failure'), "status": attempts[0]['status']}


def clean_trace(client, mock_port, admin_port, token, trace):
    # Close the data socket before calling this. Wait for the terminal log so a
    # delayed canonical write cannot recreate a deleted entry.
    until = time.monotonic() + 4
    address = f'http://127.0.0.1:{admin_port}/api/v1/testing/access-log/{trace}'
    while time.monotonic() < until:
        if api_json(client, address, token=token).get('data'):
            break
        time.sleep(.1)
    api_json(client, address, method='DELETE', token=token)
    api_json(client, f'http://127.0.0.1:{mock_port}/ai-long-stream/observation/{trace}', method='DELETE')


def blocked_probe(port, mock_port, admin_port, token, route):
    trace = uuid.uuid4().hex
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    with httpx.Client(timeout=2, trust_env=False, http2=False) as client:
        try:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
            recv_buffer = sock.getsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF)
            sock.settimeout(3)
            sock.connect(('127.0.0.1', port))
            request_body = body(True)
            head = f'POST /v1/chat/completions?payload={FRAME_PAYLOAD}&frames=60000 HTTP/1.1\r\n'
            head += ''.join(f'{key}: {value}\r\n' for key, value in headers(route, trace).items())
            head += f'Content-Length: {len(request_body)}\r\n\r\n'
            sock.sendall(head.encode('ascii') + request_body)
            # Read exactly through final headers; do not consume response data yet.
            received = bytearray()
            while not received.endswith(b'\r\n\r\n'):
                require(len(received) < 16384, "Raw response header cap exceeded")
                value = sock.recv(1)
                require(bool(value), "Raw socket closed before response headers")
                received.extend(value)
            require(received.startswith(b'HTTP/1.1 200 '), "Blocked probe response status failed")
            before, first_plateau = plateau(client, mock_port, trace)
            drained = 0
            until = time.monotonic() + 3
            resumed = None
            while time.monotonic() < until and drained < 16 * 1024 ** 2:
                chunk = sock.recv(65536)
                require(bool(chunk), "Blocked probe completed while resuming")
                drained += len(chunk)
                item = observation(client, mock_port, trace)
                if item['bytes_queued'] > before:
                    resumed = item['bytes_queued']
                    break
            require(resumed is not None and drained > 0, "Producer did not resume after actual socket reads")
            blocked, second_plateau = plateau(client, mock_port, trace)
            sock.close()
            started = time.monotonic()
            terminal = None
            while time.monotonic() - started < 3:
                state = api_json(client, f'http://127.0.0.1:{mock_port}/ai-long-stream/observation/{trace}')
                if state.get('found'):
                    item = state['observation']
                    require(not item['provider_completed'] and not item['deadline_expired'], "Cancellation was natural completion/expiry")
                    if item['upstream_cancelled']:
                        terminal = item
                        break
                time.sleep(.05)
            require(terminal is not None, "Upstream cancellation did not arrive")
            cancellation_ms = (time.monotonic() - started) * 1000
            canonical = ledger(client, admin_port, token, trace, route, True)
            return {"route": route, "receive_buffer_bytes": recv_buffer, "resumed_socket_bytes": drained,
                    "first_plateau": first_plateau, "resumed_producer_bytes": resumed,
                    "second_plateau": second_plateau, "blocked_producer_bytes": blocked,
                    "cancelled_producer_bytes": terminal['bytes_queued'], "cancellation_ms": cancellation_ms,
                    "canonical": canonical}
        finally:
            sock.close()
            clean_trace(client, mock_port, admin_port, token, trace)


async def complete_quality(port, mock_port, admin_port, token, route):
    trace = uuid.uuid4().hex
    try:
        async with httpx.AsyncClient(timeout=10, trust_env=False, http2=False) as client:
            await exchange(client, port, route, True, 1024, 8, expected(True, 1024, 8), trace=trace)
        with httpx.Client(timeout=2, trust_env=False) as client:
            state = api_json(client, f'http://127.0.0.1:{mock_port}/ai-long-stream/observation/{trace}')
            require(state.get('found') and state['observation']['provider_completed'], "Complete producer did not finish")
            return ledger(client, admin_port, token, trace, route, False)
    finally:
        with httpx.Client(timeout=2, trust_env=False) as client:
            clean_trace(client, mock_port, admin_port, token, trace)


def provenance(root, work):
    require((work / 'artifact-profile').read_text().strip() == 'release', "Expected release owned topology")
    bindir = root / 'target/release'
    require((work / 'artifact-bin-dir').read_text().strip() == str(bindir), "Artifact owner directory mismatch")
    pids, binaries = {"driver": os.getpid()}, {}
    for role, name in [('gateway', 'edgion-gateway'), ('fixture', 'test_server'), ('controller', 'edgion-controller')]:
        pid_role = 'test_server' if role == 'fixture' else role
        pid = int((work / f'pids/{pid_role}.pid').read_text())
        require(pid > 1, "Invalid owned PID")
        binary = bindir / name
        command = subprocess.run(['ps', '-p', str(pid), '-o', 'command='], capture_output=True, text=True, timeout=2, check=True).stdout.strip()
        require(command == str(binary) or command.startswith(str(binary) + ' '), "Owned process executable mismatch")
        digest = hashlib.sha256()
        with binary.open('rb') as file:
            for chunk in iter(lambda: file.read(1024 ** 2), b''):
                digest.update(chunk)
        binaries[role] = {"path": str(binary), "sha256": digest.hexdigest(), "pid": pid}
        if role != 'controller':
            pids[role] = pid
    return pids, binaries


async def run(args, artifact):
    root = Path(__file__).resolve().parents[4]
    work = Path(args.work_dir).resolve()
    require(work.parent == (root / 'tmp/integration_testing').resolve() and work.name.startswith('ai-performance.'), "Invalid performance work directory")
    require(httpx.__version__ == '0.28.1', "Performance driver requires HTTPX 0.28.1")
    token = os.environ.get('EDGION_GATEWAY_ADMIN_TOKEN')
    require(bool(token), "Missing gateway testing API credential")
    ports = json.loads(Path(args.ports).read_text())
    port = ports['suites']['EdgionBackend/SingleProvider']['http']
    mock_port, admin_port = 30001, 12001
    pids, binaries = provenance(root, work)
    artifact.update({"python": platform.python_version(), "httpx": httpx.__version__, "platform": platform.platform(),
                     "cpu_count": os.cpu_count(), "load_average_at_start": list(os.getloadavg()), "binaries": binaries, "timed": [], "memory": [], "backpressure": [],
                     "complete_quality": {}, "parameters": {"cohort_seconds": COHORT_SECONDS, "warm_requests": 8, "repeats": TIMED_REPEATS,
                     "concurrency": [1, 8], "timed_payload_bytes": 1024, "timed_frames": 8,
                     "blocked_payload_bytes": FRAME_PAYLOAD, "blocked_frames": 60000,
                     "fixture_max_wire_bytes": 1024 ** 3, "fixture_max_payload_bytes": FRAME_PAYLOAD,
                     "fixture_max_active_streams": 64, "fixture_max_lifetime_seconds": 45,
                     "fixture_max_request_bytes": 1024 ** 2, "fixture_observation_admission_entries": 128,
                     "sampler_interval_seconds": .1, "max_sampler_rows": MAX_SAMPLES, "max_cohort_requests": MAX_REQUESTS,
                     "listener_port": port, "fixture_port": mock_port, "gateway_admin_port": admin_port,
                     "memory_payload_mib": [64, 256], "memory_repeats": 2,
                     "memory_consumer_bytes_per_second": 16 * 1024 ** 2, "transport": "H1/H1", "encoding": "identity"},
                     "limitations": ["Integration mode bypasses production cached metric handles; GenAI/backend identity metrics and raw URI enabled by canonical harness.",
                     "PASS checks experiment integrity and behavioral assertions; no performance or memory release threshold is enforced.",
                     "Ordinary release differs from shipped Cargo production profile; no production capacity or SLA claim.",
                     "Closed-loop single-host driver and fixture may limit throughput; process CPU includes runtime activity.",
                     "RSS is process resident memory, including allocator retention; allocations/live heap are not measured.",
                     "CPU deltas inherit the recorded ps resolution; first body byte is transport latency, not model TTFT.",
                     "Native off retains the attempt lifecycle wrapper with empty observer children."]})
    for streaming in [False, True]:
        for concurrency in [1, 8]:
            for repeat in range(TIMED_REPEATS):
                routes = ['direct', 'plain', 'off', 'provider']
                offset = repeat % len(routes)
                routes = routes[offset:] + routes[:offset]
                if repeat % 2:
                    routes.reverse()
                for route in routes:
                    print(f'{MODE}: timed {route} stream={streaming} c={concurrency} repeat={repeat}', flush=True)
                    row = await timed_cohort(mock_port if route == 'direct' else port, route, streaming, concurrency, pids)
                    row['repeat'] = repeat
                    artifact['timed'].append(row)
    wanted_by_size = {mib: expected(True, FRAME_PAYLOAD, mib * 1024 ** 2 // FRAME_PAYLOAD) for mib in [64, 256]}
    for repeat in range(2):
        for mib in ([64, 256] if repeat == 0 else [256, 64]):
            for route in (['plain', 'off', 'provider'] if repeat == 0 else ['provider', 'off', 'plain']):
                print(f'{MODE}: memory {route} payload={mib}MiB repeat={repeat}', flush=True)
                row = await memory_cohort(port, route, mib, wanted_by_size[mib], pids)
                row['repeat'] = repeat
                artifact['memory'].append(row)
    for route in ['off', 'provider']:
        artifact['complete_quality'][route] = await complete_quality(port, mock_port, admin_port, token, route)
        for repeat in range(2):
            print(f'{MODE}: blocked socket {route} repeat={repeat}', flush=True)
            row = await asyncio.to_thread(blocked_probe, port, mock_port, admin_port, token, route)
            row['repeat'] = repeat
            artifact['backpressure'].append(row)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ports', required=True)
    parser.add_argument('--work-dir', required=True)
    args = parser.parse_args()
    artifact = {"schema_version": 1, "mode": MODE, "status": "FAIL"}
    root = Path(__file__).resolve().parents[4]
    work = Path(args.work_dir).resolve()
    if not (work.is_dir() and work.parent == (root / 'tmp/integration_testing').resolve()
            and work.name.startswith('ai-performance.')):
        print('Invalid owned performance output directory', file=sys.stderr)
        return 2
    output = work / 'ai-native-performance.json'
    try:
        asyncio.run(run(args, artifact))
        artifact['status'] = 'PASS'
        return_code = 0
    except Exception as error:
        # Preserve only type/fixed stage facts, not HTTP errors carrying headers or bodies.
        artifact['failure_type'] = type(error).__name__
        if isinstance(error, MeasurementFailure):
            artifact['failure_reason'] = str(error)
            print(str(error), file=sys.stderr)
        print(f'{MODE}: FAIL ({type(error).__name__})', file=sys.stderr)
        return_code = 1
    output.write_text(json.dumps(artifact, indent=2, allow_nan=False) + '\n')
    print(f'Performance artifact: {output}', flush=True)
    return return_code


if __name__ == '__main__':
    sys.exit(main())
