"""Adversarial checks for the report's field and execution boundaries."""
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch


TEST_DIR = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("proxy_diagnostics", TEST_DIR / "proxy_diagnostics.py")
diagnostics = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diagnostics)
SECRET = "private-test-material-must-not-appear"


class Handler(BaseHTTPRequestHandler):
    hits = 0
    status = 204
    redirect = None
    delay = 0

    def do_GET(self):
        type(self).hits += 1
        time.sleep(type(self).delay)
        try:
            self.send_response(type(self).status)
            if type(self).redirect:
                self.send_header("Location", type(self).redirect)
            self.end_headers()
        except OSError:
            pass

    def log_message(self, *_args):
        pass


@contextlib.contextmanager
def server(status=204, redirect=None, delay=0):
    handler = type("FixtureHandler", (Handler,), {"hits": 0, "status": status, "redirect": redirect, "delay": delay})
    fixture = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    fixture.daemon_threads = True
    worker = threading.Thread(target=fixture.serve_forever, daemon=True)
    worker.start()
    try:
        yield "http://127.0.0.1:" + str(fixture.server_port) + "/", handler
    finally:
        fixture.shutdown()
        fixture.server_close()
        worker.join(timeout=2)


class DiagnosticsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def file(self, name, data):
        path = self.root / name
        path.write_bytes(data)
        return path

    def test_secret_fields_are_not_reflected(self):
        entry = {"request_info": {"status": SECRET, "headers": SECRET},
                 "diagnostics": [{"kind": "error", "code": SECRET, "detail": SECRET},
                                 {"kind": "error", "code": "BackendPortResolutionFailed"},
                                 {"kind": SECRET, "code": SECRET}],
                 "upstreams": SECRET, "stage_logs": SECRET}
        output = diagnostics.access_records(self.file("access", json.dumps(entry).encode()))
        self.assertEqual(output, [{"status": "unavailable", "error_codes": ["other_error_code", "BackendPortResolutionFailed"]}])
        self.assertNotIn(SECRET, json.dumps(output))
        states = diagnostics.unit_properties("\n".join(name + "=" + SECRET for name in list(diagnostics.UNIT_FIELDS) + ["ExecMainStatus"]))
        self.assertTrue(all(value == "unavailable" for value in states.values()))
        self.assertNotIn(SECRET, json.dumps(states))
        self.assertIsNone(diagnostics.backend_ipv4(self.file("ipv4", SECRET.encode())))

    def test_status_types_and_bounds(self):
        for value in [True, False, "503", 0, 99, 600, None, [SECRET]]:
            entry = {"request_info": {"status": value}}
            result = diagnostics.access_records(self.file("access", json.dumps(entry).encode()))
            self.assertEqual(result[0]["status"], "unavailable")
        for value in [100, 200, 503, 599]:
            result = diagnostics.access_records(self.file("access", json.dumps({"request_info": {"status": value}}).encode()))
            self.assertEqual(result[0]["status"], value)
        self.assertEqual(diagnostics.unit_properties("ExecMainStatus=255")["ExecMainStatus"], 255)
        for value in ["256", "-1", "True", SECRET, "9" * 10000]:
            self.assertEqual(diagnostics.unit_properties("ExecMainStatus=" + value)["ExecMainStatus"], "unavailable")

    def test_tail_read_is_bounded_and_drops_partial_json(self):
        record = json.dumps({"request_info": {"status": 503}, "diagnostics": [{"kind": "error", "code": "Unknown"}]}).encode() + b"\n"
        path = self.file("access", (SECRET.encode() * 10000) + b"\n" + record * 7)
        with patch.object(diagnostics.os, "read", wraps=os.read) as read:
            result = diagnostics.access_records(path)
        self.assertEqual(len(result), 5)
        self.assertEqual(read.call_count, 1)
        self.assertEqual(read.call_args[0][1], diagnostics.FILE_CAP)
        self.assertNotIn(SECRET, json.dumps(result))
        self.assertEqual(diagnostics.access_records(self.file("bad", b'{"secret":' + SECRET.encode())), [])
        self.assertEqual(diagnostics.access_records(self.file("oversize", b"x" * (diagnostics.FILE_CAP + 1))), [])

    def test_diagnostic_count_and_bad_types(self):
        record = {"request_info": {"status": 503}, "diagnostics": [{"kind": "error", "code": [SECRET]}] * 100}
        result = diagnostics.access_records(self.file("access", json.dumps(record).encode()))
        self.assertEqual(len(result[0]["error_codes"]), 32)
        self.assertNotIn(SECRET, json.dumps(result))

    def test_symlink_fifo_and_oversized_ipv4_are_rejected(self):
        target = self.file("private", SECRET.encode())
        link = self.root / "symlink"
        link.symlink_to(target)
        fifo = self.root / "fifo"
        os.mkfifo(str(fifo))
        started = time.monotonic()
        for path in [link, fifo, self.root]:
            self.assertIsNone(diagnostics.backend_ipv4(path))
            self.assertEqual(diagnostics.access_records(path), "unavailable")
        self.assertLess(time.monotonic() - started, 1)
        self.assertIsNone(diagnostics.backend_ipv4(self.file("large", b"1" * 33)))
        for value in ["127.0.0.1", "169.254.169.254", "0.0.0.0", "::1", "192.168.001.1", "224.0.0.1"]:
            self.assertIsNone(diagnostics.backend_ipv4(self.file("ipv4", value.encode())))
        self.assertEqual(str(diagnostics.backend_ipv4(self.file("valid", b"172.17.0.2\n"))), "172.17.0.2")

    def test_http_does_not_read_body_and_hides_errors(self):
        class Response:
            status = 204
            closed = False

            def read(self, *_args):
                raise AssertionError("body must not be read: " + SECRET)

            def close(self):
                self.closed = True

        response = Response()
        with patch.object(diagnostics, "build_opener") as builder:
            builder.return_value.open.return_value = response
            self.assertEqual(diagnostics.http_status("http://127.0.0.1/", time.monotonic() + 1), 204)
            self.assertTrue(response.closed)
            response.status = SECRET
            self.assertEqual(diagnostics.http_status("http://127.0.0.1/", time.monotonic() + 1), "unavailable")
            response.status = True
            self.assertEqual(diagnostics.http_status("http://127.0.0.1/", time.monotonic() + 1), "unavailable")
            builder.return_value.open.side_effect = RuntimeError(SECRET)
            output = diagnostics.http_status("http://127.0.0.1/", time.monotonic() + 1)
            self.assertEqual(output, "transport_failure")
            self.assertNotIn(SECRET, output)

    def test_namespace_address_is_only_reported_as_boolean(self):
        address = diagnostics.backend_ipv4(self.file("ipv4", b"172.17.0.2"))
        output = json.dumps([{"ifname": SECRET, "addr_info": [{"family": "inet", "local": "172.17.0.2", "label": SECRET}]}])
        with patch.object(diagnostics, "run_command", return_value=output):
            result = diagnostics.address_in_namespace(address, time.monotonic() + 1)
            self.assertIs(result, True)
            self.assertNotIn(SECRET, json.dumps(result))
            self.assertNotIn("172.17.0.2", json.dumps(result))
        with patch.object(diagnostics, "run_command", return_value=SECRET):
            self.assertEqual(diagnostics.address_in_namespace(address, time.monotonic() + 1), "unavailable")

    def test_real_http_proxy_environment_is_disabled(self):
        with server(status=599) as (proxy_url, proxy_handler), server() as (target_url, target_handler):
            env = {"http_proxy": proxy_url, "HTTP_PROXY": proxy_url, "https_proxy": proxy_url,
                   "no_proxy": "", "NO_PROXY": ""}
            with patch.dict(os.environ, env):
                self.assertEqual(diagnostics.http_status(target_url, time.monotonic() + 2), 204)
            self.assertEqual(proxy_handler.hits, 0)
            self.assertEqual(target_handler.hits, 1)

    def test_real_http_redirect_is_not_followed(self):
        with server() as (destination, target_handler), server(status=302, redirect=destination) as (source, source_handler):
            self.assertEqual(diagnostics.http_status(source, time.monotonic() + 2), 302)
            self.assertEqual(source_handler.hits, 1)
            self.assertEqual(target_handler.hits, 0)

    def test_real_http_probe_timeout(self):
        with server(delay=0.2) as (url, _handler):
            started = time.monotonic()
            self.assertEqual(diagnostics.http_status(url, time.monotonic() + 0.03), "transport_failure")
            self.assertLess(time.monotonic() - started, 0.15)

    def test_whole_helper_deadline_interrupts_file_work(self):
        code = ("import importlib.util,time; "
                "s=importlib.util.spec_from_file_location('d'," + repr(str(TEST_DIR / 'proxy_diagnostics.py')) + "); "
                "d=importlib.util.module_from_spec(s); s.loader.exec_module(d); "
                "d.read_regular=lambda *a,**k: time.sleep(10); "
                "d.collect_snapshot=lambda deadline:d.read_regular('ignored',32); d.main(0.1)")
        started = time.monotonic()
        result = subprocess.run([sys.executable, "-c", code], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=1)
        self.assertLess(time.monotonic() - started, 1)
        self.assertEqual(json.loads(result.stdout), {"proxy_timeout_diagnostics": "unavailable"})
        self.assertEqual(result.stderr, b"")

    def test_failed_snapshot_does_not_bypass_original_fail(self):
        source = (TEST_DIR / "lifecycle.sh").read_text()
        start = source.index("wait_until() {")
        end = source.index("\nready()", start)
        script = ("set -eu\n" + source[start:end] + "\n"
                  "sleep() { :; }\nproxy() { return 1; }\n"
                  "proxy_timeout_diagnostics() { echo diagnostics-unavailable; return 42; }\n"
                  "fail() { echo \"FAIL: $*\"; exit 1; }\nwait_until proxy\necho forbidden-success\n")
        result = subprocess.run(["bash", "-c", script], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=2)
        self.assertEqual(result.returncode, 1)
        self.assertIn(b"FAIL: condition timed out: proxy", result.stdout)
        self.assertNotIn(b"forbidden-success", result.stdout)


if __name__ == "__main__":
    unittest.main()
