#!/usr/bin/env python3
"""Standalone topology exception: control Controller startup and real API WATCH faults.

Uses one owned local Controller and the selected real Kubernetes API, without the
shared Gateway topology. Verifies raw caches and conf-sync publication; does not
claim HTTP forwarding coverage. Never fabricates LIST bodies or resource
ADDED/MODIFIED/DELETED events; injects only protocol ERROR Status events.
"""

import argparse
from datetime import datetime, timezone
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid


ROOT = Path(__file__).resolve().parents[3]
LABEL = "edgion.io/namespace-watch-test"
INIT_METRIC = "edgion_controller_init_sync_duration_seconds_last"
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def run(command, **kwargs):
    return subprocess.run(command, check=True, text=True, capture_output=True,
                          timeout=kwargs.pop("timeout", 60), **kwargs).stdout


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(predicate, message, timeout=30):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (OSError, ValueError, urllib.error.HTTPError) as error:
            last = type(error).__name__
        time.sleep(0.05)
    raise AssertionError(f"Timed out: {message}; last error type={last}")


def close_socket(sock):
    if sock:
        try:
            sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        sock.close()


class Faults:
    def __init__(self, ns_a, ns_b, selector):
        self.a, self.b, self.selector = ns_a, ns_b, selector
        self.lock = threading.Lock()
        self.gates = {key: threading.Event() for key in ("b_initial", "namespace", "a_second", "a_runtime")}
        self.seen = {key: threading.Event() for key in self.gates}
        self.counts = {}
        self.timeline = []
        self.active = set()
        self.connections = set()
        self.runtime_fault = False
        self.runtime_list = False
        self.stopping = threading.Event()

    def choose(self, path):
        parsed = urllib.parse.urlsplit(path)
        query = urllib.parse.parse_qs(parsed.query)
        watch = query.get("watch", [""])[0].lower() in ("true", "1")
        match = re.fullmatch(r"/api/v1/namespaces/([^/]+)/services", parsed.path)
        source = match.group(1) if match and query.get("labelSelector") == [self.selector] else None
        gate, fault = None, False
        with self.lock:
            if source:
                key = (source, "watch" if watch else "list")
                self.counts[key] = self.counts.get(key, 0) + 1
                count = self.counts[key]
                if source == self.a and watch and count == 1:
                    fault = True
                elif source == self.a and watch and self.runtime_fault:
                    self.runtime_fault = False
                    self.runtime_list = True
                    fault = True
                elif source == self.a and not watch and count == 2:
                    gate = "a_second"
                elif source == self.a and not watch and self.runtime_list:
                    self.runtime_list = False
                    gate = "a_runtime"
                elif source == self.b and not watch and count == 1:
                    gate = "b_initial"
                self.timeline.append({"time": time.monotonic(), "path": parsed.path,
                                      "source": source, "operation": key[1], "count": count,
                                      "error_event_code": 410 if fault else None, "gate": gate})
            elif parsed.path == "/api/v1/namespaces" and not watch and not self.seen["namespace"].is_set():
                gate = "namespace"
            if gate:
                self.seen[gate].set()
        return source, watch, gate, fault

    def interrupt_a(self):
        with self.lock:
            assert self.active, "No active source-A WATCH to interrupt"
            self.runtime_fault = True
            sockets = list(self.active)
        for downstream, upstream in sockets:
            close_socket(downstream)
            close_socket(upstream)

    def stop(self):
        self.stopping.set()
        for gate in self.gates.values():
            gate.set()
        with self.lock:
            sockets = list(self.connections)
        for downstream, upstream in sockets:
            close_socket(downstream)
            close_socket(upstream)


class Proxy(http.server.ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = False

    def __init__(self, upstream_port, faults):
        super().__init__(("127.0.0.1", 0), ProxyHandler)
        self.upstream_port, self.faults = upstream_port, faults


class ProxyHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass  # Never log request headers, tokens, or resource bodies.

    def forward(self):
        state = self.server.faults
        upstream = None
        pair = None
        try:
            source, watch, gate, fault = state.choose(self.path)
            if gate:
                state.gates[gate].wait(60)
                if not state.gates[gate].is_set() or state.stopping.is_set():
                    return
            if fault:
                status = {"apiVersion": "v1", "kind": "Status", "status": "Failure",
                                      "reason": "Expired", "message": "Test WATCH resource version expired",
                                      "code": 410}
                # kube-runtime 2.0.1 relists on a WATCH ERROR envelope. A bare
                # HTTP 410 handshake failure only reconnects WATCH in this version.
                payload = (json.dumps({"type": "ERROR", "object": status}) + "\n").encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(payload)
                self.wfile.flush()
                self.close_connection = True
                return
            body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            headers = {key: value for key, value in self.headers.items()
                       if key.lower() not in {"host", "connection", "transfer-encoding", "accept-encoding"}}
            headers["Host"] = f"127.0.0.1:{self.server.upstream_port}"
            headers["Accept-Encoding"] = "identity"
            upstream = http.client.HTTPConnection("127.0.0.1", self.server.upstream_port, timeout=120)
            upstream.request(self.command, self.path, body=body, headers=headers)
            pair = (self.connection, upstream.sock)
            with state.lock:
                state.connections.add(pair)
            response = upstream.getresponse()
            if source == state.a and watch and response.status == 200:
                with state.lock:
                    state.active.add(pair)
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() not in {"connection", "transfer-encoding", "content-length"}:
                    self.send_header(key, value)
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True
            while not state.stopping.is_set():
                chunk = response.read1(65536)
                if not chunk:
                    break
                self.wfile.write(chunk)
                self.wfile.flush()
        except (OSError, http.client.HTTPException):
            self.close_connection = True
        finally:
            if pair:
                with state.lock:
                    state.active.discard(pair)
                    state.connections.discard(pair)
            if upstream:
                upstream.close()

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = forward


class Scenario:
    def __init__(self, args, mode, work):
        self.args, self.mode, self.work = args, mode, work
        self.run_id = uuid.uuid4().hex[:16]
        self.names = [f"edgion-nw-{self.run_id}-{suffix}" for suffix in ("a", "b", "c")]
        self.owned = {}
        self.pod_uid = None
        self.processes = []
        self.logs = []
        self.proxy = None
        self.proxy_thread = None
        self.monitor_stop = threading.Event()
        self.monitor_thread = None
        self.monitor_errors = []
        self.monitor_samples = 0
        self.token = secrets.token_urlsafe(32)
        self.result = {"ha_mode": mode, "namespaces": self.names, "checks": [], "cleanup": []}
        self.admin, self.probe, self.metrics, self.grpc = [port() for _ in range(4)]
        self.faults = Faults(*self.names[:2], f"{LABEL}={self.run_id}")

    def kubectl(self, *args, obj=None):
        return run(["kubectl", "--context", self.args.context, *args],
                   input=None if obj is None else json.dumps(obj))

    def create(self, obj):
        return json.loads(self.kubectl("create", "-f", "-", "-o", "json", obj=obj))

    def start(self, command, filename, env=None):
        logfile = (self.work / filename).open("w")
        self.logs.append(logfile)
        proc = subprocess.Popen(command, stdout=logfile, stderr=subprocess.STDOUT, env=env, cwd=ROOT)
        self.processes.append(proc)
        return proc

    def http(self, port_number, path, json_body=True):
        request = urllib.request.Request(f"http://127.0.0.1:{port_number}{path}",
                                         headers={"Authorization": f"Bearer {self.token}"})
        with HTTP.open(request, timeout=3) as response:
            body = response.read().decode()
            return json.loads(body) if json_body else body

    def ready_status(self):
        try:
            self.http(self.probe, "/ready", False)
            return 200
        except urllib.error.HTTPError as error:
            return error.code

    def raw_keys(self):
        return set(self.http(self.admin, "/api/v1/debug/store-stats")["data"]["raw_cache_keys"]["Service"])

    def services(self):
        objects = self.http(self.admin, "/configserver/Service/list")["data"]
        return {f'{obj["metadata"]["namespace"]}/{obj["metadata"]["name"]}': obj for obj in objects}

    def signature(self):
        server_id = self.http(self.admin, "/api/v1/server-info")["data"]["server_id"]
        metrics = self.http(self.metrics, "/metrics", False)
        values = [line for line in metrics.splitlines()
                  if line.startswith(INIT_METRIC + "{") and 'kind="Service"' in line]
        assert server_id and len(values) == 1, "Missing server identity or Service init metric"
        return {"server_id": server_id, "init_metric": values[0]}

    def start_monitor(self, signature):
        self.monitor_stop.clear()
        def observe():
            while not self.monitor_stop.is_set():
                try:
                    assert self.ready_status() == 200, "Readiness regressed"
                    assert self.signature() == signature, "Publication or initial sync restarted"
                    self.monitor_samples += 1
                except Exception as error:
                    if not self.monitor_stop.is_set():
                        self.monitor_errors.append(f"{type(error).__name__}: {error}")
                self.monitor_stop.wait(0.1)
        self.monitor_thread = threading.Thread(target=observe, daemon=True)
        self.monitor_thread.start()

    def stop_monitor(self):
        self.monitor_stop.set()
        if self.monitor_thread:
            self.monitor_thread.join(timeout=10)
            assert not self.monitor_thread.is_alive(), "Readiness monitor did not stop"
            self.monitor_thread = None
        self.result["availability_samples"] = self.monitor_samples
        self.result["availability_errors"] = list(self.monitor_errors)

    def pod_is_leader(self, expected):
        pod = json.loads(self.kubectl("get", "pod", "controller-identity", "-n", self.names[0], "-o", "json"))
        assert pod["metadata"]["uid"] == self.pod_uid
        return pod["metadata"].get("labels", {}).get("edgion.io/leader") == str(expected).lower()

    def replace_lease_holder(self, holder, lease_uid):
        namespace = json.loads(self.kubectl("get", "namespace", self.names[0], "-o", "json"))
        assert namespace["metadata"]["uid"] == self.owned[self.names[0]]
        assert namespace["metadata"]["labels"].get(LABEL) == self.run_id
        for attempt in range(5):
            lease = json.loads(self.kubectl("get", "lease", "namespace-watch-leader", "-n", self.names[0], "-o", "json"))
            assert lease["metadata"]["uid"] == lease_uid, "Lease identity changed; refusing replacement"
            lease["spec"]["holderIdentity"] = holder
            lease["spec"]["renewTime"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ") if holder else None
            lease["spec"]["leaseDurationSeconds"] = 60
            try:
                self.kubectl("replace", "-f", "-", obj=lease)
                return
            except subprocess.CalledProcessError as error:
                if "Conflict" not in error.stderr or attempt == 4:
                    raise
        raise AssertionError("Lease replacement conflict retry exhausted")

    def reload_and_leadership(self, expected, signature):
        self.stop_monitor()
        assert not self.monitor_errors, self.monitor_errors
        request = urllib.request.Request(f"http://127.0.0.1:{self.admin}/api/v1/reload", method="POST", data=b"",
                                         headers={"Authorization": f"Bearer {self.token}"})
        with HTTP.open(request, timeout=5) as response:
            assert response.status == 200
        def reloaded():
            if self.ready_status() != 200:
                return False
            new = self.signature()
            return new if new["server_id"] != signature["server_id"] else False
        new_signature = wait_for(reloaded, "administrative reload republishes server", 30)
        self.checkpoint("administrative-reload-converges", expected, new_signature)
        self.start_monitor(new_signature)
        wait_for(lambda: self.pod_is_leader(True), "real Lease acquired after reload", 20)
        lease = json.loads(self.kubectl("get", "lease", "namespace-watch-leader", "-n", self.names[0], "-o", "json"))
        lease_uid = lease["metadata"]["uid"]
        if self.mode == "leader-only":
            self.stop_monitor()  # This mode intentionally stops serving while not leader.
        self.replace_lease_holder(f"foreign-{self.run_id}", lease_uid)
        wait_for(lambda: self.pod_is_leader(False), "observed real leadership loss", 25)
        if self.mode == "leader-only":
            wait_for(lambda: self.ready_status() == 503, "leader-only stops serving", 10)
            self.result["checks"].append({"name": "leader-only-loss-not-ready", "ready": 503})
        else:
            b = self.names[1]
            self.kubectl("patch", "service", "stable", "-n", b, "--type=merge", "-p",
                         json.dumps({"spec": {"ports": [{"name": "http", "port": 18087, "targetPort": 18087}]}}))
            expected[f"{b}/stable"] = (expected[f"{b}/stable"][0], 18087)
            self.checkpoint("all-serve-loss-keeps-watching", expected, new_signature)
        self.replace_lease_holder("", lease_uid)
        wait_for(lambda: self.pod_is_leader(True) and self.ready_status() == 200, "real leadership reacquired", 30)
        final_signature = self.signature()
        if self.mode == "leader-only":
            assert final_signature["server_id"] != new_signature["server_id"], "Leader-only failed to rebuild serving epoch"
            self.start_monitor(final_signature)
        else:
            assert final_signature == new_signature, "All-serve leadership transition rebuilt serving epoch"
        self.checkpoint("leadership-reacquire-converges", expected, final_signature)

    def checkpoint(self, name, expected, signature=None):
        def converged():
            assert self.ready_status() == 200, "Readiness regressed"
            if signature:
                assert self.signature() == signature, "Controller publication or initial sync restarted"
            actual = self.services()
            if set(actual) != set(expected) or self.raw_keys() != set(expected):
                return False
            for key, (uid, number) in expected.items():
                if actual[key]["metadata"]["uid"] != uid or actual[key]["spec"]["ports"][0]["port"] != number:
                    return False
            return actual
        actual = wait_for(converged, name)
        self.result["checks"].append({"name": name, "raw_keys": sorted(self.raw_keys()),
                                      "processed": actual, "signature": self.signature()})

    def fixture(self, namespace, name, number):
        return self.create({"apiVersion": "v1", "kind": "Service", "metadata": {
            "namespace": namespace, "name": name, "labels": {LABEL: self.run_id}},
            "spec": {"ports": [{"name": "http", "port": number, "targetPort": number}]}})

    def execute(self):
        self.work.mkdir(parents=True)
        helper = ROOT / "edgion-tests/integration/scripts/utils/conf_sync_certs.sh"
        run(["bash", "-c", 'source "$1"; conf_sync_distribute_certs "$2"', "bash", str(helper), str(self.work)], timeout=120)
        security = run(["bash", "-c", 'source "$1"; conf_sync_security_block server', "bash", str(helper)])
        for name in self.names:
            ns = self.create({"apiVersion": "v1", "kind": "Namespace", "metadata": {
                "name": name, "labels": {LABEL: self.run_id}}})
            self.owned[name] = ns["metadata"]["uid"]
        a, b, c = self.names
        expected = {}
        for ns, name, number in [(a, "same-key", 18080), (a, "old-only", 18081), (b, "stable", 18082)]:
            obj = self.fixture(ns, name, number)
            expected[f"{ns}/{name}"] = (obj["metadata"]["uid"], number)
        self.fixture(c, "excluded", 18083)
        pod = self.create({"apiVersion": "v1", "kind": "Pod", "metadata": {
            "namespace": a, "name": "controller-identity", "labels": {LABEL: self.run_id}},
            "spec": {"nodeName": f"absent-{self.run_id}", "automountServiceAccountToken": False,
                     "containers": [{"name": "placeholder", "image": "registry.k8s.io/pause:3.10"}]}})
        self.pod_uid = pod["metadata"]["uid"]
        upstream_port = port()
        self.start(["kubectl", "--context", self.args.context, "proxy", "--address=127.0.0.1",
                    f"--port={upstream_port}"], "kubectl-proxy.log")
        def proxy_ready():
            with HTTP.open(f"http://127.0.0.1:{upstream_port}/version", timeout=2) as response:
                return response.status == 200
        wait_for(proxy_ready, "kubectl proxy ready")
        self.proxy = Proxy(upstream_port, self.faults)
        self.proxy_thread = threading.Thread(target=self.proxy.serve_forever, daemon=True)
        self.proxy_thread.start()
        kubeconfig = {"apiVersion": "v1", "kind": "Config", "current-context": "test",
                      "clusters": [{"name": "test", "cluster": {"server": f"http://127.0.0.1:{self.proxy.server_port}"}}],
                      "contexts": [{"name": "test", "context": {"cluster": "test", "user": "test"}}],
                      "users": [{"name": "test", "user": {}}]}
        (self.work / "kubeconfig.json").write_text(json.dumps(kubeconfig))
        definitions = (ROOT / "edgion-resources/src/resource/defs.rs").read_text()
        kinds = re.findall(r"^    ([A-Z][A-Za-z0-9]+) \{$", definitions, re.MULTILINE)
        assert "Service" in kinds and "Gateway" in kinds
        config = {"work_dir": str(self.work), "server": {
            f"{name}_listen": f"127.0.0.1:{number}" for name, number in
            [("admin", self.admin), ("probe", self.probe), ("metrics", self.metrics), ("grpc", self.grpc)]},
            "logging": {"log_dir": "logs", "log_level": "info", "console": True},
            "conf_center": {"type": "kubernetes", "gateway_class": "namespace-watch-test",
                "controller_name": f"edgion.io/namespace-watch/{self.run_id}",
                "watch_namespaces": [a, b, a], "label_selector": self.faults.selector,
                "ha_mode": self.mode, "no_watch_kinds": [kind for kind in kinds if kind != "Service"],
                "leader_election": {"lease_namespace": a, "lease_name": "namespace-watch-leader"}}}
        config_path = self.work / "controller.yaml"
        config_path.write_text("".join(f"{key}: {json.dumps(value)}\n" for key, value in config.items())
                               + security + "  auto_ca: false\n")
        env = {key: value for key, value in os.environ.items() if key.lower() not in
               {"http_proxy", "https_proxy", "all_proxy"}}
        env.update(KUBECONFIG=str(self.work / "kubeconfig.json"), EDGION_ADMIN_TOKEN=self.token,
                   POD_NAME="controller-identity", POD_NAMESPACE=a)
        self.start([str(self.args.binary), "--config-file", str(config_path), "--work-dir", str(self.work),
                    "--test-mode"], "controller.log", env)
        wait_for(lambda: all(self.faults.seen[key].is_set() for key in
                            ("a_second", "b_initial", "namespace")), "initial LIST interleaving", 10)
        self.faults.gates["b_initial"].set()
        wait_for(lambda: self.raw_keys() == set(expected), "complete initial raw union", 3)
        assert self.ready_status() == 503, "Namespace initial gate did not block readiness"
        self.result["checks"].append({"name": "namespace-gates-ready-with-gateway-disabled", "ready": 503,
                                      "raw_keys": sorted(self.raw_keys())})
        self.faults.gates["namespace"].set()
        wait_for(lambda: self.ready_status() == 200, "first publication", 5)
        self.checkpoint("first-complete-union", expected)
        signature = self.signature()
        self.start_monitor(signature)
        self.kubectl("patch", "service", "same-key", "-n", a, "--type=merge", "-p",
                     json.dumps({"spec": {"ports": [{"name": "http", "port": 18084, "targetPort": 18084}]}}))
        expected[f"{a}/same-key"] = (expected[f"{a}/same-key"][0], 18084)
        self.faults.gates["a_second"].set()
        self.checkpoint("initial-overlap-relist-keeps-b", expected, signature)
        wait_for(lambda: bool(self.faults.active), "real A WATCH active")
        self.faults.interrupt_a()
        wait_for(lambda: self.faults.seen["a_runtime"].is_set(), "runtime fresh LIST after 410")
        self.kubectl("delete", "service", "old-only", "same-key", "-n", a, "--wait=true")
        replacement = self.fixture(a, "same-key", 18085)
        old_uid = expected[f"{a}/same-key"][0]
        assert replacement["metadata"]["uid"] != old_uid
        self.kubectl("patch", "service", "stable", "-n", b, "--type=merge", "-p",
                     json.dumps({"spec": {"ports": [{"name": "http", "port": 18086, "targetPort": 18086}]}}))
        expected[f"{b}/stable"] = (expected[f"{b}/stable"][0], 18086)
        self.checkpoint("a-last-good-retained-while-b-updates", expected, signature)
        self.faults.gates["a_runtime"].set()
        del expected[f"{a}/old-only"]
        expected[f"{a}/same-key"] = (replacement["metadata"]["uid"], 18085)
        self.checkpoint("runtime-relist-deletes-and-replaces-uid", expected, signature)
        assert self.faults.counts[(a, "list")] == 3, "Unexpected A source duplication or relist"
        assert self.faults.counts[(b, "list")] == 1, "B unexpectedly relisted"
        self.reload_and_leadership(expected, signature)
        self.stop_monitor()
        assert self.monitor_samples > 0 and not self.monitor_errors, self.monitor_errors
        self.result["status"] = "PASS"

    def cleanup(self):
        self.stop_monitor()
        self.faults.stop()
        for proc in reversed(self.processes):
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=10)
        if self.proxy:
            self.proxy.shutdown()
            self.proxy.server_close()
            self.proxy_thread.join(timeout=5)
        for logfile in self.logs:
            logfile.close()
        self.result["faults"] = self.faults.timeline
        errors = []
        for name, uid in self.owned.items():
            try:
                text = self.kubectl("get", "namespace", name, "--ignore-not-found", "-o", "json")
                if text.strip():
                    current = json.loads(text)
                    if current["metadata"]["uid"] != uid or current["metadata"].get("labels", {}).get(LABEL) != self.run_id:
                        raise AssertionError(f"Refusing cleanup of namespace with changed ownership: {name}")
                    if name == self.names[0] and self.pod_uid:
                        text = self.kubectl("get", "pod", "controller-identity", "-n", name,
                                            "--ignore-not-found", "-o", "json")
                        if text.strip():
                            pod = json.loads(text)
                            assert pod["metadata"]["uid"] == self.pod_uid
                            assert pod["metadata"].get("labels", {}).get(LABEL) == self.run_id
                            self.kubectl("delete", "pod", "controller-identity", "-n", name,
                                         "--force", "--grace-period=0", "--wait=false")
                    self.kubectl("delete", "namespace", name, "--wait=false")
                    wait_for(lambda: not self.kubectl("get", "namespace", name, "--ignore-not-found", "-o", "name").strip(),
                             f"namespace cleanup {name}", 90)
                self.result["cleanup"].append(name)
            except Exception as error:
                errors.append(str(error))
        if errors:
            raise AssertionError("; ".join(errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", required=True, help="Exact kubectl context; never changes the current context")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/edgion-controller")
    parser.add_argument("--no-build", action="store_true", help="Use the specified existing binary without building")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    for executable in ("kubectl", "openssl", "bash"):
        assert shutil.which(executable), f"Missing {executable}"
    run(["kubectl", "--context", args.context, "get", "--raw", "/version"])
    nodes = json.loads(run(["kubectl", "--context", args.context, "get", "nodes", "-o", "json"]))
    assert any(any(c["type"] == "Ready" and c["status"] == "True" for c in
                   node.get("status", {}).get("conditions", [])) for node in nodes["items"]), "No Ready node"
    work = ROOT / "tmp/integration_testing" / f"namespace_watch_{uuid.uuid4().hex[:12]}"
    work.mkdir(parents=True)
    if not args.no_build:
        with (work / "build.log").open("w") as output:
            subprocess.run(["cargo", "build", "-p", "edgion-controller", "--bin", "edgion-controller"],
                           cwd=ROOT, stdout=output, stderr=subprocess.STDOUT, check=True, timeout=1800)
    assert args.binary.is_file(), f"Controller binary missing: {args.binary}"
    report = {"context": args.context, "binary": str(args.binary), "scenarios": []}
    for mode in ("leader-only", "all-serve"):
        scenario = Scenario(args, mode, work / mode)
        try:
            scenario.execute()
        except (Exception, KeyboardInterrupt) as error:
            scenario.result.update(status="FAIL", error=f"{type(error).__name__}: {error}")
            print(f"{mode}: {scenario.result['error']}", flush=True)
        finally:
            scenario.result["faults"] = scenario.faults.timeline
            (work / "in-progress.json").write_text(json.dumps(scenario.result, indent=2))
            previous_term = signal.signal(signal.SIGTERM, signal.SIG_IGN)
            previous_int = signal.signal(signal.SIGINT, signal.SIG_IGN)
            try:
                scenario.cleanup()
            except Exception as error:
                scenario.result.update(status="FAIL", cleanup_error=str(error))
            finally:
                signal.signal(signal.SIGTERM, previous_term)
                signal.signal(signal.SIGINT, previous_int)
            report["scenarios"].append(scenario.result)
            (work / "report.json").write_text(json.dumps(report, indent=2))
        print(f"{mode}: {scenario.result['status']}", flush=True)
        if scenario.result["status"] != "PASS":
            print(f"Report: {work / 'report.json'}", flush=True)
            return 1
    print(f"Report: {work / 'report.json'}", flush=True)
    return 0


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    raise SystemExit(main())
