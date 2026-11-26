#!/usr/bin/env python3
"""Isolated real-process HTTP shutdown acceptance (requires Python h2).

This scenario intentionally owns private Controller/Gateway processes: sending
SIGTERM to the shared integration Gateway would invalidate unrelated suites.
Reuse the UDP scenario's process/certificate helpers without changing its tests.
All listeners use reserved ephemeral ports; only owned PIDs are terminated.
"""

import argparse
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import queue
import signal
import socket
import sys
import tempfile
import threading
import time

from h2.config import H2Configuration
from h2.connection import H2Connection
from h2.events import DataReceived, RequestReceived, ResponseReceived, StreamEnded, StreamReset, TrailersReceived

from test_udp_process_shutdown import (
    ROOT, Topology as ProcessTopology, http_status, prepare_certificates,
    require, reserve_tcp_port, write_json,
)

FIRST_EVENT = b"data: first\n\n"
LAST_EVENT = b"data: last\n\nevent: done\ndata: complete\n\n"
MESSAGES = (b"first-message", b"last-message")


def grpc_frame(payload):
    return b"\x00" + len(payload).to_bytes(4, "big") + payload


class Backend:
    """Controlled streaming fixtures with worker failures forwarded to the caller."""

    def __init__(self, h1_socket, h2_socket):
        self.release = threading.Event()
        self.stopping = threading.Event()
        self.errors = queue.Queue()
        self.threads = []
        self.connections = []
        self.lock = threading.Lock()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            disable_nagle_algorithm = True

            def log_message(self, *_args):
                pass

            def handle(self):
                try:
                    super().handle()
                except ConnectionResetError:
                    # A peer may reset an idle upstream keepalive while the
                    # server reads the next request. Response writes and flushes
                    # stay inside do_GET's error-reporting block below; their
                    # failures must never be mistaken for this normal teardown.
                    self.close_connection = True

            def do_GET(self):
                try:
                    self.send_response(200)
                    if self.path == "/load":
                        self.send_header("Content-Length", "2")
                        self.end_headers()
                        self.wfile.write(b"ok")
                        return
                    require(self.path == "/sse", f"Unexpected fixture path {self.path}")
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Transfer-Encoding", "chunked")
                    self.end_headers()
                    self.wfile.write(f"{len(FIRST_EVENT):x}\r\n".encode() + FIRST_EVENT + b"\r\n")
                    self.wfile.flush()
                    while not owner.release.wait(0.1):
                        if owner.stopping.is_set():
                            return
                    self.wfile.write(f"{len(LAST_EVENT):x}\r\n".encode() + LAST_EVENT + b"\r\n0\r\n\r\n")
                    self.wfile.flush()
                except Exception as error:
                    if not owner.stopping.is_set():
                        owner.errors.put(error)

        class Server(ThreadingHTTPServer):
            def handle_error(self, _request, _client_address):
                if not owner.stopping.is_set():
                    owner.errors.put(sys.exc_info()[1])

        self.http = Server(h1_socket.getsockname(), Handler, bind_and_activate=False)
        self.http.socket.close()
        self.http.socket = h1_socket
        self.http.server_activate()
        self.h2_socket = h2_socket
        self.h2_socket.listen()
        self.h2_socket.settimeout(0.1)

    def spawn(self, target, *args):
        def guarded():
            try:
                target(*args)
            except Exception as error:
                if not self.stopping.is_set():
                    self.errors.put(error)
        thread = threading.Thread(target=guarded, daemon=True)
        self.threads.append(thread)
        thread.start()

    def start(self):
        self.spawn(self.http.serve_forever, 0.1)
        self.spawn(self.accept_h2)

    def accept_h2(self):
        while not self.stopping.is_set():
            try:
                connection, _ = self.h2_socket.accept()
            except socket.timeout:
                continue
            with self.lock:
                self.connections.append(connection)
            self.spawn(self.serve_h2, connection)

    def serve_h2(self, connection):
        connection.settimeout(0.1)
        h2 = H2Connection(config=H2Configuration(client_side=False, header_encoding="utf-8"))
        h2.initiate_connection()
        connection.sendall(h2.data_to_send())
        pending = set()
        while not self.stopping.is_set():
            try:
                data = connection.recv(65535)
                if not data:
                    return
                for event in h2.receive_data(data):
                    if isinstance(event, RequestReceived):
                        h2.send_headers(event.stream_id, [(":status", "200"), ("content-type", "application/grpc")])
                        h2.send_data(event.stream_id, grpc_frame(MESSAGES[0]))
                        pending.add(event.stream_id)
                    elif isinstance(event, DataReceived):
                        h2.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
                    elif isinstance(event, StreamReset):
                        pending.discard(event.stream_id)
            except socket.timeout:
                pass
            if self.release.is_set():
                for stream in pending:
                    h2.send_data(stream, grpc_frame(MESSAGES[1]))
                    h2.send_headers(stream, [("grpc-status", "0")], end_stream=True)
                pending.clear()
            outgoing = h2.data_to_send()
            if outgoing:
                connection.sendall(outgoing)

    def check(self):
        if not self.errors.empty():
            raise AssertionError("Backend worker failed") from self.errors.get()

    def stop(self):
        self.stopping.set()
        self.http.shutdown()
        self.http.server_close()
        self.h2_socket.close()
        with self.lock:
            for connection in self.connections:
                connection.close()
        for thread in self.threads:
            thread.join(timeout=2)


class Topology(ProcessTopology):
    def __init__(self, root, label, binaries, cert_source, work_stealing=True, grace=2, drain=6, readiness_drain=0):
        super().__init__(root, label, binaries, cert_source)
        self.work_stealing = work_stealing
        self.grace = grace
        self.drain = drain
        self.readiness_drain = readiness_drain

    def reserve_ports(self):
        for name in ("ctrl_admin", "ctrl_grpc", "ctrl_probe", "ctrl_metrics", "gw_admin", "gw_probe", "gw_metrics", "http_a", "http_b", "backend_h1", "backend_h2"):
            self.reservations[name], self.ports[name] = reserve_tcp_port()

    def write_resources(self):
        namespace = "http-shutdown"
        resources = [
            ("GatewayClass", "shutdown", {"controllerName": "edgion.io/gateway-controller"}, "gateway.networking.k8s.io/v1"),
            ("Gateway", "wire", {"gatewayClassName": "shutdown", "listeners": [
                {"name": name.replace("_", "-"), "protocol": "HTTP", "port": self.ports[name]} for name in ("http_a", "http_b")
            ]}, "gateway.networking.k8s.io/v1"),
        ]
        for protocol in ("h1", "h2"):
            port = self.ports[f"backend_{protocol}"]
            service_port = {"name": protocol, "port": port, "targetPort": port, "protocol": "TCP"}
            if protocol == "h2":
                service_port["appProtocol"] = "kubernetes.io/h2c"
            resources.append(("Service", protocol, {"ports": [service_port]}, "v1"))
            write_json(self.work / f"conf/EndpointSlice_{namespace}_{protocol}.yaml", {
                "apiVersion": "discovery.k8s.io/v1", "kind": "EndpointSlice",
                "metadata": {"name": protocol, "namespace": namespace, "labels": {"kubernetes.io/service-name": protocol}},
                "addressType": "IPv4", "ports": [{"name": protocol, "port": port, "protocol": "TCP"}],
                "endpoints": [{"addresses": ["127.0.0.1"], "conditions": {"ready": True}}],
            })
            kind = "HTTPRoute" if protocol == "h1" else "GRPCRoute"
            resources.append((kind, protocol, {"parentRefs": [{"name": "wire"}], "rules": [{"backendRefs": [{"name": protocol, "port": port}]}]}, "gateway.networking.k8s.io/v1"))
        for kind, name, spec, version in resources:
            metadata = {"name": name}
            if kind != "GatewayClass":
                metadata["namespace"] = namespace
            write_json(self.work / f"conf/{kind}_{metadata.get('namespace', '')}_{name}.yaml", {
                "apiVersion": version, "kind": kind, "metadata": metadata, "spec": spec,
            })

    def write_process_configs(self):
        super().write_process_configs()
        path = self.work / "gateway.yaml"
        config = path.read_text()
        config = config.replace("threads: 1", f"threads: 4\n  work_stealing: {str(self.work_stealing).lower()}")
        config = config.replace("grace_period_seconds: 2", f"readiness_drain_seconds: {self.readiness_drain}\n  grace_period_seconds: {self.grace}")
        config = config.replace("graceful_shutdown_timeout_seconds: 0", f"graceful_shutdown_timeout_seconds: {self.drain}")
        path.write_text(config)

    def start_backend(self):
        self.backend = Backend(self.reservations.pop("backend_h1"), self.reservations.pop("backend_h2"))
        self.backend.start()

    def start_gateway(self):
        for name in ("http_a", "http_b"):
            self.release(name)
        super().start_gateway()

    def wait_gateway_ready(self):
        self.wait("Gateway readiness", lambda: http_status(self.ports["gw_probe"], "/ready") == 200, process=self.gateway, timeout=30)
        for name in ("http_a", "http_b"):
            self.wait(f"{name} route", lambda name=name: http_status(self.ports[name], "/load") == 200, process=self.gateway, timeout=30)


class StreamAborted(Exception):
    """A peer closed or reset an unfinished HTTP/2 stream."""


class GrpcClient:
    def __init__(self, port):
        self.socket = socket.create_connection(("127.0.0.1", port), timeout=5)
        self.h2 = H2Connection(config=H2Configuration(client_side=True, header_encoding="utf-8"))
        self.h2.initiate_connection()
        self.h2.send_headers(1, [(":method", "POST"), (":scheme", "http"), (":authority", "localhost"), (":path", "/shutdown.Stream/Watch"), ("content-type", "application/grpc"), ("te", "trailers")], end_stream=True)
        self.socket.sendall(self.h2.data_to_send())
        self.body = bytearray()
        self.wire = bytearray()
        self.goaways = []
        self.trailers = {}
        self.ended = False
        self.deadline = time.monotonic() + 10

    def receive(self):
        remaining = self.deadline - time.monotonic()
        require(remaining > 0, "gRPC receive deadline exceeded")
        self.socket.settimeout(remaining)
        data = self.socket.recv(65535)
        if not data:
            raise StreamAborted("gRPC connection ended before END_STREAM")
        # hyper-h2 closes its state machine at GOAWAY, although HTTP/2 permits
        # committed streams to drain afterwards. Validate GOAWAY separately and
        # keep feeding all other complete frames through its protocol parser.
        self.wire.extend(data)
        events = []
        while len(self.wire) >= 9:
            length = int.from_bytes(self.wire[:3], "big")
            require(length <= 65536, "Unexpected oversized fixture frame")
            if len(self.wire) < length + 9:
                break
            frame = bytes(self.wire[:length + 9])
            del self.wire[:length + 9]
            if frame[3] == 7:
                require(length >= 8 and int.from_bytes(frame[5:9], "big") == 0, "Malformed GOAWAY")
                last_stream = int.from_bytes(frame[9:13], "big") & 0x7fffffff
                error = int.from_bytes(frame[13:17], "big")
                require(error == 0 and last_stream >= 1, f"GOAWAY rejected active stream: {last_stream}, {error}")
                self.goaways.append(last_stream)
            else:
                events.extend(self.h2.receive_data(frame))
        for event in events:
            if isinstance(event, ResponseReceived):
                require(dict(event.headers).get(":status") == "200", f"gRPC response: {event.headers}")
            elif isinstance(event, DataReceived):
                self.body.extend(event.data)
                self.h2.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
            elif isinstance(event, TrailersReceived):
                self.trailers.update(event.headers)
            elif isinstance(event, StreamEnded):
                self.ended = True
            elif isinstance(event, StreamReset):
                raise StreamAborted(f"gRPC reset {event.error_code}")
        outgoing = self.h2.data_to_send()
        if outgoing:
            self.socket.sendall(outgoing)

    def first(self):
        expected = grpc_frame(MESSAGES[0])
        while len(self.body) < len(expected):
            self.receive()
        require(bytes(self.body) == expected and not self.ended, "gRPC first message was incomplete or stream already ended")

    def finish(self):
        self.deadline = time.monotonic() + 5
        while not self.ended:
            self.receive()
        require(bytes(self.body) == b"".join(map(grpc_frame, MESSAGES)), "gRPC message framing/content mismatch")
        require(self.trailers.get("grpc-status") == "0", "gRPC success trailers missing")

    def close(self):
        self.socket.close()


def run_case(root, binaries, cert_source, stealing, abort=False):
    label = f"stealing-{str(stealing).lower()}-{'abort' if abort else 'drain'}"
    topology = Topology(root, label, binaries, cert_source, stealing, grace=2, drain=3 if abort else 6, readiness_drain=0 if abort else 2)
    idle, sse, grpc = [], [], []
    try:
        topology.prepare()
        topology.start_backend()
        topology.start_controller()
        topology.start_gateway()
        topology.wait_gateway_ready()
        for listener in ("http_a", "http_b"):
            port = topology.ports[listener]
            for _ in range(4):
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
                topology.clients.append(connection)
                connection.request("GET", "/load")
                response = connection.getresponse()
                require(response.status == 200 and response.read() == b"ok", "Keepalive setup failed")
                require(connection.sock is not None, "Completed request did not retain keepalive")
                connection.sock.settimeout(0.03)
                try:
                    data = connection.sock.recv(1, socket.MSG_PEEK)
                    raise AssertionError(f"Idle connection closed or sent unsolicited data: {data!r}")
                except socket.timeout:
                    pass
                idle.append(connection)
            for _ in range(2):
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
                topology.clients.append(connection)
                connection.request("GET", "/sse")
                response = connection.getresponse()
                require(response.status == 200 and response.chunked, "SSE missing HTTP/1 chunk framing")
                require(response.read(len(FIRST_EVENT)) == FIRST_EVENT, "First SSE event missing")
                sse.append(response)
                client = GrpcClient(port)
                topology.clients.append(client)
                client.first()
                grpc.append(client)
        topology.backend.check()
        started = time.monotonic()
        topology.gateway.send_signal(signal.SIGTERM)
        topology.wait("withdraw readiness", lambda: http_status(topology.ports["gw_probe"], "/ready") == 503,
                      process=topology.gateway, timeout=2)
        if topology.readiness_drain:
            require(http_status(topology.ports["gw_probe"], "/health") == 200, "Liveness failed during withdrawal")
            require(http_status(topology.ports["http_a"], "/load") == 200, "Fresh connection failed during withdrawal")
        topology.wait_log_text("Graceful shutdown: grace period 2s starts", timeout=topology.readiness_drain + 3)
        drain_started = time.monotonic()
        require(drain_started - started >= topology.readiness_drain, "Listeners drained before withdrawal window elapsed")
        for connection in idle:
            connection.sock.settimeout(max(0.01, 3 - (time.monotonic() - drain_started)))
            require(connection.sock.recv(1) == b"", "Idle keepalive was not closed by shutdown")
        require(topology.gateway.poll() is None, "Gateway exited before active stream release")
        if abort:
            topology.wait_exit(True, timeout=12)
            for response in sse:
                try:
                    response.read()
                except (http.client.IncompleteRead, ConnectionError):
                    continue
                raise AssertionError("Unreleased SSE stream completed successfully")
            for client in grpc:
                try:
                    client.finish()
                except (StreamAborted, ConnectionError):
                    continue
                raise AssertionError("Unreleased gRPC stream completed successfully")
        else:
            # Intentionally hold active streams halfway through the configured
            # grace period. Shutdown entry and idle wakeup were already observed;
            # this delay tests grace preservation, not waiter-registration timing.
            remaining = drain_started + topology.grace / 2 - time.monotonic()
            if remaining > 0:
                time.sleep(remaining)
            require(topology.gateway.poll() is None, "Gateway exited during active-stream grace")
            topology.backend.release.set()
            for response in sse:
                require(response.read() == LAST_EVENT and response.isclosed(), "SSE terminal event or chunk terminator missing")
            for client in grpc:
                client.finish()
            topology.wait_exit(True, timeout=15)
        topology.backend.check()
        print(f"PASS {label}: idle=8 SSE=4 gRPC=4 exit=0 elapsed={time.monotonic() - started:.3f}s", flush=True)
    finally:
        topology.cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gateway-bin", type=Path, default=ROOT / "target/debug/edgion-gateway")
    parser.add_argument("--controller-bin", type=Path, default=ROOT / "target/debug/edgion-controller")
    parser.add_argument("--work-root", type=Path, default=ROOT / "tmp/integration_testing")
    args = parser.parse_args()
    binaries = {"controller": args.controller_bin.resolve(), "gateway": args.gateway_bin.resolve()}
    for binary in binaries.values():
        require(binary.is_file(), f"Missing binary: {binary}")
    args.work_root.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="http-shutdown.", dir=args.work_root.resolve()))
    print(f"Artifacts: {root}", flush=True)
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt("Termination requested")

    signal.signal(signal.SIGTERM, interrupted)
    prepare_certificates(root, None)
    for stealing in (True, False):
        run_case(root, binaries, root / "certs", stealing)
        run_case(root, binaries, root / "certs", stealing, abort=True)


if __name__ == "__main__":
    main()
