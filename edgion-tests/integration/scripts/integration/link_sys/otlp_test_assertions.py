#!/usr/bin/env python3
"""Owned standalone OTLP phases; stdlib-only, monotonic state-based readiness.

The main Rust dynamic runner cannot replace immutable process log outputs. This
helper keeps the same wait_phase contract (deadline plus final-state assertion)
inside the existing standalone exception. Socket work belongs to integration.
"""
import base64
import json
import os
from pathlib import Path
import re
import socket
import ssl
import subprocess
import sys
import time
import urllib.request
import uuid

ROOT = Path(os.environ["OTLP_PROJECT_ROOT"])
WORK = Path(os.environ["OTLP_WORK_DIR"])
CONFIG = WORK / "config"
GENERATED = WORK / "generated"
NS = "edgion-test"
NAME = "otlp-collector"
KEY = f"{NS}/{NAME}"
TOKEN_A = "otlp-integration-collector-a-token"
TOKEN_B = "otlp-integration-collector-b-token"
TOKEN_WRONG = "otlp-integration-rejected-token"
SENTINEL = "otlp-invalid-endpoint-sensitive-marker"
RAW_PATH = "/otlp-sensitive-customer-42"
MASKED_PATH = "[otlp-path-redacted]"
CLI = ROOT / "target/debug/edgion-cli"
PORTS = {name: int(os.environ.get(f"OTLP_{name}_PORT", default)) for name, default in {
    "CONTROLLER_ADMIN": 22101, "CONTROLLER_PROBE": 22100, "CONTROLLER_GRPC": 22151,
    "CONTROLLER_METRICS": 22190, "GATEWAY_ADMIN": 22001, "GATEWAY_PROBE": 22000,
    "GATEWAY_METRICS": 22090, "PROXY": 31930, "BACKEND": 33001, "TCP_PROXY": 31931, "TLS_PROXY": 31932, "TLS_BACKEND": 33002,
    "COLLECTOR_A": 24317, "HEALTH_A": 23133, "COLLECTOR_B": 24318, "HEALTH_B": 23134,
    "UNREACHABLE": 24319,
}.items()}
CONTROLLER = f"http://127.0.0.1:{PORTS['CONTROLLER_ADMIN']}"
GATEWAY = f"http://127.0.0.1:{PORTS['GATEWAY_ADMIN']}"
HOST = os.environ.get("OTLP_COLLECTOR_HOST", "127.0.0.1")
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def wait_phase(name, predicate, timeout=45):
    deadline = time.monotonic() + timeout
    last_error = "condition not yet satisfied"
    while True:
        try:
            value = predicate()
            if value:
                print(f"PASS {name}", flush=True)
                with (WORK / "report.log").open("a") as report:
                    report.write(f"PASS {name}\n")
                return value
        except (OSError, ValueError, KeyError, AssertionError) as error:
            last_error = str(error)
        if time.monotonic() >= deadline:
            raise AssertionError(f"Phase timed out: {name}: {last_error}")
        time.sleep(min(0.25, max(0, deadline - time.monotonic())))


def fetch(url, token=None):
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    with HTTP.open(urllib.request.Request(url, headers=headers), timeout=3) as response:
        return response.read().decode()


def admin(path):
    return json.loads(fetch(GATEWAY + path, os.environ["EDGION_GATEWAY_ADMIN_TOKEN"]))


def current():
    return admin(f"/configclient/LinkSys?namespace={NS}&name={NAME}").get("data", {})


def stats():
    return admin("/api/v1/debug/store-stats")["data"]["link_sys_runtime"]["otlp_clients"]


def retiring():
    return admin("/api/v1/debug/store-stats")["data"]["link_sys_runtime"]["otlp_retiring_clients"]


def metrics():
    return fetch(f"http://127.0.0.1:{PORTS['GATEWAY_METRICS']}/metrics")


def health_value():
    match = re.search(r'^edgion_link_sys_health\{[^\n]*link="' + re.escape(KEY)
                      + r'"[^\n]*\}\s+([01])(?:\.0)?\s*$', metrics(), re.MULTILINE)
    return bool(int(match[1])) if match else None


def healthy(expected=True):
    return health_value() is expected


def apply_file(path):
    subprocess.run([str(CLI), "--server", CONTROLLER, "apply", "-f", str(path)], check=True)


def apply(resource):
    GENERATED.mkdir(exist_ok=True)
    path = GENERATED / f"{resource['kind']}-{resource['metadata']['name']}.json"
    path.write_text(json.dumps(resource, indent=2) + "\n")
    apply_file(path)


def delete(kind, name):
    subprocess.run([str(CLI), "--server", CONTROLLER, "delete", kind, name, "-n", NS], check=True)


def secret(name, token):
    apply({"apiVersion": "v1", "kind": "Secret", "metadata": {"name": name, "namespace": NS},
           "type": "Opaque", "data": {"bearerToken": base64.b64encode(token.encode()).decode()}})


def endpoint(which):
    return f"http://{HOST}:{PORTS['COLLECTOR_' + which.upper()]}"


def link(phase, target=None, auth="otlp-auth-a", provider="otlp", auth_identity=None):
    config = {"endpoint": target or endpoint("a"), "timeoutMs": 2000,
              "auth": {"secretRef": {"name": auth}}}
    if auth_identity:
        config["auth"]["secretRef"].update(auth_identity)
    if provider == "redis":
        # Construct a real typed runtime client; connectivity is unnecessary
        # for proving type-change removal from the callable provider registry.
        config = {"endpoints": [f"redis://127.0.0.1:{PORTS['UNREACHABLE']}"]}
    resource = {"apiVersion": "edgion.io/v1", "kind": "LinkSys",
                "metadata": {"name": NAME, "namespace": NS,
                             "annotations": {"edgion-test-phase": phase}},
                "spec": {"type": provider, "config": config}}
    apply(resource)
    wait_phase(f"{phase}: current generation reaches Gateway",
               lambda: current().get("metadata", {}).get("annotations", {}).get("edgion-test-phase") == phase)


def records(which):
    path = WORK / f"collector-{which}" / "otlp-logs.ndjson"
    if not path.exists():
        return []
    result = []
    lines = path.read_text().splitlines()
    for index, line in enumerate(lines):
        try:
            batch = json.loads(line)
        except json.JSONDecodeError:
            # The collector may be appending its last batch during observation.
            if index == len(lines) - 1:
                continue
            raise
        for resource in batch.get("resourceLogs", []):
            for scope in resource.get("scopeLogs", []):
                result.extend(scope.get("logRecords", []))
    return result


def body(record):
    return json.loads(record["body"]["stringValue"])


def record_id(record):
    return body(record).get("request_info", {}).get("trace_id")


def found(which, trace):
    return next((record for record in records(which) if record_id(record) == trace), None)


def request(trace):
    headers = {"Host": "otlp-gateway.edgion-test.example.com",
               "traceparent": f"00-{trace}-0123456789abcdef-00"}
    url = f"http://127.0.0.1:{PORTS['PROXY']}{RAW_PATH}"
    with HTTP.open(urllib.request.Request(url, headers=headers), timeout=5) as response:
        assert response.status == 200
    return True


def delivered(which, phase):
    trace = uuid.uuid4().hex
    # Readiness is established by the shared health state first, then a single
    # uniquely identified request proves delivery; repeated traffic cannot hide loss.
    wait_phase(f"{phase}: provider healthy", healthy)
    request(trace)
    record = wait_phase(f"{phase}: collector {which} receives identified record",
                        lambda: found(which, trace))
    return trace, record


def preserved(which, phase):
    # Observe beyond one 10s healthy probe cadence plus the 3s candidate budget.
    # Each iteration sends a distinct record and checks delivery continuously;
    # this is a business observation window, not a readiness sleep that might
    # pass before asynchronous candidate preparation has even started.
    until = time.monotonic() + 15
    while True:
        trace = uuid.uuid4().hex
        request(trace)
        def retained_delivery():
            if not healthy():
                raise RuntimeError(f"{phase}: failed candidate degraded the active generation")
            return found(which, trace)
        wait_phase(f"{phase}: old generation delivers identified record", retained_delivery)
        if time.monotonic() >= until:
            break
    assert stats() == 1


def fallback(phase):
    trace = uuid.uuid4().hex
    request(trace)
    path = WORK / "logs/fallback.log"
    return wait_phase(f"{phase}: identified record reaches local fallback",
                      lambda: path.exists() and trace in path.read_text())


def compose(*args):
    subprocess.run(["docker", "compose", "-p", os.environ["OTLP_COMPOSE_PROJECT"],
                    "-f", str(CONFIG / "collectors.yaml"), *args], check=True)


def collector_ready(which):
    return fetch(f"http://127.0.0.1:{PORTS['HEALTH_' + which.upper()]}/") is not None


def prepare():
    # Fail on collision; never kill another test's process or collector.
    sockets = []
    try:
        for name, port in PORTS.items():
            sock = socket.socket()
            sock.bind(("0.0.0.0", port))
            sockets.append(sock)
    finally:
        for sock in sockets:
            sock.close()
    GENERATED.mkdir(exist_ok=True)
    (CONFIG / "controller.yaml").write_text(f"""server:
  grpc_listen: '127.0.0.1:{PORTS['CONTROLLER_GRPC']}'
  admin_listen: '127.0.0.1:{PORTS['CONTROLLER_ADMIN']}'
  probe_listen: '127.0.0.1:{PORTS['CONTROLLER_PROBE']}'
  metrics_listen: '127.0.0.1:{PORTS['CONTROLLER_METRICS']}'
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: '{WORK / 'resources'}'
logging:
  log_dir: logs
  log_level: info
""")
    output = f"""    otlp:
      linkSysRef:
        namespace: {NS}
        name: {NAME}
"""
    (CONFIG / "gateway.yaml").write_text(f"""gateway:
  server_addrs: ['https://127.0.0.1:{PORTS['CONTROLLER_GRPC']}']
  admin_listen: '127.0.0.1:{PORTS['GATEWAY_ADMIN']}'
  probe_listen: '127.0.0.1:{PORTS['GATEWAY_PROBE']}'
metrics:
  listen: '127.0.0.1:{PORTS['GATEWAY_METRICS']}'
logging:
  log_dir: logs
access_log_sensitive:
  mode: replace
  replace_with: "{MASKED_PATH}"
access_log:
  enabled: true
  output:
{output}  fallback:
    localFile:
      path: logs/fallback.log
      diskUsageThresholdPercent: 0
tcp_log:
  enabled: true
  output:
{output}tls_log:
  enabled: true
  output:
{output}ssl_log:
  enabled: false
""")
    services = {}
    for which, token in [("a", TOKEN_A), ("b", TOKEN_B)]:
        logs = WORK / f"collector-{which}"
        logs.mkdir()
        logs.chmod(0o777)
        collector = CONFIG / f"collector-{which}.json"
        collector.write_text(json.dumps({
            "receivers": {"otlp": {"protocols": {"grpc": {
                "endpoint": "0.0.0.0:4317", "auth": {"authenticator": "bearertokenauth"}}}}},
            "exporters": {"file": {"path": "/var/log/otel/otlp-logs.ndjson"}},
            "extensions": {"health_check": {"endpoint": "0.0.0.0:13133"},
                           "bearertokenauth": {"token": token}},
            "service": {"extensions": ["health_check", "bearertokenauth"],
                        "pipelines": {"logs": {"receivers": ["otlp"], "exporters": ["file"]}}},
        }, indent=2))
        services[f"collector-{which}"] = {
            "image": "otel/opentelemetry-collector-contrib:0.154.0",
            "command": ["--config=/etc/otelcol-contrib/config.json"],
            "ports": [f"{PORTS['COLLECTOR_' + which.upper()]}:4317",
                      f"127.0.0.1:{PORTS['HEALTH_' + which.upper()]}:13133"],
            "volumes": [f"{collector}:/etc/otelcol-contrib/config.json:ro", f"{logs}:/var/log/otel"],
            "restart": "no",
        }
    (CONFIG / "collectors.yaml").write_text(json.dumps({"services": services}, indent=2))


def initial():
    for path in sorted((ROOT / "edgion-tests/integration/conf/base").glob("*.yaml")):
        apply_file(path)
    fixture = ROOT / "edgion-tests/integration/conf/LinkSys/Otlp"
    for path in sorted(fixture.glob("*.yaml")):
        content = path.read_text().replace("30001", str(PORTS["BACKEND"]))
        content = content.replace("30002", str(PORTS["TLS_BACKEND"]))
        for original, name in [("31930", "PROXY"), ("31931", "TCP_PROXY"), ("31932", "TLS_PROXY")]:
            content = content.replace(original, str(PORTS[name]))
        content = content.replace("http://collector.example.com:4317", endpoint("a"))
        target = GENERATED / path.name
        target.write_text(content)
        apply_file(target)
    secret("otlp-auth-a", TOKEN_A)
    secret("otlp-auth-b", TOKEN_B)


def run():
    wait_phase("Gateway starts with collector absent", lambda: fetch(
        f"http://127.0.0.1:{PORTS['GATEWAY_PROBE']}/ready") is not None, 60)
    wait_phase("route and backend ready", lambda: request(uuid.uuid4().hex), 60)
    wait_phase("one connecting owner shared by access/TCP/TLS outputs", lambda: stats() == 1)
    wait_phase("unpublished collector has no health gauge", lambda: health_value() is None)
    fallback("collector absent")
    compose("up", "-d", "collector-a", "collector-b")
    for which in ["a", "b"]:
        wait_phase(f"collector {which} ready", lambda which=which: collector_ready(which), 60)
    trace, record = delivered("a", "collector recovery")
    attrs = {attribute["key"]: attribute["value"] for attribute in record["attributes"]}
    access_body = body(record)
    assert access_body, "LogRecord body must contain access JSON"
    assert attrs["http.request.method"]["stringValue"] == "GET"
    assert int(attrs["http.response.status_code"]["intValue"]) == 200
    assert access_body["request_info"]["path"] == MASKED_PATH
    assert attrs["url.path"]["stringValue"] == MASKED_PATH
    assert RAW_PATH not in json.dumps(record), "raw request path reached the OTLP record"
    assert record["severityNumber"] == 9
    assert int(record["timeUnixNano"]) > 0 and int(record["observedTimeUnixNano"]) > 0
    typed_trace = record["traceId"]
    if len(typed_trace) != 32:
        typed_trace = base64.b64decode(typed_trace).hex()
    assert typed_trace.lower() == trace, "typed OTLP trace ID must match the access-log identity"
    wait_phase("record body, typed trace, attributes, severity and timestamps preserved", lambda: True)

    # Both stream loggers share the same key. Exercise real successful streams,
    # then identify their different payloads by their dedicated listener ports.
    for protocol in ["TCP", "TLS"]:
        def stream_request(protocol=protocol):
            connection = socket.create_connection(("127.0.0.1", PORTS[protocol + "_PROXY"]), timeout=3)
            if protocol == "TLS":
                context = ssl.create_default_context(cafile=str(WORK / "certs/otlp/backend/ca.crt"))
                connection = context.wrap_socket(connection, server_hostname="otlp-backend.edgion-test.example.com")
            with connection:
                connection.sendall(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                response = bytearray()
                while True:
                    chunk = connection.recv(4096)
                    if not chunk:
                        break
                    response.extend(chunk)
                return response.startswith(b"HTTP/1.1 200")
        wait_phase(f"successful {protocol} forwarding", stream_request)
        wait_phase(f"{protocol} record reaches shared collector", lambda protocol=protocol: any(
            body(item).get("listener_port") == PORTS[protocol + "_PROXY"] for item in records("a")))
    assert stats() == 1

    for phase, target, auth in [
        ("unreachable update", f"http://{HOST}:{PORTS['UNREACHABLE']}", "otlp-auth-a"),
        ("invalid endpoint update", f"http://{SENTINEL}@collector.invalid/private?secret=hidden", "otlp-auth-a"),
        ("unresolved auth update", endpoint("b"), "otlp-auth-missing"),
    ]:
        link(phase, target, auth)
        preserved("a", phase)
    for identity in [{"kind": "ConfigMap"}, {"group": "external.example"}]:
        phase = f"invalid Secret identity {next(iter(identity))}"
        link(phase, endpoint("b"), "otlp-auth-a", auth_identity=identity)
        wait_phase(f"{phase}: same-name Secret is not transported",
                   lambda: current().get("spec", {}).get("config", {}).get("auth", {}).get("secret") is None)
        preserved("a", phase)

    secret("otlp-auth-malformed", "x" * 8193)
    link("malformed auth update", endpoint("b"), "otlp-auth-malformed")
    preserved("a", "malformed auth update")

    # Syntactically valid credentials can still be rejected remotely. Candidate
    # preflight must observe that rejection before retiring the working owner.
    secret("otlp-auth-wrong", TOKEN_WRONG)
    link("wrong-token update", endpoint("b"), "otlp-auth-wrong")
    preserved("a", "collector rejects valid-format wrong-token candidate")

    link("valid hot replacement", endpoint("b"), "otlp-auth-b")
    # Health remains true while A serves; delivery to B is the publication gate.
    replacement_trace = uuid.uuid4().hex
    def replacement_ready():
        request(replacement_trace)
        return found("b", replacement_trace)
    wait_phase("valid replacement delivers to collector B", replacement_ready, 60)
    trace, _ = delivered("b", "replacement settled")
    assert found("a", trace) is None, "new request must not reach retired collector A"
    assert stats() == 1

    delete("Secret", "otlp-auth-b")
    wait_phase("deleted credential clears resolved material", lambda: current().get(
        "spec", {}).get("config", {}).get("auth", {}).get("secret") is None)
    delivered("b", "Secret deletion preserves last-known-good")
    secret("otlp-auth-b", TOKEN_B)
    wait_phase("Secret recreation resolves credential", lambda: current().get(
        "spec", {}).get("config", {}).get("auth", {}).get("secret") == "[redacted]")
    delivered("b", "Secret recreation recovers")

    link("wrong provider type", provider="redis")
    wait_phase("type change removes OTLP runtime owner", lambda: stats() == 0 and retiring() == 0)
    wait_phase("foreign Redis runtime client is registered", lambda: admin(
        "/api/v1/debug/store-stats")["data"]["link_sys_runtime"]["redis_clients"] == 1)
    fallback("wrong provider type")
    link("restore OTLP provider", endpoint("b"), "otlp-auth-b")
    wait_phase("OTLP type change removes the former Redis client", lambda: admin(
        "/api/v1/debug/store-stats")["data"]["link_sys_runtime"]["redis_clients"] == 0)
    delivered("b", "restore after type change")

    for phase in ["resource deletion", "resource re-add"]:
        if phase == "resource deletion":
            delete("LinkSys", NAME)
            wait_phase("resource deletion clears config and runtime residue", lambda: not current() and stats() == 0 and retiring() == 0)
            fallback(phase)
        else:
            link(phase, endpoint("b"), "otlp-auth-b")
            publication_trace = uuid.uuid4().hex
            def readd_published():
                request(publication_trace)
                return found("b", publication_trace)
            wait_phase("resource re-add publishes and delivers through the new generation",
                       readd_published, 60)
            delivered("b", phase)
            assert stats() == 1

    compose("stop", "collector-b")
    wait_phase("running collector outage marks provider unhealthy", lambda: healthy(False), 60)
    fallback("running collector outage")
    compose("start", "collector-b")
    wait_phase("restored collector B ready", lambda: collector_ready("b"), 60)
    delivered("b", "running collector recovery")

    # All labels are validated keys. Credentials and invalid endpoint material
    # must not appear on system-log, health or admin config surfaces.
    metric_text = metrics()
    labels = re.findall(r'^edgion_link_sys_health\{[^\n]*link="([^"]+)"', metric_text, re.MULTILINE)
    assert labels and set(labels) == {KEY}, f"unexpected health identity: {labels}"
    admin_text = json.dumps(current())
    log_text = "\n".join(path.read_text(errors="replace") for path in (WORK / "logs").glob("*.log")
                         if path.name != "fallback.log")
    for value in [TOKEN_A, TOKEN_B, TOKEN_WRONG, SENTINEL]:
        assert value not in log_text + metric_text, "sensitive provider material reached diagnostics"
    assert all(token not in admin_text for token in [TOKEN_A, TOKEN_B, TOKEN_WRONG])
    wait_phase("safe key metrics and admin/system-log secret redaction", lambda: True)
    shutdown_ids = [uuid.uuid4().hex for _ in range(5)]
    (WORK / "shutdown-ids.json").write_text(json.dumps(shutdown_ids))
    for trace in shutdown_ids:
        request(trace)
    print("Identified shutdown batch sent; awaiting owned Gateway shutdown", flush=True)


def wait_pid(pid, timeout):
    def stopped():
        result = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True)
        return result.returncode != 0 or not result.stdout.strip() or result.stdout.strip().startswith("Z")
    wait_phase(f"owned process {pid} exits", stopped, timeout)


def main():
    command = sys.argv[1]
    if command == "prepare":
        prepare()
    elif command == "controller-ready":
        wait_phase("Controller ready", lambda: fetch(
            f"http://127.0.0.1:{PORTS['CONTROLLER_PROBE']}/ready") is not None, 60)
    elif command == "apply-initial":
        initial()
    elif command == "run":
        run()
    elif command == "wait-pid":
        wait_pid(int(sys.argv[2]), int(sys.argv[3]))
    elif command == "shutdown-delivery":
        expected = json.loads((WORK / "shutdown-ids.json").read_text())
        wait_phase("all five identified shutdown records delivered", lambda: all(found("b", trace) for trace in expected), 15)
        print("OTLP LinkSys dynamic integration passed", flush=True)
    else:
        raise ValueError(f"Unknown helper command: {command}")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        with (WORK / "report.log").open("a") as report:
            report.write(f"FAIL {type(error).__name__}: {error}\n")
        raise
