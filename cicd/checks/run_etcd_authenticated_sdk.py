#!/usr/bin/env python3
"""Run the authenticated SDK regression with one exclusively owned Etcd fixture."""

import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import uuid


IMAGE = (
    "gcr.io/etcd-development/etcd:v3.7.2@sha256:"
    "7c6c239825d00e3f6328a69caafd54be92063acf0c2ce78b8394699f52b75dc3"
)
ROOT = Path(__file__).resolve().parents[2]


def main():
    # Normal CI termination must unwind the fixture's finally block as well.
    signal.signal(signal.SIGTERM, lambda signum, frame: sys.exit(128 + signum))
    name = "edgion-sdk-auth-" + uuid.uuid4().hex[:12]
    started = False

    def ctl(arguments):
        return subprocess.run(
            ["docker", "exec", name, "/usr/local/bin/etcdctl",
             "--endpoints=http://127.0.0.1:2379", *arguments],
            capture_output=True, text=True, timeout=5,
        )

    try:
        print("Starting a dedicated authenticated Etcd 3.7.2 fixture", flush=True)
        subprocess.run(
            ["docker", "run", "-d", "--name", name,
             "--label", "edgion.fixture=authenticated-sdk", "-p", "127.0.0.1::2379",
             IMAGE, "/usr/local/bin/etcd", "--name=auth-fixture",
             "--data-dir=/tmp/etcd-data",
             "--listen-client-urls=http://0.0.0.0:2379",
             "--advertise-client-urls=http://127.0.0.1:2379",
             "--listen-peer-urls=http://0.0.0.0:2380",
             "--initial-advertise-peer-urls=http://127.0.0.1:2380",
             "--initial-cluster=auth-fixture=http://127.0.0.1:2380",
             "--initial-cluster-state=new"],
            check=True, stdout=subprocess.DEVNULL, timeout=180,
        )
        started = True
        port = subprocess.check_output(
            ["docker", "port", name, "2379/tcp"], text=True, timeout=10,
        ).strip().rsplit(":", 1)[1]
        deadline = time.monotonic() + 30
        while ctl(["endpoint", "health"]).returncode != 0:
            if time.monotonic() >= deadline:
                raise RuntimeError("dedicated Etcd fixture did not become healthy")
            time.sleep(0.2)
        for args in [
            ["user", "add", "root:integration-etcd-password"],
            ["user", "grant-role", "root", "root"],
            ["auth", "enable"],
        ]:
            result = ctl(args)
            if result.returncode:
                raise RuntimeError("fixture authentication setup failed: " + result.stderr)
        if ctl(["get", "/authentication-must-be-enabled"]).returncode == 0:
            raise RuntimeError("dedicated fixture unexpectedly accepted an unauthenticated read")
        env = os.environ.copy()
        env["EDGION_ETCD_AUTH_ENDPOINT"] = "http://127.0.0.1:" + port
        subprocess.run(
            ["cargo", "test", "--locked", "-p", "edgion-common", "--test",
             "etcd_authenticated_lease", "--", "--ignored", "--nocapture"],
            cwd=ROOT, env=env, check=True, timeout=600,
        )
    finally:
        cleanup = subprocess.run(
            ["docker", "rm", "-fv", name], stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, timeout=15,
        )
        if started and cleanup.returncode:
            raise RuntimeError("failed to remove owned Etcd fixture " + name)


if __name__ == "__main__":
    main()
