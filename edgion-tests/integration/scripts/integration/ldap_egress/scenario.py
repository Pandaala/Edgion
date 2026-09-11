#!/usr/bin/env python3
"""D0054: real Gateway/OpenLDAP, deterministic DNS, observed TCP and TLS SNI.

Only resolution of reserved fixture names is replaced. TCP relays are opaque:
OpenLDAP itself negotiates TLS/StartTLS, verifies client certificates and binds.
All waits poll a readiness/state predicate with a deadline. Runtime artifacts
are retained, while only this run's processes and containers are stopped.
"""

import base64
import http.client
import http.server
import json
import os
from pathlib import Path
import select
import signal
import socket
import struct
import subprocess
import sys
import threading
import time
import uuid


TOKEN = "ldap-egress-integration-admin-token-0123456789"
HOST = "mixed.ldap-egress.test"
AUTH = "ldap " + base64.b64encode(b"alice:password123").decode()
FAIL_BODY = b"503 An unexpected error occurred"
LDAP_IMAGE = "osixia/openldap:1.5.0"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def wait_for(label, predicate, timeout=45):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (OSError, http.client.HTTPException) as error:
            last = error
        threading.Event().wait(0.1)
    raise AssertionError(f"Timed out waiting for {label}: {last}")


def request(port, path, headers=None):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=8)
    try:
        connection.request("GET", path, headers=headers or {})
        response = connection.getresponse()
        return response.status, dict(response.getheaders()), response.read(1024 * 1024)
    finally:
        connection.close()


def counter(values, name, **labels):
    # Exporters may add binary-owned global labels (for example service).
    return sum(value for key, value in values.items() if key.startswith(name + "{") and
               all(f'{label}="{expected}"' in key for label, expected in labels.items()))


def hello_sni(data):
    """Return SNI from a complete ClientHello body, with strict vector bounds."""
    def vector(offset, width):
        require(offset + width <= len(data), "truncated ClientHello vector")
        size = int.from_bytes(data[offset:offset + width], "big")
        begin = offset + width
        require(begin + size <= len(data), "ClientHello vector exceeds body")
        return begin, begin + size

    _, offset = vector(34, 1)  # legacy version, random, session ID
    _, offset = vector(offset, 2)  # cipher suites
    _, offset = vector(offset, 1)  # compression methods
    offset, end = vector(offset, 2)
    while offset < end:
        require(offset + 4 <= end, "truncated TLS extension")
        kind = int.from_bytes(data[offset:offset + 2], "big")
        begin, finish = vector(offset + 2, 2)
        require(finish <= end, "TLS extension exceeds extension list")
        if kind == 0:
            name, name_end = vector(begin, 2)
            require(name_end == finish and name + 3 <= finish, "invalid SNI list")
            require(data[name] == 0, "expected DNS SNI")
            start, stop = vector(name + 1, 2)
            require(stop == name_end, "unexpected SNI tail")
            return data[start:stop].decode("ascii")
        offset = finish
    return ""


class HelloObserver:
    def __init__(self):
        self.pending = bytearray()
        self.handshake = bytearray()
        self.sni = None

    def feed(self, chunk):
        if self.sni is not None:
            return
        self.pending.extend(chunk)
        require(len(self.pending) + len(self.handshake) <= 65536, "ClientHello observation cap exceeded")
        while self.pending:
            if self.pending[0] == 0x30:  # LDAP StartTLS request, before TLS records
                if len(self.pending) < 2:
                    return
                size, head = self.pending[1], 2
                if size & 0x80:
                    count = size & 0x7f
                    require(0 < count <= 4, "invalid LDAP BER length")
                    if len(self.pending) < 2 + count:
                        return
                    size = int.from_bytes(self.pending[2:2 + count], "big")
                    head += count
                if len(self.pending) < head + size:
                    return
                del self.pending[:head + size]
                continue
            if len(self.pending) < 5:
                return
            kind, version, size = struct.unpack("!BHH", self.pending[:5])
            require(kind == 22 and version >> 8 == 3, "expected TLS handshake record")
            if len(self.pending) < 5 + size:
                return
            self.handshake.extend(self.pending[5:5 + size])
            del self.pending[:5 + size]
            if len(self.handshake) >= 4:
                require(self.handshake[0] == 1, "expected ClientHello")
                length = int.from_bytes(self.handshake[1:4], "big")
                if len(self.handshake) >= 4 + length:
                    self.sni = hello_sni(self.handshake[4:4 + length])
                    return


class Recorder:
    """One allowed IPv4 relay and a denied IPv6 trap at the same port."""
    def __init__(self, upstream):
        self.upstream = upstream
        self.lock = threading.Lock()
        self.events = []
        self.listeners = []
        for family, address in [(socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")]:
            listener = socket.socket(family)
            self.listeners.append(listener)
            if family == socket.AF_INET6:
                listener.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
            listener.bind((address, self.port if family == socket.AF_INET6 else 0))
            self.port = listener.getsockname()[1]
            listener.listen()
            threading.Thread(target=self.accept, args=(listener, family == socket.AF_INET6), daemon=True).start()

    def accept(self, listener, denied):
        while True:
            try:
                connection, _ = listener.accept()
            except OSError:
                return
            event = {"denied": denied, "done": False, "sni": None, "error": None}
            with self.lock:
                self.events.append(event)
            threading.Thread(target=self.relay, args=(connection, event), daemon=True).start()

    def relay(self, connection, event):
        observer = HelloObserver()
        try:
            with connection:
                if event["denied"]:
                    return
                with socket.create_connection(("127.0.0.1", self.upstream), timeout=5) as upstream:
                    connection.settimeout(5)
                    while True:
                        readable, _, _ = select.select([connection, upstream], [], [], 10)
                        require(readable, "LDAP relay idle timeout")
                        for source in readable:
                            chunk = source.recv(16384)
                            if not chunk:
                                return
                            if source is connection:
                                observer.feed(chunk)
                            target = upstream if source is connection else connection
                            target.sendall(chunk)
        except (OSError, AssertionError) as error:
            with self.lock:
                event["error"] = str(error)
        finally:
            with self.lock:
                event["sni"] = observer.sni
                event["done"] = True

    def snapshot(self):
        with self.lock:
            return [dict(event) for event in self.events]

    def close(self):
        for listener in self.listeners:
            listener.close()


class Scenario:
    def __init__(self, repo, work, binaries, resolver_library):
        self.repo, self.work = repo, work
        self.binaries, self.resolver_library = binaries, resolver_library
        self.processes, self.containers, self.recorders = [], [], []
        self.env = dict(os.environ)
        self.env.update(EDGION_ADMIN_TOKEN=TOKEN, EDGION_GATEWAY_ADMIN_TOKEN=TOKEN,
                        EDGION_TOKEN=TOKEN, EDGION_ALLOW_INTEGRATION_TESTING_MODE="1")
        self.results = []
        self.backend = None
        (work / "conf").mkdir()
        (work / "logs").mkdir()
        self.ports = {}
        # Reserve the selected ports together to prevent intra-run reuse.
        reservations = []
        for key in ["admin", "grpc", "probe", "gw_admin", "gw_probe", "metrics", "http"]:
            sock = socket.socket()
            sock.bind(("127.0.0.1", 0))
            reservations.append(sock)
            self.ports[key] = sock.getsockname()[1]
        for sock in reservations:
            sock.close()

    def run(self, arguments, check=True, env=None, timeout=60):
        result = subprocess.run(arguments, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                env=env or self.env, timeout=timeout)
        with (self.work / "commands.log").open("a") as log:
            log.write(f"{arguments[0]}: exit {result.returncode}\n{result.stdout}\n")
        if check:
            require(result.returncode == 0, f"{arguments[0]} failed: {result.stdout}")
        return result

    def certs(self):
        certs = self.work / "certs" / "ldap"
        certs.mkdir()
        for name in ["ca", "wrong-ca"]:
            self.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
                      "-subj", f"/CN=ldap-egress-{name}", "-keyout", str(certs / f"{name}.key"),
                      "-out", str(certs / f"{name}.crt")])
        for name, usage, extra in [("server", "serverAuth", f"subjectAltName=DNS:{HOST}\n"),
                                   ("client", "clientAuth", "")]:
            self.run(["openssl", "req", "-newkey", "rsa:2048", "-nodes", "-subj", f"/CN=ldap-egress-{name}",
                      "-keyout", str(certs / f"{name}.key"), "-out", str(certs / f"{name}.csr")])
            extension = certs / f"{name}.ext"
            extension.write_text("basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\n"
                                 f"extendedKeyUsage={usage}\n{extra}")
            self.run(["openssl", "x509", "-req", "-in", str(certs / f"{name}.csr"), "-CA", str(certs / "ca.crt"),
                      "-CAkey", str(certs / "ca.key"), "-CAcreateserial", "-days", "2", "-extfile", str(extension),
                      "-out", str(certs / f"{name}.crt")])
        return certs

    def ldap(self, certs, mtls):
        name = f"edgion-ldap-egress-{uuid.uuid4().hex[:12]}"
        self.containers.append(name)
        bootstrap = self.repo / "edgion-tests/integration/conf/Services/ldap-openldap-tls/bootstrap"
        arguments = ["docker", "run", "--pull=never", "-d", "--name", name, "-p", "127.0.0.1::389", "-p", "127.0.0.1::636",
                     "-v", f"{certs}:/container/service/slapd/assets/certs",
                     "-v", f"{bootstrap}:/container/service/slapd/assets/config/bootstrap/ldif/custom:ro"]
        for key, value in {"LDAP_ORGANISATION": "Edgion Test", "LDAP_DOMAIN": "example.org",
                           "LDAP_ADMIN_PASSWORD": "admin", "LDAP_TLS": "true", "LDAP_TLS_CRT_FILENAME": "server.crt",
                           "LDAP_TLS_KEY_FILENAME": "server.key", "LDAP_TLS_CA_CRT_FILENAME": "ca.crt",
                           "LDAP_TLS_VERIFY_CLIENT": "demand" if mtls else "try", "LDAP_TLS_ENFORCE": "false"}.items():
            arguments.extend(["-e", f"{key}={value}"])
        self.run(arguments + [LDAP_IMAGE, "--copy-service"])
        directory = "/container/service/slapd/assets/certs"

        def ready():
            result = self.run(["docker", "exec", "-e", "LDAPTLS_REQCERT=never",
                               "-e", f"LDAPTLS_CERT={directory}/client.crt", "-e", f"LDAPTLS_KEY={directory}/client.key",
                               name, "ldapwhoami", "-x", "-H", "ldaps://127.0.0.1:636",
                               "-D", "uid=alice,ou=people,dc=example,dc=org", "-w", "password123"], check=False, timeout=10)
            return result.returncode == 0 and "dn:uid=alice" in result.stdout

        wait_for(f"OpenLDAP bind readiness ({name})", ready, 90)
        ports = json.loads(self.run(["docker", "inspect", name]).stdout)[0]["NetworkSettings"]["Ports"]
        return {mode: int(ports[f"{port}/tcp"][0]["HostPort"]) for mode, port in [("ldaps", 636), ("starttls", 389)]}

    def resource(self, kind, name, spec=None, labels=None, **fields):
        api = "gateway.networking.k8s.io/v1" if kind in ["GatewayClass", "Gateway", "HTTPRoute"] else "edgion.io/v1"
        if kind in ["Secret", "Service"]:
            api = "v1"
        if kind == "EndpointSlice":
            api = "discovery.k8s.io/v1"
        if kind == "EdgionGatewayConfig":
            api = "edgion.io/v1alpha1"
        metadata = {"name": name}
        if labels:
            metadata["labels"] = labels
        if kind not in ["GatewayClass", "EdgionGatewayConfig"]:
            metadata["namespace"] = "edgion-test"
        value = dict(apiVersion=api, kind=kind, metadata=metadata, **fields)
        if spec is not None:
            value["spec"] = spec
        # Runtime updates go exclusively through the Admin API. Do not also
        # publish the same generation through the filesystem watcher.
        directory = self.work / ("updates" if self.processes else "conf")
        directory.mkdir(exist_ok=True)
        path = directory / f"{kind}_{metadata.get('namespace', '')}_{name}.yaml"
        path.write_text(json.dumps(value, indent=2))
        return path

    def setup(self):
        self.run(["docker", "info"], timeout=15)
        image = self.run(["docker", "image", "inspect", LDAP_IMAGE], check=False)
        require(image.returncode == 0, f"Missing local fixture image; run docker pull {LDAP_IMAGE} before testing")
        (self.work / "config").mkdir()
        (self.work / "config" / "crd").symlink_to(self.repo / "config" / "crd", target_is_directory=True)
        certs = self.certs()
        self.backends = {False: self.ldap(certs, False), True: self.ldap(certs, True)}
        self.relays = {}
        for mtls, ports in self.backends.items():
            for mode, upstream in ports.items():
                recorder = Recorder(upstream)
                self.recorders.append(recorder)
                self.relays[mtls, mode] = recorder
                # Calibrate the negative witness: the denied IP really reaches
                # its trap on this host. A non-routable trap is not evidence.
                with socket.socket(socket.AF_INET6) as probe:
                    probe.settimeout(2)
                    probe.connect(("::", recorder.port))
                wait_for("denied trap calibration", lambda: recorder.snapshot() and recorder.snapshot()[-1]["done"])

        self.resource("GatewayClass", "ldap-egress", {"controllerName": "edgion.io/gateway-controller",
                      "parametersRef": {"group": "edgion.io", "kind": "EdgionGatewayConfig", "name": "ldap-egress"}})
        self.resource("EdgionGatewayConfig", "ldap-egress", {"server": {"threads": 2}})
        self.resource("Gateway", "ldap-egress", {"gatewayClassName": "ldap-egress", "listeners": [
            {"name": "http", "protocol": "HTTP", "port": self.ports["http"]}]})
        class Backend(http.server.BaseHTTPRequestHandler):
            def do_GET(handler):
                valid = handler.headers.get("X-Credential-Identifier") == "alice"
                body = b"ldap-egress-authenticated" if valid else b"missing LDAP identity"
                handler.send_response(200 if valid else 409)
                handler.send_header("Content-Length", str(len(body)))
                handler.end_headers()
                handler.wfile.write(body)

            def log_message(handler, *_args):
                pass

        self.backend = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Backend)
        threading.Thread(target=self.backend.serve_forever, daemon=True).start()
        backend_port = self.backend.server_port
        self.resource("Service", "ldap-egress", {"ports": [{"name": "http", "port": backend_port}]})
        self.resource("EndpointSlice", "ldap-egress", labels={"kubernetes.io/service-name": "ldap-egress"},
                      addressType="IPv4", ports=[{"name": "http", "port": backend_port, "protocol": "TCP"}],
                      endpoints=[{"addresses": ["127.0.0.1"], "conditions": {"ready": True}}])
        self.resource("HTTPRoute", "ldap-egress", {"parentRefs": [{"name": "ldap-egress"}],
            "hostnames": ["ldap-egress.example.com"], "rules": [{"filters": [{"type": "ExtensionRef", "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": "ldap-egress"}}],
                "backendRefs": [{"name": "ldap-egress", "port": backend_port}]}]})
        for name in ["ca", "wrong-ca"]:
            self.resource("Secret", f"ldap-{name}", type="Opaque", data={
                "ca.crt": base64.b64encode((certs / f"{name}.crt").read_bytes()).decode()})
        self.resource("Secret", "ldap-client", type="kubernetes.io/tls", data={
            key: base64.b64encode((certs / file).read_bytes()).decode()
            for key, file in [("tls.crt", "client.crt"), ("tls.key", "client.key")]})
        self.plugin("initial", "ldaps")
        self.start_processes()

    def plugin(self, phase, mode, host=HOST, ca="ldap-ca", san=None, mtls=False, identity=False):
        tls = {"verify": True, "validation": {"caCertificateRefs": [{"kind": "Secret", "name": ca}]}}
        if san:
            tls["validation"]["subjectAltNames"] = [{"type": "Hostname", "hostname": san}]
        if identity:
            tls["clientCertificateRef"] = {"kind": "Secret", "name": "ldap-client"}
        config = {"ldapHost": host, "ldapPort": self.relays[mtls, mode].port,
                  "ldaps": mode == "ldaps", "startTls": mode == "starttls", "tls": tls,
                  "baseDn": "ou=people,dc=example,dc=org", "attribute": "uid", "timeout": 3000, "realm": phase}
        return self.resource("EdgionPlugins", "ldap-egress", {"requestPlugins": [
            {"type": "LdapAuth", "config": config}]})

    def start_processes(self):
        ports, work = self.ports, self.work
        controller = f"""work_dir: {json.dumps(str(work))}
server:
  grpc_listen: "127.0.0.1:{ports['grpc']}"
  admin_listen: "127.0.0.1:{ports['admin']}"
  probe_listen: "127.0.0.1:{ports['probe']}"
conf_center:
  type: file_system
  conf_dir: {json.dumps(str(work / 'conf'))}
logging:
  log_dir: {json.dumps(str(work / 'logs'))}
"""
        gateway = f"""work_dir: {json.dumps(str(work))}
logging:
  log_dir: {json.dumps(str(work / 'logs'))}
metrics:
  listen: "127.0.0.1:{ports['metrics']}"
"""
        for name, config, security in [("controller", controller, "server"), ("gateway", gateway, "client")]:
            path = work / f"{name}.yaml"
            path.write_text(config + (work / f"{security}_security.yaml").read_text())
            arguments = [str(self.binaries / f"edgion-{name}"), "-c", str(path), "--work-dir", str(work)]
            env = dict(self.env)
            if name == "gateway":
                if sys.platform == "darwin":
                    env["DYLD_INSERT_LIBRARIES"] = str(self.resolver_library)
                else:
                    env["LD_PRELOAD"] = str(self.resolver_library)
                arguments += ["--server-addr", f"https://127.0.0.1:{ports['grpc']}", "--integration-testing-mode",
                              "--admin-listen", f"127.0.0.1:{ports['gw_admin']}",
                              "--probe-listen", f"127.0.0.1:{ports['gw_probe']}"]
            with (work / "logs" / f"{name}.log").open("w") as log:
                process = subprocess.Popen(arguments, env=env, stdout=log, stderr=subprocess.STDOUT)
            self.processes.append(process)

            def ready():
                require(process.poll() is None, f"{name} exited ({process.returncode}); see logs/{name}.log")
                return request(ports["probe" if name == "controller" else "gw_probe"], "/ready")[0] == 200

            wait_for(f"{name} readiness", ready)

    def metrics(self):
        status, _, body = request(self.ports["metrics"], "/metrics")
        require(status == 200, "metrics endpoint unavailable")
        values = {}
        for line in body.decode().splitlines():
            if line and not line.startswith("#"):
                key, value, *_ = line.split()
                values[key] = float(value)
        return values

    def check(self, phase, mode, expected=200, diagnostic=None, **options):
        path = self.plugin(phase, mode, **options)
        self.run([str(self.binaries / "edgion-cli"), "--server", f"http://127.0.0.1:{self.ports['admin']}",
                  "apply", "-f", str(path)])
        headers = {"Host": "ldap-egress.example.com"}

        def configured():
            status, response_headers, _ = request(self.ports["http"], "/ready", headers)
            challenge = next((value for key, value in response_headers.items() if key.lower() == "www-authenticate"), "")
            return status == 401 and phase in challenge

        wait_for(f"plugin generation {phase}", configured)
        recorder = self.relays[options.get("mtls", False), mode]
        before = len(recorder.snapshot())
        metrics_before = self.metrics()
        trace = uuid.uuid4().hex
        headers.update(Authorization=AUTH, **{"traceparent": f"00-{trace}-0123456789abcdef-00", "access_log": "test_store"})
        status, response_headers, body = request(self.ports["http"], f"/{phase}", headers)
        require(status == expected, f"{phase}: expected {expected}, got {status}: {body!r}")
        if expected == 503:
            require(body == FAIL_BODY, f"{phase}: non-static public failure body: {body!r}")
            public = body + json.dumps(response_headers).encode()
            for sensitive in [b"127.0.0.1", b"0.0.0.0", b"::", b"ldap-egress.test", b"ConnectionFailed", b"password123"]:
                require(sensitive not in public, f"{phase}: internal detail in public response")
        else:
            require(body == b"ldap-egress-authenticated", f"{phase}: authenticated chain did not complete")

        blocked = options.get("host") == "blocked.ldap-egress.test"
        if not blocked:
            wait_for(f"{phase} relay completion", lambda: len(recorder.snapshot()) > before and
                     all(event["done"] for event in recorder.snapshot()[before:]))
        events = recorder.snapshot()[before:]
        require(not any(event["denied"] for event in events), f"{phase}: denied candidate contacted: {events}")
        require(len(events) == (0 if blocked else 1), f"{phase}: unexpected TCP contacts: {events}")
        if not blocked:
            require(events[0]["sni"] == options.get("host", HOST), f"{phase}: original-hostname SNI lost: {events}")
        metrics_after = self.metrics()
        metric = "edgion_outbound_egress_blocked_total"
        delta = counter(metrics_after, metric, reason="unspecified") - counter(metrics_before, metric, reason="unspecified")
        require(delta == (2 if blocked else 1), f"{phase}: resolver fixture/filter did not run: delta={delta}")
        if diagnostic:
            metric = "edgion_ldap_auth_errors_total"
            require(counter(metrics_after, metric, kind=diagnostic) - counter(metrics_before, metric, kind=diagnostic) == 1,
                    f"{phase}: wrong static diagnostic classification")
            def access_log():
                code, _, content = request(self.ports["gw_admin"], f"/api/v1/testing/access-log/{trace}",
                                           {"Authorization": f"Bearer {TOKEN}"})
                return json.loads(content).get("data") if code == 200 else None
            entry = wait_for(f"{phase} access log", access_log)
            expected_tag = "ldap unreachable" if diagnostic == "unreachable" else "outbound tls san mismatch"
            require(expected_tag in json.dumps(entry), f"{phase}: missing PluginLog diagnostic")
        self.results.append({"phase": phase, "status": status, "egress_drops": delta, "connections": events})
        (self.work / "results.json").write_text(json.dumps(self.results, indent=2))
        print(f"PASS {phase}: status={status}, rejected candidates={int(delta)}, allowed contacts={len(events)}", flush=True)

    def exercise(self):
        for repetition in range(2):
            for mode in ["ldaps", "starttls"]:
                prefix = f"r{repetition}-{mode}"
                self.check(f"{prefix}-mixed", mode)
                self.check(f"{prefix}-blocked", mode, 503, "unreachable", host="blocked.ldap-egress.test")
                self.check(f"{prefix}-recovered", mode)
                self.check(f"{prefix}-wrong-host", mode, 503, "unreachable", host="wrong.ldap-egress.test")
                self.check(f"{prefix}-wrong-ca", mode, 503, "unreachable", ca="ldap-wrong-ca")
                self.check(f"{prefix}-san-match", mode, san=HOST)
                self.check(f"{prefix}-san-mismatch", mode, 503, "tls_san_mismatch", san="absent.ldap-egress.test")
                self.check(f"{prefix}-mtls-missing", mode, 503, mtls=True)
                self.check(f"{prefix}-mtls-identity", mode, mtls=True, identity=True)

    def close(self):
        for process in reversed(self.processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        for recorder in self.recorders:
            recorder.close()
        if self.backend:
            self.backend.shutdown()
            self.backend.server_close()
        for container in self.containers:
            result = self.run(["docker", "logs", container], check=False)
            (self.work / "logs" / f"{container}.log").write_text(result.stdout)
            self.run(["docker", "rm", "-f", container], check=False)


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    scenario = Scenario(*(Path(argument) for argument in sys.argv[1:]))
    try:
        scenario.setup()
        scenario.exercise()
        print(f"PASS LDAP egress: {len(scenario.results)} assertions; artifacts: {scenario.work}")
    finally:
        scenario.close()
