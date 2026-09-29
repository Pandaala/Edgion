#!/usr/bin/env python3
"""Manual, isolated Edgion shutdown-notifier A/B load measurement.

Each sample owns a Controller/Gateway pair because the notifier variant and
worker configuration are fixed at process startup. Only owned PIDs are stopped.
Build both binaries from the same snapshot, changing only the isolated fork's
shard count to one for the control. This driver cannot verify build provenance;
retain the source digests, patch and Cargo metadata alongside its JSON output.
Requires h2load and the Python h2 dependency of the shared process fixture.
"""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile

from test_http_process_shutdown import ROOT, Topology, prepare_certificates, require


def percentile(values, fraction):
    return values[max(0, math.ceil(len(values) * fraction) - 1)]


def run_sample(root, controller, gateway, variant, index, duration, clients):
    label = f"{index:02d}-{variant}"
    topology = Topology(root, label, {"controller": controller, "gateway": gateway}, root / "certs")
    try:
        topology.prepare()
        topology.start_backend()
        topology.start_controller()
        topology.start_gateway()
        topology.wait_gateway_ready()
        request_log = topology.work / "requests.tsv"
        command = [
            "h2load", "--h1", "-t", "4", "-c", str(clients), "-m", "1",
            "-D", str(duration), "--warm-up-time=1", f"--log-file={request_log}",
            f"http://127.0.0.1:{topology.ports['http_a']}/load",
        ]
        measured = subprocess.run(command, capture_output=True, text=True, timeout=duration + 30)
        (topology.work / "h2load.txt").write_text(measured.stdout + measured.stderr)
        require(measured.returncode == 0, f"h2load failed; see {topology.work}")
        counts = re.search(r"(\d+) succeeded, (\d+) failed, (\d+) errored, (\d+) timeout", measured.stdout)
        require(counts is not None, "Missing h2load completion counts")
        succeeded, failed, errored, timed_out = map(int, counts.groups())
        require(succeeded > 0 and failed == errored == timed_out == 0, f"Incomplete load requests: {counts.group(0)}")
        rate = re.search(r"finished in .*?, ([0-9.]+) req/s", measured.stdout)
        require(rate is not None, "Missing h2load throughput")
        latencies = []
        for line in request_log.read_text().splitlines():
            fields = line.split()
            require(len(fields) >= 3 and fields[1] == "200", f"Unexpected load response: {line}")
            latencies.append(int(fields[2]))
        latencies.sort()
        require(len(latencies) == succeeded, "Per-request records do not match completed request count")
        topology.backend.check()
        topology.gateway.send_signal(signal.SIGTERM)
        topology.wait_exit(True, timeout=20)
        return {
            "variant": variant, "sample": index, "requests": succeeded,
            "requests_per_second": float(rate.group(1)),
            "p50_us": percentile(latencies, 0.50),
            "p95_us": percentile(latencies, 0.95),
            "p99_us": percentile(latencies, 0.99),
            "command": command, "artifacts": str(topology.work),
        }
    finally:
        topology.cleanup()


def main():
    def interrupted(signum, _frame):
        raise KeyboardInterrupt(f"received signal {signum}")

    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--controller-bin", type=Path, required=True)
    parser.add_argument("--sharded-bin", type=Path, required=True)
    parser.add_argument("--single-shard-bin", type=Path, required=True)
    parser.add_argument("--profile", required=True, help="Actual shared Cargo build profile")
    parser.add_argument("--duration", type=int, default=5)
    parser.add_argument("--clients", type=int, default=64)
    parser.add_argument("--work-root", type=Path, default=ROOT / "tmp/integration_testing")
    args = parser.parse_args()
    require(args.duration > 0 and args.clients >= 4, "Positive duration and at least four clients required")
    args.work_root.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="http-shutdown-benchmark.", dir=args.work_root)).resolve()
    binaries = {"sharded": args.sharded_bin.resolve(), "single-shard": args.single_shard_bin.resolve()}
    controller = args.controller_bin.resolve()
    for binary in [controller, *binaries.values()]:
        require(binary.is_file() and os.access(binary, os.X_OK), f"Missing executable: {binary}")
    env = {key: value for key, value in os.environ.items() if not key.startswith("EDGION_")}
    prepare_certificates(root, env)
    report = {
        "profile": args.profile, "gateway_workers_per_listener": 4,
        "work_stealing": True, "listeners": 2, "loaded_listeners": 1,
        "load_threads": 4, "clients": args.clients, "duration_seconds": args.duration,
        "warmup_seconds": 1, "backend": "shared Python HTTP/1 fixture; possible throughput bottleneck",
        "contention": "not directly profiled; see separate notification microbenchmark",
        "binary_sha256": {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in binaries.items()},
        "h2load_version": subprocess.check_output(["h2load", "--version"], text=True).strip(),
        "samples": [],
    }
    print(f"Artifacts: {root}", flush=True)
    for index, variant in enumerate(("sharded", "single-shard", "single-shard", "sharded", "sharded", "single-shard"), 1):
        sample = run_sample(root, controller, binaries[variant], variant, index, args.duration, args.clients)
        report["samples"].append(sample)
        (root / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(sample), flush=True)


if __name__ == "__main__":
    main()
