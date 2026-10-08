#!/usr/bin/env python3
"""Real-process UDP shutdown coverage for signals, policy fatal, and startup failure."""

import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time


ROOT = Path(__file__).resolve().parents[4]
UTILS = ROOT / "edgion-tests/integration/scripts/utils"
TOKEN = "integration-udp-shutdown-token-0123456789"
NAMESPACE = "udp-shutdown"
GRACE_SECONDS = 2
AUXILIARY_DRAIN_SECONDS = 5


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def write_json_atomic(path, value):
    replacement = path.with_suffix(path.suffix + ".tmp")
    write_json(replacement, value)
    replacement.replace(path)


def http_status(port, path):
    request = f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n".encode()
    with socket.create_connection(("127.0.0.1", port), timeout=0.5) as connection:
        connection.settimeout(0.5)
        connection.sendall(request)
        response = connection.recv(128)
    fields = response.split(b"\r\n", 1)[0].split()
    return int(fields[1]) if len(fields) >= 2 else 0


def reserve_tcp_port():
    reservation = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    reservation.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    reservation.bind(("127.0.0.1", 0))
    return reservation, reservation.getsockname()[1]


def reserve_udp_port():
    reservation = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    reservation.bind(("0.0.0.0", 0))
    return reservation, reservation.getsockname()[1]


def require_fresh(binary, source_roots):
    binary_mtime = binary.stat().st_mtime
    for source_root in source_roots:
        for source in source_root.rglob("*.rs"):
            require(
                source.stat().st_mtime <= binary_mtime,
                f"{binary} is older than {source}; rebuild the integration binaries",
            )


class StreamingUdpBackend:
    """Echo requests and keep each upstream receiver active with periodic datagrams."""

    def __init__(self, reservation):
        self.socket = reservation
        self.socket.settimeout(0.05)
        self.peers = set()
        self.stopping = threading.Event()
        self.thread = threading.Thread(target=self._run, name="udp-shutdown-backend", daemon=True)

    def start(self):
        self.thread.start()

    def _run(self):
        sequence = 0
        next_stream = time.monotonic()
        while not self.stopping.is_set():
            try:
                payload, peer = self.socket.recvfrom(65535)
                self.peers.add(peer)
                self.socket.sendto(payload, peer)
            except socket.timeout:
                pass
            except OSError:
                if not self.stopping.is_set():
                    raise
                break
            now = time.monotonic()
            if now >= next_stream:
                packet = f"stream-{sequence}".encode()
                for peer in tuple(self.peers):
                    try:
                        self.socket.sendto(packet, peer)
                    except OSError:
                        if not self.stopping.is_set():
                            raise
                sequence += 1
                next_stream = now + 0.05

    def stop(self):
        self.stopping.set()
        self.socket.close()
        self.thread.join(timeout=2)


class UdpClient:
    def __init__(self, port):
        self.port = port
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.socket.bind(("127.0.0.1", 0))
        self.socket.settimeout(0.1)

    def close(self):
        self.socket.close()

    def echo(self, payload, timeout=1.0):
        self.socket.sendto(payload, ("127.0.0.1", self.port))
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                received, _ = self.socket.recvfrom(65535)
            except socket.timeout:
                continue
            if received == payload:
                return True
        return False

    def receives_stream(self, timeout=1.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                received, _ = self.socket.recvfrom(65535)
            except socket.timeout:
                continue
            if received.startswith(b"stream-"):
                return True
        return False


class Topology:
    def __init__(self, root, label, binaries, cert_source, include_http=False, blocked_listener=False):
        self.root = root
        self.label = label
        self.work = root / label
        self.binaries = binaries
        self.cert_source = cert_source
        self.include_http = include_http
        self.blocked_listener = blocked_listener
        self.processes = []
        self.gateway = None
        self.controller = None
        self.clients = []
        self.backend = None
        self.reservations = {}
        self.ports = {}
        self.gateway_resource_path = self.work / "conf/Gateway_udp-shutdown_wire.yaml"
        self.gateway_resource = None
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("EDGION_")}
        self.env.update(
            EDGION_ADMIN_TOKEN=TOKEN,
            EDGION_GATEWAY_ADMIN_TOKEN=TOKEN,
            EDGION_ALLOW_INTEGRATION_TESTING_MODE="1",
        )

    def reserve_ports(self):
        for name in (
            "ctrl_admin",
            "ctrl_grpc",
            "ctrl_probe",
            "ctrl_metrics",
            "gw_admin",
            "gw_probe",
            "gw_metrics",
            "http",
        ):
            reservation, port = reserve_tcp_port()
            self.reservations[name] = reservation
            self.ports[name] = port
        for name in ("udp_a", "udp_b", "backend"):
            reservation, port = reserve_udp_port()
            self.reservations[name] = reservation
            self.ports[name] = port

    def release(self, name):
        reservation = self.reservations.pop(name, None)
        if reservation is not None:
            reservation.close()

    def prepare(self):
        self.work.mkdir()
        (self.work / "conf").mkdir()
        (self.work / "logs").mkdir()
        (self.work / "config").mkdir()
        (self.work / "config/crd").symlink_to(ROOT / "config/crd", target_is_directory=True)
        shutil.copytree(self.cert_source, self.work / "certs")
        for transport in ("server-tls.yaml", "client-tls.yaml"):
            shutil.copy2(self.cert_source.parent / transport, self.work / transport)
        self.reserve_ports()
        self.write_resources()
        self.write_process_configs()

    def write_resources(self):
        gateway_class = {
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GatewayClass",
            "metadata": {"name": "udp-shutdown"},
            "spec": {"controllerName": "edgion.io/gateway-controller"},
        }
        listeners = [
            {"name": "udp-a", "protocol": "UDP", "port": self.ports["udp_a"]},
            {"name": "udp-b", "protocol": "UDP", "port": self.ports["udp_b"]},
        ]
        if self.include_http:
            listeners.append({"name": "control", "protocol": "HTTP", "port": self.ports["http"]})
        self.gateway_resource = {
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "Gateway",
            "metadata": {"name": "wire", "namespace": NAMESPACE},
            "spec": {"gatewayClassName": "udp-shutdown", "listeners": listeners},
        }
        service = {
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {"name": "echo", "namespace": NAMESPACE},
            "spec": {
                "ports": [
                    {
                        "name": "udp",
                        "port": self.ports["backend"],
                        "targetPort": self.ports["backend"],
                        "protocol": "UDP",
                    }
                ]
            },
        }
        endpoint_slice = {
            "apiVersion": "discovery.k8s.io/v1",
            "kind": "EndpointSlice",
            "metadata": {
                "name": "echo",
                "namespace": NAMESPACE,
                "labels": {"kubernetes.io/service-name": "echo"},
            },
            "addressType": "IPv4",
            "ports": [{"name": "udp", "port": self.ports["backend"], "protocol": "UDP"}],
            "endpoints": [{"addresses": ["127.0.0.1"], "conditions": {"ready": True}}],
        }
        write_json(self.work / "conf/GatewayClass__udp-shutdown.yaml", gateway_class)
        write_json(self.gateway_resource_path, self.gateway_resource)
        write_json(self.work / "conf/Service_udp-shutdown_echo.yaml", service)
        write_json(self.work / "conf/EndpointSlice_udp-shutdown_echo.yaml", endpoint_slice)
        for listener in ("udp-a", "udp-b"):
            route = {
                "apiVersion": "gateway.networking.k8s.io/v1alpha2",
                "kind": "UDPRoute",
                "metadata": {"name": listener, "namespace": NAMESPACE},
                "spec": {
                    "parentRefs": [{"name": "wire", "sectionName": listener}],
                    "rules": [
                        {
                            "backendRefs": [
                                {"name": "echo", "port": self.ports["backend"], "weight": 100}
                            ]
                        }
                    ],
                },
            }
            write_json(self.work / f"conf/UDPRoute_udp-shutdown_{listener}.yaml", route)

    def write_process_configs(self):
        controller = f"""work_dir: {self.work}
server:
  admin_listen: 127.0.0.1:{self.ports['ctrl_admin']}
  grpc_listen: 127.0.0.1:{self.ports['ctrl_grpc']}
  probe_listen: 127.0.0.1:{self.ports['ctrl_probe']}
  metrics_listen: 127.0.0.1:{self.ports['ctrl_metrics']}
logging:
  log_dir: {self.work / 'logs'}
  console: false
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: {self.work / 'conf'}
conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
"""
        gateway = f"""work_dir: {self.work}
logging:
  log_dir: {self.work / 'logs'}
  console: false
server:
  threads: 1
  grace_period_seconds: {GRACE_SECONDS}
  graceful_shutdown_timeout_seconds: 0
metrics:
  listen: 127.0.0.1:{self.ports['gw_metrics']}
udp_log:
  enabled: true
  output:
    localFile:
      path: {self.work / 'udp-access.log'}
      # Test-only sink: shutdown assertions require terminal records regardless
      # of host disk utilization. Actual write failures still fail assertions.
      diskUsageThresholdPercent: 0
      rotation:
        strategy: never
        maxFiles: 1
        checkIntervalSecs: 1
"""
        (self.work / "controller.yaml").write_text(
            controller + (self.work / "server-tls.yaml").read_text(encoding="utf-8"), encoding="utf-8"
        )
        (self.work / "gateway.yaml").write_text(
            gateway + (self.work / "client-tls.yaml").read_text(encoding="utf-8"), encoding="utf-8"
        )

    def start_process(self, name, arguments):
        log_path = self.work / f"{name}.log"
        log = log_path.open("wb")
        process = subprocess.Popen(
            [str(self.binaries[name]), *arguments],
            cwd=self.work,
            env=self.env,
            stdout=log,
            stderr=log,
        )
        log.close()
        self.processes.append(process)
        return process

    def start_controller(self):
        for name in ("ctrl_admin", "ctrl_grpc", "ctrl_probe", "ctrl_metrics"):
            self.release(name)
        self.controller = self.start_process(
            "controller",
            ["-c", str(self.work / "controller.yaml"), "--work-dir", str(self.work)],
        )
        self.wait(
            "Controller readiness",
            lambda: http_status(self.ports["ctrl_probe"], "/ready") == 200,
            process=self.controller,
            timeout=30,
        )

    def start_backend(self):
        backend_socket = self.reservations.pop("backend")
        self.backend = StreamingUdpBackend(backend_socket)
        self.backend.start()

    def start_gateway(self):
        for name in ("gw_admin", "gw_probe", "gw_metrics", "udp_a", "http"):
            self.release(name)
        if not self.blocked_listener:
            self.release("udp_b")
        self.gateway = self.start_process(
            "gateway",
            [
                "-c",
                str(self.work / "gateway.yaml"),
                "--work-dir",
                str(self.work),
                "--server-addr",
                f"https://127.0.0.1:{self.ports['ctrl_grpc']}",
                "--integration-testing-mode",
                "--admin-listen",
                f"127.0.0.1:{self.ports['gw_admin']}",
                "--probe-listen",
                f"127.0.0.1:{self.ports['gw_probe']}",
            ],
        )

    def wait(self, label, predicate, process=None, timeout=15):
        deadline = time.monotonic() + timeout
        last_error = None
        while time.monotonic() < deadline:
            if process is not None and process.poll() is not None:
                raise AssertionError(
                    f"{label}: process exited with {process.returncode}; artifacts: {self.work}"
                )
            try:
                if predicate():
                    return
            except (OSError, ValueError) as error:
                last_error = error
            time.sleep(0.05)
        detail = f": {last_error}" if last_error else ""
        raise AssertionError(f"Timed out waiting for {label}{detail}; artifacts: {self.work}")

    def wait_gateway_ready(self):
        self.wait(
            "Gateway readiness",
            lambda: http_status(self.ports["gw_probe"], "/ready") == 200,
            process=self.gateway,
            timeout=30,
        )
        for name in ("udp_a", "udp_b"):
            client = UdpClient(self.ports[name])
            self.clients.append(client)
            payload = f"ready-{self.label}-{name}".encode()
            self.wait(
                f"{name} UDP echo",
                lambda client=client, payload=payload: client.echo(payload, 0.3),
                process=self.gateway,
                timeout=15,
            )
            require(client.receives_stream(), f"{name} did not receive the active upstream response stream")

    def wait_log_text(self, text, timeout=15):
        log_path = self.work / "logs/edgion-gateway.log"
        self.wait(
            f"Gateway log marker {text!r}",
            lambda: log_path.exists() and text in log_path.read_text(encoding="utf-8", errors="replace"),
            process=self.gateway,
            timeout=timeout,
        )

    def assert_udp_active(self, label):
        for index, client in enumerate(self.clients):
            payload = f"{label}-{index}-{time.monotonic_ns()}".encode()
            require(client.echo(payload), f"UDP listener {index} stopped before post-Pingora drain")

    def assert_udp_stopped(self):
        for name in ("udp_a", "udp_b"):
            probe = UdpClient(self.ports[name])
            try:
                for attempt in range(2):
                    payload = f"stopped-{name}-{attempt}-{time.monotonic_ns()}".encode()
                    require(
                        not probe.echo(payload, 0.4),
                        f"{name} still forwarded after post-Pingora drain began",
                    )
            finally:
                probe.close()

    def wait_exit(self, expected_zero, timeout=AUXILIARY_DRAIN_SECONDS + 7):
        try:
            return_code = self.gateway.wait(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise AssertionError(f"Gateway did not exit within {timeout}s; artifacts: {self.work}") from error
        if expected_zero:
            require(return_code == 0, f"Gateway exited {return_code}, expected success; artifacts: {self.work}")
        else:
            require(return_code != 0, f"Gateway exited successfully, expected failure; artifacts: {self.work}")

    def wait_ports_released_before_exit(self, names):
        def ports_are_available():
            checks = []
            try:
                for name in names:
                    check = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                    checks.append(check)
                    check.bind(("0.0.0.0", self.ports[name]))
                return True
            except OSError:
                return False
            finally:
                for check in checks:
                    check.close()

        self.wait(
            f"UDP ports {', '.join(names)} to be released before process completion",
            ports_are_available,
            process=self.gateway,
            timeout=4,
        )

    def wait_for_terminal_logs(self):
        log_path = self.work / "udp-access.log"

        def matching_ports():
            if not log_path.exists():
                return set()
            matched = set()
            for line in log_path.read_text(encoding="utf-8", errors="replace").splitlines():
                try:
                    entry = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if entry.get("packets_sent", 0) > 0 and entry.get("packets_received", 0) > 0:
                    matched.add(entry.get("listener_port"))
            return matched

        wanted = {self.ports["udp_a"], self.ports["udp_b"]}
        self.wait(
            "terminal UDP records reaching the local-file sink",
            lambda: wanted.issubset(matching_ports()),
            process=self.gateway,
            timeout=4,
        )

    def trigger_policy_fatal(self):
        updated = json.loads(json.dumps(self.gateway_resource))
        updated["metadata"]["annotations"] = {
            "edgion.io/inbound-proxy-protocol": json.dumps(
                {"source": "inline", "config": {"mode": "required"}}, separators=(",", ":")
            )
        }
        write_json_atomic(self.gateway_resource_path, updated)

    def cleanup(self):
        for client in self.clients:
            client.close()
        self.clients.clear()
        if self.backend is not None:
            self.backend.stop()
            self.backend = None
        for process in reversed(self.processes):
            if process.poll() is None:
                process.terminate()
        deadline = time.monotonic() + 1
        for process in reversed(self.processes):
            remaining = max(0, deadline - time.monotonic())
            if process.poll() is None:
                try:
                    process.wait(timeout=remaining)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)
        for reservation in self.reservations.values():
            reservation.close()
        self.reservations.clear()


def prepare_certificates(run_root, env):
    command = r'''
set -e
source "$1"
conf_sync_distribute_certs "$2"
conf_sync_security_block server > "$2/server-tls.yaml"
conf_sync_security_block client > "$2/client-tls.yaml"
'''
    subprocess.run(
        ["bash", "-c", command, "udp-shutdown", str(UTILS / "conf_sync_certs.sh"), str(run_root)],
        cwd=ROOT,
        env=env,
        check=True,
        timeout=90,
        stdout=subprocess.DEVNULL,
    )


def run_signal_case(root, binaries, cert_source, name, sent_signal):
    topology = Topology(root, name, binaries, cert_source)
    try:
        topology.prepare()
        topology.start_backend()
        topology.start_controller()
        topology.start_gateway()
        topology.wait_gateway_ready()
        os.kill(topology.gateway.pid, sent_signal)
        if sent_signal == signal.SIGTERM:
            topology.wait_log_text(f"Graceful shutdown: grace period {GRACE_SECONDS}s starts")
            require(topology.gateway.poll() is None, "Gateway exited during the configured SIGTERM grace phase")
            topology.assert_udp_active("sigterm-grace")
        topology.wait_log_text("Pingora stopped, shutting down auxiliary services")
        require(topology.gateway.poll() is None, "Gateway skipped the post-Pingora auxiliary drain")
        topology.assert_udp_stopped()
        topology.wait_for_terminal_logs()
        topology.wait_ports_released_before_exit(("udp_a", "udp_b"))
        topology.wait_exit(expected_zero=True)
    finally:
        topology.cleanup()


def run_policy_fatal_case(root, binaries, cert_source):
    topology = Topology(root, "policy-fatal", binaries, cert_source, include_http=True)
    try:
        topology.prepare()
        topology.start_backend()
        topology.start_controller()
        topology.start_gateway()
        topology.wait_gateway_ready()
        topology.trigger_policy_fatal()
        topology.wait_log_text("Pingora stopped, shutting down auxiliary services")
        topology.assert_udp_stopped()
        topology.wait_for_terminal_logs()
        topology.wait_ports_released_before_exit(("udp_a", "udp_b"))
        topology.wait_exit(expected_zero=False)
        gateway_log = (topology.work / "logs/edgion-gateway.log").read_text(encoding="utf-8", errors="replace")
        require("ListenerPolicyTransition" in gateway_log, "policy-fatal exit lost its original diagnostic")
    finally:
        topology.cleanup()


def run_bind_failure_case(root, binaries, cert_source):
    topology = Topology(root, "later-bind-failure", binaries, cert_source, blocked_listener=True)
    try:
        topology.prepare()
        topology.start_backend()
        topology.start_controller()
        topology.start_gateway()
        topology.wait_log_text(f"addr=0.0.0.0:{topology.ports['udp_a']}")
        topology.wait_ports_released_before_exit(("udp_a",))
        topology.wait_exit(expected_zero=False)
        gateway_log = (topology.work / "logs/edgion-gateway.log").read_text(encoding="utf-8", errors="replace")
        require("Address already in use" in gateway_log, "later bind failure lost the socket error diagnostic")
    finally:
        topology.cleanup()


def main():
    if os.name != "posix" or not hasattr(signal, "SIGTERM") or not hasattr(signal, "SIGINT"):
        print("SKIP: UDP process shutdown signals require a Unix-like platform")
        return 0

    binaries = {
        "controller": Path(os.environ.get("EDGION_CONTROLLER_BIN", ROOT / "target/debug/edgion-controller")),
        "gateway": Path(os.environ.get("EDGION_GATEWAY_BIN", ROOT / "target/debug/edgion-gateway")),
    }
    for binary in binaries.values():
        require(binary.is_file() and os.access(binary, os.X_OK), f"Missing executable: {binary}")
    require_fresh(
        binaries["controller"],
        (
            ROOT / "edgion-controller/src",
            ROOT / "edgion-common/src",
            ROOT / "edgion-resources/src",
        ),
    )
    require_fresh(
        binaries["gateway"],
        (
            ROOT / "edgion-gateway/src",
            ROOT / "edgion-common/src",
            ROOT / "edgion-resources/src",
        ),
    )

    def interrupted(signum, _frame):
        raise KeyboardInterrupt(f"received signal {signum}")

    signal.signal(signal.SIGTERM, interrupted)

    run_base = ROOT / "tmp/integration_testing"
    run_base.mkdir(parents=True, exist_ok=True)
    run_root = Path(tempfile.mkdtemp(prefix="udp-shutdown.", dir=run_base))
    # Preserve the explicit CLI binary override for certificate generation.
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("EDGION_") or key == "EDGION_CLI_BIN"
    }
    success = False
    try:
        prepare_certificates(run_root, env)
        cert_source = run_root / "certs"
        run_signal_case(run_root, binaries, cert_source, "sigterm", signal.SIGTERM)
        run_signal_case(run_root, binaries, cert_source, "sigint", signal.SIGINT)
        run_policy_fatal_case(run_root, binaries, cert_source)
        run_bind_failure_case(run_root, binaries, cert_source)
        success = True
        print("PASS: UDP process shutdown wire coverage")
        return 0
    except (Exception, KeyboardInterrupt) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        print(f"INFO: retained artifacts: {run_root}", file=sys.stderr)
        return 1
    finally:
        if success:
            shutil.rmtree(run_root)


if __name__ == "__main__":
    raise SystemExit(main())
