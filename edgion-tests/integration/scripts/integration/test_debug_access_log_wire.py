#!/usr/bin/env python3
"""D0087: real Controller publication and debug response-header assertions.

Invoked through test_debug_access_log_wire.sh for a whole-run process-group
deadline. Replace and Wrap require separate Gateway processes because masking
is startup-global. No external Python packages or shared fixture edits needed.
"""

import argparse
from http.client import HTTPConnection, HTTPException
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import uuid


ROOT = Path(__file__).resolve().parents[4]
UTILS = Path(__file__).resolve().parent.parent / "utils"
NAMESPACE = "debug-wire"
HOST = "debug-wire.example.com"
TOKEN = "integration-debug-wire-admin-token-0123456789"
MASK = "MASKED-D0087"
PREFIX, SUFFIX = "<WIRE>", "</WIRE>"
SUPPRESSED = "wrap mode:header suppressed"
MAX_BODY = 1024 * 1024


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def decode_json(body):
    return json.loads(body, object_pairs_hook=unique_object)


def http(port, path, method="GET", body=None, headers=None):
    # Direct loopback connections: never inherit proxy environment or redirects.
    conn = HTTPConnection("127.0.0.1", port, timeout=2)
    try:
        conn.request(method, path, body=body, headers=headers or {})
        response = conn.getresponse()
        data = response.read(MAX_BODY + 1)
        require(len(data) <= MAX_BODY, "Fixture response exceeded 1 MiB")
        return response.status, response.getheaders(), data
    finally:
        conn.close()


def values(headers, name):
    return [value for key, value in headers if key.lower() == name.lower()]


def resource(kind, name, spec, api="edgion.io/v1", namespaced=True):
    metadata = {"name": name}
    if namespaced:
        metadata["namespace"] = NAMESPACE
    return {"apiVersion": api, "kind": kind, "metadata": metadata, "spec": spec}


def write_json(path, obj):
    # JSON is a YAML subset accepted by the product's YAML readers.
    path.write_text(json.dumps(obj, indent=2) + "\n")


class Topology:
    def __init__(self, mode, work, binaries, timeout):
        self.mode, self.work, self.binaries, self.timeout = mode, work, binaries, timeout
        self.processes = []
        self.reservations = {}
        self.ports = {}
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("EDGION_")}
        self.env.update(EDGION_ADMIN_TOKEN=TOKEN, EDGION_GATEWAY_ADMIN_TOKEN=TOKEN,
                        EDGION_ALLOW_INTEGRATION_TESTING_MODE="1")

    def reserve_ports(self):
        for name in ("ctrl_admin", "ctrl_grpc", "ctrl_probe", "ctrl_metrics",
                     "gw_admin", "gw_probe", "gw_metrics", "gateway", "backend"):
            sock = socket.socket()
            sock.bind(("127.0.0.1", 0))
            self.reservations[name] = sock
            self.ports[name] = sock.getsockname()[1]

    def start(self, name, args, ports):
        for port in ports:
            self.reservations.pop(port).close()
        with (self.work / f"{name}.log").open("wb") as log:
            process = subprocess.Popen([str(self.binaries[name]), *args],
                                       cwd=self.work, env=self.env, stdout=log, stderr=log)
        self.processes.append((name, process))

    def alive(self):
        for name, process in self.processes:
            require(process.poll() is None,
                    f"{name} exited with {process.returncode}; see {self.work / (name + '.log')}")

    def wait(self, label, predicate):
        deadline = time.monotonic() + self.timeout
        last = "condition not met"
        while time.monotonic() < deadline:
            self.alive()
            try:
                result = predicate()
                if result:
                    return result
            except (OSError, HTTPException) as error:
                last = str(error)
            time.sleep(0.1)
        raise AssertionError(f"Timed out: {self.mode} {label}: {last}; artifacts: {self.work}")

    def admin(self, target, path, method="GET", obj=None):
        headers = {"Authorization": f"Bearer {TOKEN}", "Content-Type": "application/yaml"}
        body = json.dumps(obj) if obj is not None else None
        return http(self.ports[target], path, method, body, headers)

    def publish(self, obj, update=False):
        kind, meta = obj["kind"], obj["metadata"]
        base = f"/api/v1/namespaced/{kind}/{NAMESPACE}"
        path = f"{base}/{meta['name']}" if update else base
        code, _, body = self.admin("ctrl_admin", path, "PUT" if update else "POST", obj)
        require(code in (200, 201), f"Publish {kind}/{meta['name']} failed: HTTP {code}: {body!r}")
        # Save exactly the operator input, separately from Controller-owned files.
        write_json(self.work / f"published-{kind}-{meta['name']}.json", obj)
        code, _, body = self.admin("ctrl_admin", f"{base}/{meta['name']}")
        require(code == 200, f"Read published {kind} failed: HTTP {code}")
        version = decode_json(body)["metadata"]["resourceVersion"]
        self.wait(f"publish {kind}/{meta['name']} rv={version}",
                  lambda: (self.gateway_resource(kind, meta['name']) or {}).get(
                      "metadata", {}).get("resourceVersion") == version)

    def gateway_resource(self, kind, name):
        code, _, body = self.admin("gw_admin", f"/configclient/{kind}?namespace={NAMESPACE}&name={name}")
        require(code == 200, f"Gateway config read failed: HTTP {code}")
        return decode_json(body).get("data")

    def delete_plugin(self):
        code, _, body = self.admin("ctrl_admin", f"/api/v1/namespaced/EdgionPlugins/{NAMESPACE}/debug",
                                   "DELETE")
        require(code == 204, f"Delete plugin failed: HTTP {code}: {body!r}")
        self.wait("plugin deletion published", lambda: self.gateway_resource("EdgionPlugins", "debug") is None)

    def route(self, debug):
        refs = ["boundary"] + (["debug"] if debug else [])
        obj = resource("HTTPRoute", "wire", {
            "parentRefs": [{"name": "wire", "sectionName": "http"}],
            "hostnames": [HOST],
            "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": "/"}}],
                       "filters": [{"type": "ExtensionRef", "extensionRef": {
                           "group": "edgion.io", "kind": "EdgionPlugins", "name": name}} for name in refs],
                       "backendRefs": [{"name": "backend", "port": self.ports["backend"]}]}],
        }, "gateway.networking.k8s.io/v1")
        obj["metadata"]["labels"] = {"wire-snapshot": "terminal-only-marker"}
        return obj

    def plugin(self, generation, enabled=True, present=True):
        entries = [{"type": "ResponseHeaderModifier", "config": {"set": [
            {"name": "X-Wire-Generation", "value": generation}]}}]
        if present:
            entries.append({"type": "DebugAccessLogToHeader", "enable": enabled, "config": {}})
        return resource("EdgionPlugins", "debug", {"upstreamResponseFilterPlugins": entries})

    def prepare(self):
        self.work.mkdir()
        (self.work / "conf").mkdir()
        (self.work / "logs").mkdir()
        (self.work / "config").mkdir()
        (self.work / "config/crd").symlink_to(ROOT / "config/crd", target_is_directory=True)
        self.reserve_ports()
        # Use the canonical mTLS generator and transport blocks, including its
        # own completeness checks. No copied transport policy or shared CA cache.
        subprocess.run(["bash", "-c", 'source "$1"; conf_sync_distribute_certs "$2" && '
                        'conf_sync_security_block server > "$2/server-tls.yaml" && '
                        'conf_sync_security_block client > "$2/client-tls.yaml"',
                        "debug-wire", str(UTILS / "conf_sync_certs.sh"), str(self.work)],
                       env=self.env, check=True, timeout=60, stdout=subprocess.DEVNULL)
        controller = {
            "server": {key: f"127.0.0.1:{self.ports[port]}" for key, port in (
                ("admin_listen", "ctrl_admin"), ("grpc_listen", "ctrl_grpc"),
                ("probe_listen", "ctrl_probe"), ("metrics_listen", "ctrl_metrics"))},
            "logging": {"log_dir": "logs", "console": True},
            "conf_center": {"type": "file_system", "conf_dir": str(self.work / "conf")},
            "conf_sync": {"no_sync_kinds": ["ReferenceGrant", "Secret"], "default_capacity": 200},
        }
        gateway = {
            "logging": {"log_dir": "logs"},
            "server": {"threads": 1, "grace_period_seconds": 0, "graceful_shutdown_timeout_seconds": 1},
            "metrics": {"listen": f"127.0.0.1:{self.ports['gw_metrics']}"},
            "access_log_options": {"includeRawUri": True},
            "access_log_sensitive": {"mode": self.mode, "replace_with": MASK,
                                     "wrap_prefix": PREFIX, "wrap_suffix": SUFFIX},
        }
        for name, config, role in (("controller", controller, "server"), ("gateway", gateway, "client")):
            # Each top-level value is JSON, with the canonical YAML transport
            # mapping appended. This avoids a third-party YAML dependency.
            content = "".join(f"{key}: {json.dumps(value)}\n" for key, value in config.items())
            (self.work / f"{name}.yaml").write_text(content + (self.work / f"{role}-tls.yaml").read_text())
        seeds = [
            resource("GatewayClass", "debug-wire", {"controllerName": "edgion.io/gateway-controller"},
                     "gateway.networking.k8s.io/v1", False),
            resource("Gateway", "wire", {"gatewayClassName": "debug-wire", "listeners": [
                {"name": "http", "protocol": "HTTP", "port": self.ports["gateway"],
                 "allowedRoutes": {"namespaces": {"from": "Same"}}}]}, "gateway.networking.k8s.io/v1"),
            resource("Service", "backend", {"ports": [{"name": "http", "port": self.ports["backend"],
                     "targetPort": self.ports["backend"]}]}, "v1"),
        ]
        endpoints = {"apiVersion": "discovery.k8s.io/v1", "kind": "EndpointSlice",
                     "metadata": {"name": "backend", "namespace": NAMESPACE,
                                  "labels": {"kubernetes.io/service-name": "backend"}},
                     "addressType": "IPv4", "ports": [{"name": "http", "protocol": "TCP",
                                                         "port": self.ports["backend"]}],
                     "endpoints": [{"addresses": ["127.0.0.1"], "conditions": {"ready": True}}]}
        boundary = resource("EdgionPlugins", "boundary", {
            "waf": {"mode": "detectionOnly", "rules":
                    'SecRule REQUEST_HEADERS:X-Wire-Waf "@streq observe" "id:8701,phase:1,deny,status:403"'},
            "accessLogExtern": [{"key": "wire_boundary", "from": "routeLabel", "name": "wire-snapshot"}],
        })
        for obj in [*seeds, endpoints, boundary, self.route(False)]:
            meta = obj["metadata"]
            filename = f"{obj['kind']}_{meta.get('namespace', '')}_{meta['name']}.yaml"
            write_json(self.work / "conf" / filename, obj)
        write_json(self.work / "ports.json", self.ports)
        self.start("test_server", ["--http-ports", str(self.ports["backend"]), "--grpc-ports", "",
                                  "--websocket-port", "0", "--tcp-port", "0", "--udp-port", "0"], ["backend"])
        self.wait("backend ready", lambda: http(self.ports["backend"], "/health")[0] == 200)
        self.start("controller", ["-c", "controller.yaml", "--work-dir", str(self.work)],
                   ["ctrl_admin", "ctrl_grpc", "ctrl_probe", "ctrl_metrics"])
        self.wait("controller ready", lambda: http(self.ports["ctrl_probe"], "/ready")[0] == 200)
        self.start("gateway", ["-c", "gateway.yaml", "--work-dir", str(self.work),
                               "--server-addr", f"https://127.0.0.1:{self.ports['ctrl_grpc']}",
                               "--admin-listen", f"127.0.0.1:{self.ports['gw_admin']}",
                               "--probe-listen", f"127.0.0.1:{self.ports['gw_probe']}",
                               "--integration-testing-mode"],
                   ["gw_admin", "gw_probe", "gw_metrics", "gateway"])
        self.wait("gateway ready", lambda: http(self.ports["gw_probe"], "/health")[0] == 200)

    def request(self):
        trace = f"d0087-{self.mode}-{uuid.uuid4()}"
        response = http(self.ports["gateway"], "/status/201?private=wire-query-sentinel", headers={
            "Host": HOST, "x-trace-id": trace, "access_log": "test_store", "X-Wire-Waf": "observe"})
        return trace, response

    def terminal_log(self, trace):
        def fetch():
            code, _, body = self.admin("gw_admin", f"/api/v1/testing/access-log/{trace}")
            require(code in (200, 404), f"Access Log Store failed: HTTP {code}")
            return decode_json(body).get("data") if code == 200 else None
        return self.wait(f"terminal access log {trace}", fetch)

    def check(self, label, generation=None, debug=False, deleted=False):
        expected_status = 500 if deleted else 201

        def converged():
            trace, response = self.request()
            status, headers, _ = response
            if status != expected_status or values(headers, "X-Wire-Generation") != (
                    [generation] if generation else []):
                return None
            return trace, response

        trace, (status, headers, body) = self.wait(f"wire generation {label}", converged)
        if not deleted:
            require(body == b"Status: 201", f"{label}: response body changed: {body!r}")
        header_values = values(headers, "X-Debug-Access-Log")
        if debug and self.mode == "replace":
            require(len(header_values) == 1, f"{label}: expected exactly one debug header")
            snapshot = decode_json(header_values[0])
            require(isinstance(snapshot, dict), "Debug snapshot must be a JSON object")
            require(snapshot["request_info"]["x-trace-id"] == trace, "Debug snapshot trace mismatch")
            for field in ("direct_peer_ip", "remote_ip", "path"):
                require(snapshot["request_info"][field] == MASK, f"Unmasked debug request field: {field}")
            require(snapshot.get("request_uri") == MASK, "Raw URI projection was not replaced")
            require("wire-query-sentinel" not in header_values[0], "Raw query leaked into debug snapshot")
            require("ext" not in snapshot and "waf" not in snapshot, "Terminal-only fields leaked into debug snapshot")
            require(snapshot["request_info"]["status"] == 0, "Debug snapshot claimed terminal status")
        else:
            require(not header_values, f"{label}: unexpected debug response header")
        terminal = self.terminal_log(trace)
        require(terminal["request_info"]["x-trace-id"] == trace, "Terminal trace mismatch")
        require(terminal["request_info"]["status"] == status, "Terminal status differs from response")
        logs = [log for stage in terminal.get("stage_logs", [])
                for group in stage.get("edgion_plugins", []) for log in group.get("logs", [])
                if log.get("name") == "DebugAccessLogToHeader"]
        require(len(logs) == int(debug), f"{label}: unexpected debug plugin execution count: {logs}")
        decisions = [item for log in logs for item in log.get("log", [])]
        require(decisions.count(SUPPRESSED) == int(debug and self.mode == "wrap"),
                f"{label}: wrong Wrap suppression decision: {decisions}")
        if not deleted:
            require(terminal.get("ext", {}).get("wire_boundary") == "terminal-only-marker",
                    "Terminal ext positive control missing")
            require(terminal.get("waf", {}).get("mode") == "detection_only", "Terminal WAF positive control missing")
            require(terminal["waf"].get("findings"), "WAF fixture did not record its matching rule")
            remote = terminal["request_info"]["remote_ip"]
            require(remote == (MASK if self.mode == "replace" else f"{PREFIX}127.0.0.1{SUFFIX}"),
                    "Terminal masking does not match process startup mode")
        write_json(self.work / f"{label}-evidence.json", {
            "trace": trace, "status": status, "headers": headers, "body": body.decode(), "terminal": terminal})
        self.alive()
        print(f"PASS {self.mode}/{label}: wire + terminal log, trace={trace}", flush=True)

    def run(self):
        self.prepare()
        self.check("baseline")
        self.publish(self.plugin("added"))
        self.publish(self.route(True), update=True)
        self.check("added", "added", debug=True)
        self.publish(self.plugin("disabled", enabled=False), update=True)
        self.check("disabled", "disabled")
        self.publish(self.plugin("entry-removed", present=False), update=True)
        self.check("entry-removed", "entry-removed")
        self.publish(self.plugin("reenabled"), update=True)
        self.check("reenabled", "reenabled", debug=True)
        self.delete_plugin()
        self.check("resource-deleted", deleted=True)
        self.publish(self.plugin("recreated"))
        self.check("recreated", "recreated", debug=True)

    def close(self):
        for sock in self.reservations.values():
            sock.close()
        for _, process in reversed(self.processes):
            if process.poll() is None:
                process.terminate()
        for _, process in reversed(self.processes):
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, default=ROOT / "target/debug")
    parser.add_argument("--state-timeout", type=float, default=30)
    args = parser.parse_args()
    require(args.state_timeout > 0, "--state-timeout must be positive")
    binaries = {name: Path(os.environ.get(env, args.bin_dir / binary)).resolve()
                for name, env, binary in (("controller", "EDGION_CONTROLLER_BIN", "edgion-controller"),
                                          ("gateway", "EDGION_GATEWAY_BIN", "edgion-gateway"),
                                          ("test_server", "EDGION_TEST_SERVER_BIN", "test_server"))}
    for path in binaries.values():
        require(path.is_file() and os.access(path, os.X_OK),
                f"Missing binary: {path}; build the Controller, Gateway and test_server before running")
    work_root = ROOT / "tmp/integration_testing"
    work_root.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="debug-access-log-wire.", dir=work_root))
    print(f"Artifacts: {work}", flush=True)

    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    for mode in ("replace", "wrap"):
        topology = Topology(mode, work / mode, binaries, args.state_timeout)
        try:
            topology.run()
        finally:
            topology.close()
    print("PASS D0087: 14 isolated wire/lifecycle checks; existing suites unchanged", flush=True)


if __name__ == "__main__":
    main()
