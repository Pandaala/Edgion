#!/usr/bin/env python3
"""D0057: cross-process OIDC egress, proxy isolation, and TLS identity proof.

The Gateway uses a process-local system-resolver fixture. Calibrated TCP traps
make denied DNS candidates and proxy use observable, while a local HTTPS IdP
drives discovery, authorization callback, token exchange, and JWKS validation.
Every wait polls a state predicate with a deadline; runtime secrets stay under
the per-run work directory and are never included in public-response failures.
"""

import base64
import http.client
import http.server
import json
import os
from pathlib import Path
import signal
import socket
import ssl
import subprocess
import sys
import threading
import time
from urllib.parse import parse_qs, urlparse


ADMIN_TOKEN = "oidc-egress-integration-admin-token-0123456789"
ROUTE_HOST = "oidc-egress.example.com"
IDP_HOST = "mixed.oidc-egress.test"
BLOCKED_DISCOVERY_HOST = "blocked.oidc-egress.test"
BLOCKED_JWKS_HOST = "blocked-jwks.oidc-egress.test"
BLOCKED_TOKEN_HOST = "blocked-token.oidc-egress.test"
CLIENT_SECRET = "oidc-egress-client-secret"
SESSION_SECRET = "oidc-egress-session-secret-at-least-32-bytes"
JWT_KID = "oidc-egress-key"
PUBLIC_FAILURE_BODY = b"502 Bad Gateway"


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
        except (OSError, http.client.HTTPException, json.JSONDecodeError) as error:
            last = error
        threading.Event().wait(0.1)
    raise AssertionError(f"Timed out waiting for {label}: {last}")


def request(port, path, method="GET", headers=None, body=None, timeout=8):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    try:
        connection.request(method, path, body=body, headers=headers or {})
        response = connection.getresponse()
        content = response.read(1024 * 1024 + 1)
        require(len(content) <= 1024 * 1024, "response exceeded integration-test cap")
        return response.status, response.getheaders(), content
    finally:
        connection.close()


def header_values(headers, wanted):
    return [value for name, value in headers if name.lower() == wanted.lower()]


def cookie_value(headers, wanted):
    for value in header_values(headers, "set-cookie"):
        pair = value.split(";", 1)[0]
        name, separator, content = pair.partition("=")
        if separator and name == wanted:
            return content
    return None


def metric_value(values, name, **labels):
    return sum(
        value
        for key, value in values.items()
        if key.startswith(name + "{")
        and all(f'{label}="{expected}"' in key for label, expected in labels.items())
    )


def b64url(value):
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


class ConnectionTrap:
    """A calibrated listener whose post-calibration connection count must stay zero."""

    def __init__(self, family, address, port=0):
        self.lock = threading.Lock()
        self.connections = 0
        self.listener = socket.socket(family)
        if family == socket.AF_INET6:
            self.listener.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
        self.listener.bind((address, port))
        self.port = self.listener.getsockname()[1]
        self.listener.listen()
        threading.Thread(target=self._accept, daemon=True).start()

    def _accept(self):
        while True:
            try:
                connection, _ = self.listener.accept()
            except OSError:
                return
            with self.lock:
                self.connections += 1
            connection.close()

    def count(self):
        with self.lock:
            return self.connections

    def calibrate(self, address):
        family = socket.AF_INET6 if ":" in address else socket.AF_INET
        before = self.count()
        with socket.socket(family) as probe:
            probe.settimeout(2)
            probe.connect((address, self.port))
        wait_for("trap calibration", lambda: self.count() == before + 1)
        return self.count()

    def close(self):
        self.listener.close()


class OidcState:
    def __init__(self):
        self.lock = threading.Lock()
        self.hits = {}
        self.sni = []
        self.token_auth_valid = 0
        self.port = None
        self.issuer = None
        self.jwt = None
        self.jwk_n = None

    def record_hit(self, path):
        with self.lock:
            self.hits[path] = self.hits.get(path, 0) + 1

    def record_sni(self, name):
        with self.lock:
            self.sni.append(name)

    def record_token_auth(self, valid):
        with self.lock:
            if valid:
                self.token_auth_valid += 1

    def snapshot(self):
        with self.lock:
            return {
                "hits": dict(self.hits),
                "sni": list(self.sni),
                "token_auth_valid": self.token_auth_valid,
            }


class Scenario:
    def __init__(self, repo, work, binaries, resolver_library):
        self.repo = repo
        self.work = work
        self.binaries = binaries
        self.resolver_library = resolver_library
        self.processes = []
        self.servers = []
        self.traps = []
        self.results = []
        self.state = OidcState()
        self.env = dict(os.environ)
        for name in [
            "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
            "http_proxy", "https_proxy", "all_proxy", "no_proxy",
        ]:
            self.env.pop(name, None)
        self.env.update(
            EDGION_ADMIN_TOKEN=ADMIN_TOKEN,
            EDGION_GATEWAY_ADMIN_TOKEN=ADMIN_TOKEN,
            EDGION_TOKEN=ADMIN_TOKEN,
            EDGION_ALLOW_INTEGRATION_TESTING_MODE="1",
        )
        (work / "conf").mkdir()
        (work / "logs").mkdir()
        (work / "config").mkdir()
        (work / "config" / "crd").symlink_to(repo / "config" / "crd", target_is_directory=True)
        self.ports = {}
        reservations = []
        for key in [
            "admin", "grpc", "probe", "controller_metrics", "gw_admin",
            "gw_probe", "metrics", "http",
        ]:
            reservation = socket.socket()
            reservation.bind(("127.0.0.1", 0))
            reservations.append(reservation)
            self.ports[key] = reservation.getsockname()[1]
        for reservation in reservations:
            reservation.close()

    def run(self, arguments, check=True, env=None, timeout=60, input_bytes=None):
        result = subprocess.run(
            arguments,
            input=input_bytes,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=env or self.env,
            timeout=timeout,
        )
        output = result.stdout.decode("utf-8", errors="replace")
        with (self.work / "commands.log").open("a") as log:
            log.write(f"{arguments[0]}: exit {result.returncode}\n{output}\n")
        if check:
            require(result.returncode == 0, f"{arguments[0]} failed: {output}")
        return result.stdout

    def generate_crypto(self):
        certs = self.work / "certs" / "oidc"
        certs.mkdir(parents=True)
        self.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
            "-subj", "/CN=oidc-egress-ca", "-keyout", str(certs / "ca.key"),
            "-out", str(certs / "ca.crt"),
        ])
        self.run([
            "openssl", "req", "-newkey", "rsa:2048", "-nodes", "-subj", f"/CN={IDP_HOST}",
            "-keyout", str(certs / "server.key"), "-out", str(certs / "server.csr"),
        ])
        extension = certs / "server.ext"
        extension.write_text(
            "basicConstraints=CA:FALSE\n"
            "keyUsage=digitalSignature,keyEncipherment\n"
            "extendedKeyUsage=serverAuth\n"
            f"subjectAltName=DNS:{IDP_HOST}\n"
        )
        self.run([
            "openssl", "x509", "-req", "-in", str(certs / "server.csr"),
            "-CA", str(certs / "ca.crt"), "-CAkey", str(certs / "ca.key"),
            "-CAcreateserial", "-days", "2", "-extfile", str(extension),
            "-out", str(certs / "server.crt"),
        ])
        self.run([
            "openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048",
            "-pkeyopt", "rsa_keygen_pubexp:65537", "-out", str(certs / "jwt.key"),
        ])
        modulus_output = self.run([
            "openssl", "rsa", "-in", str(certs / "jwt.key"), "-noout", "-modulus",
        ]).decode("ascii").strip()
        require(modulus_output.startswith("Modulus="), "unable to read JWT RSA modulus")
        modulus = bytes.fromhex(modulus_output.split("=", 1)[1])
        self.state.jwk_n = b64url(modulus)
        self.jwt_key = certs / "jwt.key"
        return certs

    def sign_jwt(self):
        header = b64url(json.dumps(
            {"alg": "RS256", "typ": "JWT", "kid": JWT_KID}, separators=(",", ":"),
        ).encode())
        payload = b64url(json.dumps({
            "iss": self.state.issuer,
            "sub": "oidc-egress-user",
            "scope": "api:read",
            "exp": int(time.time()) + 3600,
        }, separators=(",", ":")).encode())
        signing_input = f"{header}.{payload}".encode("ascii")
        signature = self.run(
            ["openssl", "dgst", "-sha256", "-sign", str(self.jwt_key)],
            input_bytes=signing_input,
        )
        self.state.jwt = f"{header}.{payload}.{b64url(signature)}"

    def start_fixture_servers(self, certs):
        state = self.state

        class OidcHandler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def send_json(handler, status, value):
                body = json.dumps(value, separators=(",", ":")).encode()
                handler.send_response(status)
                handler.send_header("Content-Type", "application/json")
                handler.send_header("Content-Length", str(len(body)))
                handler.end_headers()
                handler.wfile.write(body)

            def do_GET(handler):
                path = urlparse(handler.path).path
                state.record_hit(path)
                if path.endswith("/.well-known/openid-configuration"):
                    parts = path.strip("/").split("/")
                    require(len(parts) >= 3, f"invalid discovery fixture path: {path}")
                    phase = parts[1]
                    jwks_host = BLOCKED_JWKS_HOST if phase.endswith("blocked-jwks") else IDP_HOST
                    token_host = BLOCKED_TOKEN_HOST if phase.endswith("blocked-token") else IDP_HOST
                    base = f"https://{IDP_HOST}:{state.port}/oidc/{phase}"
                    handler.send_json(200, {
                        "issuer": state.issuer,
                        "authorization_endpoint": f"{base}/authorize",
                        "token_endpoint": f"https://{token_host}:{state.port}/oidc/{phase}/token",
                        "jwks_uri": f"https://{jwks_host}:{state.port}/oidc/{phase}/jwks",
                    })
                    return
                if path.endswith("/jwks"):
                    handler.send_json(200, {"keys": [{
                        "kty": "RSA", "kid": JWT_KID, "use": "sig", "alg": "RS256",
                        "n": state.jwk_n, "e": "AQAB",
                    }]})
                    return
                if path == "/health":
                    handler.send_json(200, {"ready": True})
                    return
                handler.send_json(404, {"error": "not_found"})

            def do_POST(handler):
                path = urlparse(handler.path).path
                state.record_hit(path)
                length = int(handler.headers.get("Content-Length", "0"))
                require(length <= 65536, "OIDC fixture request exceeded cap")
                handler.rfile.read(length)
                if path.endswith("/token"):
                    expected = "Basic " + base64.b64encode(
                        f"test-client:{CLIENT_SECRET}".encode()
                    ).decode()
                    state.record_token_auth(handler.headers.get("Authorization") == expected)
                    handler.send_json(200, {
                        "access_token": state.jwt,
                        "token_type": "Bearer",
                        "expires_in": 600,
                    })
                    return
                handler.send_json(404, {"error": "not_found"})

            def log_message(handler, *_args):
                pass

        idp = http.server.ThreadingHTTPServer(("127.0.0.1", 0), OidcHandler)
        self.state.port = idp.server_port
        self.state.issuer = f"https://{IDP_HOST}:{idp.server_port}/oidc"
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certs / "server.crt", certs / "server.key")
        context.set_servername_callback(lambda _socket, name, _context: state.record_sni(name))
        idp.socket = context.wrap_socket(idp.socket, server_side=True)
        threading.Thread(target=idp.serve_forever, daemon=True).start()
        self.servers.append(idp)

        denied = ConnectionTrap(socket.AF_INET6, "::", idp.server_port)
        self.traps.append(denied)
        self.denied_baseline = denied.calibrate("::")
        self.denied_trap = denied

        proxy = ConnectionTrap(socket.AF_INET, "127.0.0.1")
        self.traps.append(proxy)
        self.proxy_baseline = proxy.calibrate("127.0.0.1")
        self.proxy_trap = proxy

        class BackendHandler(http.server.BaseHTTPRequestHandler):
            def do_GET(handler):
                valid = handler.headers.get("X-User-ID") == "oidc-egress-user"
                body = b"oidc-egress-authenticated" if valid else b"missing OIDC identity"
                handler.send_response(200 if valid else 409)
                handler.send_header("Content-Length", str(len(body)))
                handler.end_headers()
                handler.wfile.write(body)

            def log_message(handler, *_args):
                pass

        backend = http.server.ThreadingHTTPServer(("127.0.0.1", 0), BackendHandler)
        threading.Thread(target=backend.serve_forever, daemon=True).start()
        self.servers.append(backend)
        self.backend_port = backend.server_port
        self.sign_jwt()

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
        value = {"apiVersion": api, "kind": kind, "metadata": metadata, **fields}
        if spec is not None:
            value["spec"] = spec
        directory = self.work / ("updates" if self.processes else "conf")
        directory.mkdir(exist_ok=True)
        path = directory / f"{kind}_{metadata.get('namespace', '')}_{name}.yaml"
        path.write_text(json.dumps(value, indent=2))
        return path

    def create_base_resources(self, certs):
        self.resource("GatewayClass", "oidc-egress", {
            "controllerName": "edgion.io/gateway-controller",
            "parametersRef": {"group": "edgion.io", "kind": "EdgionGatewayConfig", "name": "oidc-egress"},
        })
        self.resource("EdgionGatewayConfig", "oidc-egress", {})
        self.resource("Gateway", "oidc-egress", {
            "gatewayClassName": "oidc-egress",
            "listeners": [{"name": "http", "protocol": "HTTP", "port": self.ports["http"]}],
        })
        self.resource("Service", "oidc-egress", {"ports": [{"name": "http", "port": self.backend_port}]})
        self.resource(
            "EndpointSlice", "oidc-egress",
            labels={"kubernetes.io/service-name": "oidc-egress"},
            addressType="IPv4",
            ports=[{"name": "http", "port": self.backend_port, "protocol": "TCP"}],
            endpoints=[{"addresses": ["127.0.0.1"], "conditions": {"ready": True}}],
        )
        self.resource("HTTPRoute", "oidc-egress", {
            "parentRefs": [{"name": "oidc-egress"}],
            "hostnames": [ROUTE_HOST],
            "rules": [{
                "filters": [{"type": "ExtensionRef", "extensionRef": {
                    "group": "edgion.io", "kind": "EdgionPlugins", "name": "oidc-egress",
                }}],
                "backendRefs": [{"name": "oidc-egress", "port": self.backend_port}],
            }],
        })
        self.resource("Secret", "oidc-egress-client", type="Opaque", data={
            "clientSecret": base64.b64encode(CLIENT_SECRET.encode()).decode(),
        })
        self.resource("Secret", "oidc-egress-session", type="Opaque", data={
            "sessionSecret": base64.b64encode(SESSION_SECRET.encode()).decode(),
        })
        self.resource("Secret", "oidc-egress-ca", type="Opaque", data={
            "ca.crt": base64.b64encode((certs / "ca.crt").read_bytes()).decode(),
        })
        self.plugin_resource("initial-default-full", san=False)

    def plugin_resource(self, phase, san, discovery_host=IDP_HOST):
        validation = {"caCertificateRefs": [{"kind": "Secret", "name": "oidc-egress-ca"}]}
        if san:
            validation["subjectAltNames"] = [{"type": "Hostname", "hostname": IDP_HOST}]
        config = {
            "discovery": f"https://{discovery_host}:{self.state.port}/oidc/{phase}/.well-known/openid-configuration",
            "clientId": "test-client",
            "clientSecretRef": {"name": "oidc-egress-client"},
            "sessionSecretRef": {"name": "oidc-egress-session"},
            "unauthAction": "Auth",
            "verificationMode": "JwksOnly",
            "useJwks": True,
            "tokenSigningAlg": "RS256",
            "allowedSigningAlgs": ["RS256"],
            "requiredScopes": ["api:read"],
            "claimsToHeaders": {"sub": "X-User-ID"},
            "timeout": 3,
            "realm": phase,
            "tls": {"verify": True, "validation": validation},
        }
        return self.resource("EdgionPlugins", "oidc-egress", {
            "requestPlugins": [{"enable": True, "type": "OpenidConnect", "config": config}],
        })

    def start_processes(self):
        ports = self.ports
        controller = f"""work_dir: {json.dumps(str(self.work))}
server:
  grpc_listen: "127.0.0.1:{ports['grpc']}"
  admin_listen: "127.0.0.1:{ports['admin']}"
  probe_listen: "127.0.0.1:{ports['probe']}"
  metrics_listen: "127.0.0.1:{ports['controller_metrics']}"
conf_center:
  type: file_system
  conf_dir: {json.dumps(str(self.work / 'conf'))}
logging:
  log_dir: {json.dumps(str(self.work / 'logs'))}
"""
        gateway = f"""work_dir: {json.dumps(str(self.work))}
logging:
  log_dir: {json.dumps(str(self.work / 'logs'))}
metrics:
  listen: "127.0.0.1:{ports['metrics']}"
"""
        for name, config, security in [("controller", controller, "server"), ("gateway", gateway, "client")]:
            path = self.work / f"{name}.yaml"
            path.write_text(config + (self.work / f"{security}_security.yaml").read_text())
            arguments = [str(self.binaries / f"edgion-{name}"), "-c", str(path), "--work-dir", str(self.work)]
            env = dict(self.env)
            if name == "gateway":
                proxy_url = f"http://127.0.0.1:{self.proxy_trap.port}"
                env.update({
                    "HTTP_PROXY": proxy_url, "HTTPS_PROXY": proxy_url, "ALL_PROXY": proxy_url,
                    "http_proxy": proxy_url, "https_proxy": proxy_url, "all_proxy": proxy_url,
                    "NO_PROXY": "", "no_proxy": "",
                })
                if sys.platform == "darwin":
                    env["DYLD_INSERT_LIBRARIES"] = str(self.resolver_library)
                else:
                    env["LD_PRELOAD"] = str(self.resolver_library)
                arguments += [
                    "--server-addr", f"https://127.0.0.1:{ports['grpc']}",
                    "--integration-testing-mode",
                    "--admin-listen", f"127.0.0.1:{ports['gw_admin']}",
                    "--probe-listen", f"127.0.0.1:{ports['gw_probe']}",
                ]
            with (self.work / "logs" / f"{name}.log").open("w") as log:
                process = subprocess.Popen(arguments, env=env, stdout=log, stderr=subprocess.STDOUT)
            self.processes.append(process)

            def ready():
                require(process.poll() is None, f"{name} exited ({process.returncode}); see logs/{name}.log")
                probe = ports["probe" if name == "controller" else "gw_probe"]
                return request(probe, "/ready")[0] == 200

            wait_for(f"{name} readiness", ready)
            if name == "controller":
                wait_for(
                    "controller metrics readiness",
                    lambda: request(ports["controller_metrics"], "/metrics")[0] == 200,
                )

    def metrics(self):
        status, _, body = request(self.ports["metrics"], "/metrics")
        require(status == 200, "metrics endpoint unavailable")
        values = {}
        for line in body.decode().splitlines():
            if line and not line.startswith("#"):
                key, value, *_ = line.split()
                values[key] = float(value)
        return values

    def gateway_request(self, path, method="GET", extra_headers=None, body=None):
        headers = {"Host": ROUTE_HOST, "Connection": "close"}
        if extra_headers:
            headers.update(extra_headers)
        return request(self.ports["http"], path, method, headers, body)

    def apply_plugin(self, phase, san, discovery_host=IDP_HOST):
        path = self.plugin_resource(phase, san, discovery_host)
        self.run([
            str(self.binaries / "edgion-cli"), "--server", f"http://127.0.0.1:{self.ports['admin']}",
            "apply", "-f", str(path),
        ])

    def begin_auth(self, phase):
        status, headers, body = self.gateway_request("/protected")
        if status != 302:
            return None
        locations = header_values(headers, "location")
        if len(locations) != 1 or f"/oidc/{phase}/authorize" not in locations[0]:
            return None
        state_cookie = cookie_value(headers, "edgion_oidc_state")
        state = parse_qs(urlparse(locations[0]).query).get("state", [None])[0]
        if not state_cookie or not state:
            return None
        require(body == b"", f"{phase}: authorization redirect carried a body")
        return state, state_cookie

    def assert_public_failure(self, phase, response):
        status, headers, body = response
        require(status == 502, f"{phase}: expected 502, got {status}: {body!r}")
        require(body == PUBLIC_FAILURE_BODY, f"{phase}: non-static public failure body: {body!r}")
        public = body + json.dumps(headers).encode()
        sensitive = [
            IDP_HOST, BLOCKED_DISCOVERY_HOST, BLOCKED_JWKS_HOST, BLOCKED_TOKEN_HOST,
            "127.0.0.1", "0.0.0.0", "::", "proxy", CLIENT_SECRET, SESSION_SECRET,
            self.state.jwt, "oidc-egress-user", "edgion_oidc_state", "cookie",
        ]
        for value in sensitive:
            require(value.encode() not in public, f"{phase}: sensitive detail in public response")

    def assert_transport_isolation(self, phase, metric_before, expected_drops):
        require(
            self.proxy_trap.count() == self.proxy_baseline,
            f"{phase}: Gateway contacted proxy-environment trap",
        )
        require(
            self.denied_trap.count() == self.denied_baseline,
            f"{phase}: Gateway contacted denied DNS candidate",
        )
        after = self.metrics()
        metric = "edgion_outbound_egress_blocked_total"
        delta = metric_value(after, metric, reason="unspecified") - metric_value(
            metric_before, metric, reason="unspecified"
        )
        require(delta == expected_drops, f"{phase}: expected {expected_drops} DNS drops, got {delta}")
        return int(delta)

    def exercise_full_flow(self, phase, san):
        metric_before = self.metrics()
        snapshot_before = self.state.snapshot()
        self.apply_plugin(phase, san)
        state_value, state_cookie = wait_for(
            f"{phase} plugin generation", lambda: self.begin_auth(phase),
        )
        callback = f"/.edgion/oidc/callback?code=allowed-code&state={state_value}"
        status, headers, body = self.gateway_request(
            callback, extra_headers={"Cookie": f"edgion_oidc_state={state_cookie}"},
        )
        require(status == 302 and body == b"", f"{phase}: callback failed: {status} {body!r}")
        session_cookie = cookie_value(headers, "edgion_oidc_session")
        require(session_cookie, f"{phase}: callback did not establish an OIDC session")
        status, _, body = self.gateway_request(
            "/protected", extra_headers={"Cookie": f"edgion_oidc_session={session_cookie}"},
        )
        require(status == 200, f"{phase}: authenticated request failed: {status} {body!r}")
        require(body == b"oidc-egress-authenticated", f"{phase}: OIDC identity did not reach backend")

        snapshot_after = self.state.snapshot()
        token_path = f"/oidc/{phase}/token"
        jwks_path = f"/oidc/{phase}/jwks"
        require(
            snapshot_after["hits"].get(token_path, 0) > snapshot_before["hits"].get(token_path, 0),
            f"{phase}: token endpoint was not reached",
        )
        require(
            snapshot_after["hits"].get(jwks_path, 0) > snapshot_before["hits"].get(jwks_path, 0),
            f"{phase}: JWKS endpoint was not reached",
        )
        require(
            snapshot_after["token_auth_valid"] > snapshot_before["token_auth_valid"],
            f"{phase}: token endpoint did not receive valid client authentication",
        )
        new_sni = snapshot_after["sni"][len(snapshot_before["sni"]):]
        require(new_sni and all(name == IDP_HOST for name in new_sni), f"{phase}: original-hostname SNI lost: {new_sni}")
        drops = self.assert_transport_isolation(phase, metric_before, 1)
        self.record_result(phase, "full", san, status, drops)

    def exercise_blocked_discovery(self, phase, san):
        metric_before = self.metrics()
        snapshot_before = self.state.snapshot()
        self.apply_plugin(phase, san, BLOCKED_DISCOVERY_HOST)
        response = wait_for(
            f"{phase} blocked discovery generation",
            lambda: (candidate if (candidate := self.gateway_request("/protected"))[0] == 502 else None),
        )
        self.assert_public_failure(phase, response)
        snapshot_after = self.state.snapshot()
        require(snapshot_after["hits"] == snapshot_before["hits"], f"{phase}: blocked discovery reached IdP")
        drops = self.assert_transport_isolation(phase, metric_before, 2)
        self.record_result(phase, "blocked-discovery", san, 502, drops)

    def exercise_blocked_jwks(self, phase, san):
        metric_before = self.metrics()
        snapshot_before = self.state.snapshot()
        self.apply_plugin(phase, san)
        wait_for(f"{phase} plugin generation", lambda: self.begin_auth(phase))
        response = self.gateway_request(
            "/protected", extra_headers={"Authorization": f"Bearer {self.state.jwt}"},
        )
        self.assert_public_failure(phase, response)
        snapshot_after = self.state.snapshot()
        blocked_path = f"/oidc/{phase}/jwks"
        require(
            snapshot_after["hits"].get(blocked_path, 0) == snapshot_before["hits"].get(blocked_path, 0),
            f"{phase}: blocked JWKS endpoint was contacted",
        )
        drops = self.assert_transport_isolation(phase, metric_before, 3)
        self.record_result(phase, "blocked-jwks", san, 502, drops)

    def exercise_blocked_token(self, phase, san):
        metric_before = self.metrics()
        snapshot_before = self.state.snapshot()
        self.apply_plugin(phase, san)
        state_value, state_cookie = wait_for(
            f"{phase} plugin generation", lambda: self.begin_auth(phase),
        )
        callback = f"/.edgion/oidc/callback?code=blocked-code&state={state_value}"
        response = self.gateway_request(
            callback, extra_headers={"Cookie": f"edgion_oidc_state={state_cookie}"},
        )
        self.assert_public_failure(phase, response)
        snapshot_after = self.state.snapshot()
        blocked_path = f"/oidc/{phase}/token"
        require(
            snapshot_after["hits"].get(blocked_path, 0) == snapshot_before["hits"].get(blocked_path, 0),
            f"{phase}: blocked token endpoint was contacted",
        )
        drops = self.assert_transport_isolation(phase, metric_before, 3)
        self.record_result(phase, "blocked-token", san, 502, drops)

    def record_result(self, phase, kind, san, status, drops):
        self.results.append({
            "phase": phase, "kind": kind, "san_pinning": san,
            "status": status, "rejected_candidates": drops,
        })
        (self.work / "results.json").write_text(json.dumps(self.results, indent=2))
        print(
            f"PASS {phase}: kind={kind}, san_pinning={san}, status={status}, rejected={drops}",
            flush=True,
        )

    def setup(self):
        certs = self.generate_crypto()
        self.start_fixture_servers(certs)
        self.create_base_resources(certs)
        self.start_processes()

    def exercise(self):
        for branch, san in [("default", False), ("san", True)]:
            self.exercise_full_flow(f"{branch}-full", san)
            self.exercise_blocked_discovery(f"{branch}-blocked-discovery", san)
            self.exercise_blocked_jwks(f"{branch}-blocked-jwks", san)
            self.exercise_blocked_token(f"{branch}-blocked-token", san)
        print(f"PASS OIDC egress: {len(self.results)} phases; artifacts: {self.work}")

    def close(self):
        for process in reversed(self.processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        for server in self.servers:
            server.shutdown()
            server.server_close()
        for trap in self.traps:
            trap.close()


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    scenario = Scenario(*(Path(argument) for argument in sys.argv[1:]))
    try:
        scenario.setup()
        scenario.exercise()
    finally:
        scenario.close()
