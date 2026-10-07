"""Checks for the private fixture barrier and supported listener bootstrap order."""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import HTTPRedirectHandler, ProxyHandler


TEST_DIR = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("gateway_fixture_ready", TEST_DIR / "gateway_fixture_ready.py")
barrier = importlib.util.module_from_spec(spec)
spec.loader.exec_module(barrier)
SECRET = "private-fixture-material-must-not-appear"
GATEWAY = {"metadata": {"name": "package-test", "namespace": "default"},
           "spec": {"gatewayClassName": "package-test", "listeners": [{"protocol": "HTTP", "port": 80}]}}
CLASS = {"metadata": {"name": "package-test"}, "spec": {"controllerName": "edgion.io/gateway-controller"}}


def envelope(resource):
    return {"success": True, "data": [resource], "count": 1}


class Response:
    def __init__(self, body, status=200):
        self.body, self.status = body, status
        self.closed = False
        self.read_bound = None

    def read(self, limit):
        self.read_bound = limit
        return self.body[:limit]

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.closed = True


class FixtureBarrierTests(unittest.TestCase):
    def test_real_list_envelopes_match_owned_resources_only(self):
        for kind, resource in [("Gateway", GATEWAY), ("GatewayClass", CLASS)]:
            self.assertTrue(barrier.fixture_present(kind, envelope(resource)))
            for invalid in [None, [], {}, {"success": False, "data": [resource]},
                            {"success": 1, "data": [resource]}, {"success": True, "data": resource},
                            {"success": True, "data": []}, {"success": True, "data": [None, SECRET]},
                            {"success": True, "data": [{"data": resource}]}]:
                self.assertFalse(barrier.fixture_present(kind, invalid))
            wrong_name = copy.deepcopy(resource)
            wrong_name["metadata"]["name"] = "another-fixture"
            self.assertFalse(barrier.fixture_present(kind, envelope(wrong_name)))

    def test_listener_identity_and_port_are_strict(self):
        for field, value in [("namespace", "other"), ("namespace", None)]:
            resource = copy.deepcopy(GATEWAY)
            resource["metadata"][field] = value
            self.assertFalse(barrier.fixture_present("Gateway", envelope(resource)))
        for field, values in [("gatewayClassName", [SECRET, None]),
                              ("listeners", [None, {}, [], [SECRET], [{"port": 80}]])]:
            for value in values:
                resource = copy.deepcopy(GATEWAY)
                resource["spec"][field] = value
                self.assertFalse(barrier.fixture_present("Gateway", envelope(resource)))
        for port in [True, False, "80", 80.0, 81, None]:
            resource = copy.deepcopy(GATEWAY)
            resource["spec"]["listeners"][0]["port"] = port
            self.assertFalse(barrier.fixture_present("Gateway", envelope(resource)))
        resource = copy.deepcopy(GATEWAY)
        resource["spec"]["listeners"][0]["protocol"] = "HTTPS"
        self.assertFalse(barrier.fixture_present("Gateway", envelope(resource)))
        resource = copy.deepcopy(CLASS)
        resource["spec"]["controllerName"] = SECRET
        self.assertFalse(barrier.fixture_present("GatewayClass", envelope(resource)))

    def test_http_probe_bounds_and_transport_policy(self):
        response = Response(json.dumps(envelope(GATEWAY)).encode())
        with patch.object(barrier, "build_opener") as builder:
            builder.return_value.open.return_value = response
            self.assertTrue(barrier.probe("Gateway", SECRET))
            handlers = builder.call_args.args
            self.assertIsInstance(handlers[0], ProxyHandler)
            self.assertEqual(handlers[0].proxies, {})
            self.assertIsInstance(handlers[1], HTTPRedirectHandler)
            self.assertIsNone(handlers[1].redirect_request(None, None, 302, SECRET, {}, SECRET))
            request = builder.return_value.open.call_args.args[0]
            self.assertEqual(request.full_url, "http://127.0.0.1:12001/configclient/Gateway/list")
            self.assertEqual(request.get_header("Authorization"), "Bearer " + SECRET)
            self.assertEqual(builder.return_value.open.call_args.kwargs, {"timeout": 2})
        self.assertEqual(response.read_bound, barrier.RESPONSE_CAP + 1)
        self.assertTrue(response.closed)

    def test_main_requires_both_resources_and_hides_all_failures(self):
        with tempfile.TemporaryDirectory() as state:
            Path(state, "gateway-token").write_text(SECRET)
            output, errors = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
                with patch.object(barrier, "probe", side_effect=[True, True]) as probe:
                    self.assertEqual(barrier.main(state), 0)
                    self.assertEqual([call.args[0] for call in probe.call_args_list], ["GatewayClass", "Gateway"])
                for results in [[False], [True, False]]:
                    with patch.object(barrier, "probe", side_effect=results):
                        self.assertEqual(barrier.main(state), 1)
                for body in [b"bad-json-" + SECRET.encode(), b"x" * (barrier.RESPONSE_CAP + 1)]:
                    with patch.object(barrier, "build_opener") as builder:
                        builder.return_value.open.return_value = Response(body)
                        self.assertEqual(barrier.main(state), 1)
                for error in [TimeoutError(SECRET), HTTPError(SECRET, 302, SECRET, {}, None)]:
                    with patch.object(barrier, "build_opener") as builder:
                        builder.return_value.open.side_effect = error
                        self.assertEqual(barrier.main(state), 1)
            self.assertEqual(output.getvalue() + errors.getvalue(), "")

    def test_lifecycle_restart_is_unconditional_and_before_body_assertion(self):
        source = (TEST_DIR / "lifecycle.sh").read_text()
        start = source.index('    quiet_cli apply -f "$state/resources.yaml"')
        end = source.index('    quiet_cli apply -f /tests/fixtures/crud.yaml', start)
        bootstrap = source[start:end]
        for barrier_status in [0, 1]:
            script = '''
set -euo pipefail
state=/private-state
quiet_cli() { :; }
wait_until() { echo "wait:$*"; if [[ $1 == fixture_ready ]]; then return "$barrier_status"; fi; }
systemctl() { echo "systemctl:$*"; }
'''
            result = subprocess.run(["bash", "-c", script + bootstrap], env={"barrier_status": str(barrier_status)},
                                    capture_output=True, text=True)
            if barrier_status:
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("systemctl:", result.stdout)
            else:
                self.assertEqual(result.returncode, 0)
                self.assertEqual(result.stdout.splitlines(), ["wait:fixture_ready", "systemctl:restart edgion-gateway",
                                                              "wait:ready 12000", "wait:proxy"])


if __name__ == "__main__":
    unittest.main()
