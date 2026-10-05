#!/usr/bin/env python3
"""Real-etcd policy assertions against the existing runner's Edgion processes."""
import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request


def wait(label, predicate, timeout=45):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            last = predicate()
            if last:
                print(f"PASS {label}", flush=True)
                return last
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            last = type(error).__name__
        time.sleep(0.2)
    raise AssertionError(f"Deadline exceeded: {label}; last={last!r}")


class Suite:
    def __init__(self, args):
        self.work = Path(args.work_dir)
        self.root = Path(__file__).resolve().parents[5]
        self.compose = ["docker", "compose", "-p", os.environ["ETCD_POLICY_PROJECT"], "-f",
                        str(self.root / "edgion-tests/integration/conf/Services/etcd/docker-compose-policy.yaml")]
        self.cli = str(self.root / "target/debug/edgion-cli")
        self.gateway = args.gateway
        self.controller = args.controller
        self.links = []
        self.secrets = []
        self.serial = 0
        self.http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        self.ports = [os.environ[f"ETCD_POLICY_{name}_PORT"] for name in "ABC"]

    def command(self, args):
        return subprocess.check_output(args, stderr=subprocess.PIPE, timeout=30).decode()

    def ctl(self, *args, member="b", authenticated=True):
        auth = ["--user=root:policy-test-password"] if authenticated else []
        return self.command(self.compose + ["exec", "-T", f"member-{member}", "etcdctl",
            "--endpoints=https://127.0.0.1:2379", "--cacert=/certs/ca.crt",
            "--cert=/certs/client.crt", "--key=/certs/client.key"] + auth + list(args))

    def api(self, path, body=None):
        request = urllib.request.Request(self.gateway + path,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Authorization": "Bearer " + os.environ["EDGION_GATEWAY_ADMIN_TOKEN"],
                     "Content-Type": "application/json"})
        try:
            with self.http.open(request, timeout=12) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            return json.load(error)

    def op(self, name, operation, body=None):
        return self.api(f"/api/v1/testing/link-sys/etcd/default_{name}/{operation}", body)

    def apply(self, kind, name, content):
        self.serial += 1
        resource = {"apiVersion": "v1" if kind == "Secret" else "edgion.io/v1", "kind": kind,
                    "metadata": {"namespace": "default", "name": name}, **content}
        fixture = self.work / f"policy-{self.serial}.json"
        fixture.write_text(json.dumps(resource))
        self.command([self.cli, "--server", self.controller, "apply", "-f", str(fixture)])
        collection = self.links if kind == "LinkSys" else self.secrets
        if name not in collection:
            collection.append(name)

    def link(self, name, config):
        self.apply("LinkSys", name, {"spec": {"type": "etcd", "config": config}})

    def ready(self, name):
        return wait(f"{name} admitted", lambda: self.op(name, "ping").get("success"))

    def config(self, member=0, mtls=True):
        tls = {"enabled": True, "validation": {"caCertificateRefs": [{"kind": "Secret", "name": "etcd-policy-ca"}]}}
        if mtls:
            tls["clientCertificateRef"] = {"name": "etcd-policy-client"}
        return {"endpoints": [f"https://127.0.0.1:{self.ports[member]}"], "tls": tls,
                "auth": {"secretRef": {"name": "etcd-policy-auth"}},
                "timeout": {"dial": 1000, "request": 2000},
                "keepAlive": {"time": 2, "timeout": 1},
                "maxCallSendSize": 16384, "maxCallRecvSize": 16384,
                "userAgent": "edgion-policy-integration", "rejectOldCluster": True,
                "minClusterVersion": "3.5.0",
                "observability": {"metrics": {"enabled": True},
                                  "logging": {"enabled": True, "level": "info", "slowLogThreshold": 1}}}

    def rejected(self, name, config):
        self.link(name, config)
        # Wait for an actual completed connection attempt, not mere absence
        # before the resource has reached the Gateway.
        log = self.work / "logs/gateway.log"
        wait(f"{name} attempted and rejected", lambda: any(
            f"default/{name}" in line and "Etcd maintenance" in line
            and "init" in line and "failed" in line
            for line in log.read_text(errors="replace").splitlines()))
        assert not self.op(name, "ping").get("success"), name
        health = self.op(name, "health")
        assert not (health.get("data") or {}).get("connected", False), health

    def metrics(self, name, outcome):
        with self.http.open("http://127.0.0.1:12090/metrics", timeout=5) as response:
            text = response.read().decode()
        return sum(float(line.rsplit(" ", 1)[1]) for line in text.splitlines()
                   if line.startswith("edgion_link_sys_etcd_operations_total{")
                   and f'link="default/{name}"' in line and 'operation="kv"' in line
                   and f'outcome="{outcome}"' in line)

    def setup(self):
        wait("three etcd members healthy", lambda: all(
            self.ctl("endpoint", "health", member=name, authenticated=False) is not None for name in "abc"))
        self.ctl("user", "add", "root:policy-test-password", authenticated=False)
        self.ctl("user", "grant-role", "root", "root", authenticated=False)
        self.ctl("auth", "enable", authenticated=False)
        wait("authenticated cluster healthy", lambda: self.ctl("endpoint", "health") is not None)

    def run(self):
        certs = self.work / "certs/grpc"
        def secret(name, data, kind="Opaque"):
            self.apply("Secret", name, {"type": kind, "data": {
                key: base64.b64encode(value).decode() for key, value in data.items()}})
        secret("etcd-policy-ca", {"ca.crt": (certs / "primary/ca.crt").read_bytes()})
        secret("etcd-policy-wrong-ca", {"ca.crt": (certs / "secondary/ca.crt").read_bytes()})
        secret("etcd-policy-client", {"tls.crt": (certs / "primary/client.crt").read_bytes(),
                                      "tls.key": (certs / "primary/client.key").read_bytes()}, "kubernetes.io/tls")
        secret("etcd-policy-auth", {"username": b"root", "password": b"policy-test-password"})
        secret("etcd-policy-wrong-auth", {"username": b"root", "password": b"wrong-password"})
        self.link("etcd-policy-tls", self.config(mtls=False))
        self.ready("etcd-policy-tls")
        self.link("etcd-policy-mtls", self.config(member=1))
        self.ready("etcd-policy-mtls")
        wrong = self.config()
        wrong["auth"]["secretRef"]["name"] = "etcd-policy-wrong-auth"
        self.rejected("etcd-policy-bad-auth", wrong)
        wrong = self.config()
        wrong["tls"]["validation"]["caCertificateRefs"][0]["name"] = "etcd-policy-wrong-ca"
        self.rejected("etcd-policy-bad-ca", wrong)
        self.rejected("etcd-policy-missing-cert", self.config(member=1, mtls=False))
        wrong = self.config()
        wrong["minClusterVersion"] = "3.99.0"
        self.rejected("etcd-policy-version", wrong)
        self.link("etcd-policy-version", self.config())
        self.ready("etcd-policy-version")

        name = "etcd-policy-limits"
        config = self.config()
        self.link(name, config)
        self.ready(name)
        big = "v" * 2048
        key = "policy-limit-value"
        assert self.op(name, "put", {"key": key, "value": big})["success"]
        config["maxCallSendSize"] = 512
        config["maxCallRecvSize"] = 512
        self.link(name, config)
        rejected_key = "policy-rejected-value"
        # First wait until the new generation's receive limit is effective;
        # this avoids intentionally writing the rejected key on the old client.
        wait("hot reload receive cap lowered", lambda: "limit exceeded" in (self.op(name, "get/" + key).get("error") or ""))
        self.ready(name)
        before_send = self.metrics(name, "send_limit")
        response = self.op(name, "put", {"key": rejected_key, "value": big})
        assert not response["success"], response
        assert self.metrics(name, "send_limit") == before_send + 1
        stored = json.loads(self.ctl("get", rejected_key, "-w", "json"))
        assert not stored.get("kvs"), "oversized rejected PUT reached etcd"
        assert self.op(name, "put", {"key": "policy-small", "value": "ok"})["success"]
        before_recv = self.metrics(name, "recv_limit")
        assert not self.op(name, "get/" + key)["success"]
        assert self.metrics(name, "recv_limit") == before_recv + 1
        assert self.op(name, "get/policy-small")["data"]["value"] == "ok"
        config["maxCallSendSize"] = config["maxCallRecvSize"] = 16384
        self.link(name, config)
        wait("hot reload receive cap restored", lambda: (self.op(name, "get/" + key).get("data") or {}).get("value") == big)
        assert self.op(name, "put", {"key": rejected_key, "value": big})["success"]
        print("PASS exact send/receive metric deltas and rejected write absence", flush=True)

        name = "etcd-policy-discovery"
        config = self.config()
        config["autoSyncInterval"] = 1
        self.link(name, config)
        def coverage(count):
            data = self.op(name, "health").get("data") or {}
            return data.get("configured_endpoints") == count and data.get("live_endpoints") == count
        wait("discovery expands one seed to three members", lambda: coverage(3))
        members = json.loads(self.ctl("member", "list", "-w", "json"))["members"]
        seed = next(member for member in members if member["name"] == "member-a")
        self.ctl("member", "remove", format(seed["ID"], "x"))
        self.command(self.compose + ["stop", "member-a"])
        wait("discovery removes seed", lambda: coverage(2))
        self.command(self.compose + ["restart", "member-b", "member-c"])
        wait("survivor quorum restored", lambda: self.ctl("endpoint", "health") is not None)
        wait("reconnect uses discovered members with seed absent", lambda: self.op(name, "put", {"key": "policy-after-restart", "value": "alive"}).get("success"))
        assert self.op(name, "get/policy-after-restart")["data"]["value"] == "alive"
        wait("latest two-member coverage restored", lambda: coverage(2))
        print("PASS real etcd policy suite", flush=True)

    def cleanup(self):
        for kind, names in [("LinkSys", self.links), ("Secret", self.secrets)]:
            for name in names:
                try:
                    self.command([self.cli, "--server", self.controller, "delete", kind, name, "-n", "default"])
                except (OSError, subprocess.CalledProcessError):
                    pass


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["setup", "run"])
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--gateway", default="http://127.0.0.1:12001")
    parser.add_argument("--controller", default="http://127.0.0.1:12101")
    args = parser.parse_args()
    suite = Suite(args)
    if args.mode == "setup":
        suite.setup()
    else:
        try:
            suite.run()
        finally:
            suite.cleanup()
