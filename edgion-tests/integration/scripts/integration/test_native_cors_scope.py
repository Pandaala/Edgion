#!/usr/bin/env python3
"""D0084: native CORS scope, real status/publication, and hot reload.

Reuse the running HTTPRoute/Basic topology (one Controller, Gateway, test_server).
This standalone client never starts or stops processes and only owns uniquely named
HTTPRoutes. It avoids changing frozen integration fixtures or suite registration.
Run after utils/start_all_with_conf.sh --suites HTTPRoute/Basic, or supply ports.
All phases poll status, published configuration, and actual traffic together.
"""

import argparse
import copy
from http.client import HTTPConnection, HTTPException
import json
import os
import time
import uuid


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def request(port, path, method="GET", headers=None, body=None):
    conn = HTTPConnection("127.0.0.1", port, timeout=2)
    try:
        conn.request(method, path, body=body, headers=headers or {})
        response = conn.getresponse()
        data = response.read(1024 * 1024 + 1)
        require(len(data) <= 1024 * 1024, "Response exceeded fixture byte cap")
        return response.status, {k.lower(): v for k, v in response.getheaders()}, data
    finally:
        conn.close()


class Scenario:
    def __init__(self, args):
        self.args = args
        self.name = "d0084-" + uuid.uuid4().hex[:12]
        self.host = self.name + ".example.com"
        self.owned = []
        self.controller_token = os.getenv("EDGION_ADMIN_TOKEN", "integration-controller-admin-token-0123456789")
        self.gateway_token = os.getenv("EDGION_GATEWAY_ADMIN_TOKEN", "integration-gateway-admin-token-0123456789")
        self.cors = {"type": "CORS", "cors": {
            "allowOrigins": ["https://app.example"], "allowMethods": ["GET"], "maxAge": 300,
        }}

    def admin(self, gateway, path, method="GET", obj=None):
        port = self.args.gateway_admin_port if gateway else self.args.controller_port
        token = self.gateway_token if gateway else self.controller_token
        return request(port, path, method, {"Authorization": "Bearer " + token,
                       "Content-Type": "application/yaml"}, json.dumps(obj) if obj is not None else None)

    def path(self, name=None):
        base = f"/api/v1/namespaced/HTTPRoute/{self.args.namespace}"
        return base + "/" + name if name else base

    def publish(self, obj, update=False):
        name = obj["metadata"]["name"]
        if not update:
            self.owned.append(name)
        code, _, body = self.admin(False, self.path(name if update else None), "PUT" if update else "POST", obj)
        require(code in (200, 201), f"Publish {name}: HTTP {code}: {body.decode(errors='replace')}")

    def read_route(self, name, gateway=False, allow_missing=False):
        # The storage API returns authored input. Derived invalid indices and
        # resolved delegation rules live in the Controller's publication cache.
        cache = "configclient" if gateway else "configserver"
        path = f"/{cache}/HTTPRoute?namespace={self.args.namespace}&name={name}"
        code, _, body = self.admin(gateway, path)
        require(code == 200, f"Read {name}: HTTP {code}")
        obj = json.loads(body)
        data = obj.get("data")
        require(allow_missing or data is not None, f"{name}: not yet published in {cache}")
        return data

    def wait_phase(self, label, check):
        deadline = time.monotonic() + self.args.timeout
        last_error = "not ready"
        while time.monotonic() < deadline:
            try:
                check()
                print("PASS " + label, flush=True)
                return
            except (AssertionError, OSError, HTTPException) as error:
                last_error = str(error)
            time.sleep(0.1)
        raise AssertionError(f"{label}: {last_error}")

    def rule(self, path, rule_cors=False, backend_cors=False, weighted=False):
        backend = {"name": self.args.service, "port": self.args.backend_port}
        if backend_cors:
            backend["filters"] = [copy.deepcopy(self.cors)]
        backends = [backend]
        if weighted:
            backend["weight"] = 0
            backends.insert(0, {"name": self.args.service, "port": self.args.backend_port, "weight": 1})
        rule = {"matches": [{"path": {"type": "PathPrefix", "value": path}}], "backendRefs": backends}
        if rule_cors:
            rule["filters"] = [copy.deepcopy(self.cors)]
        return rule

    def route(self, name, rules, parents=None):
        return {"apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
                "metadata": {"name": name, "namespace": self.args.namespace},
                "spec": {"hostnames": [self.host], "rules": rules,
                         "parentRefs": parents if parents is not None else [{"name": self.args.gateway}]}}

    def state(self, name, invalid, rejected=False):
        obj = self.read_route(name)
        actual = obj["spec"].get("invalidRuleIndices", [])
        require(actual == invalid, f"{name}: invalid indices {actual}, expected {invalid}")
        conditions = [c for p in obj.get("status", {}).get("parents", []) for c in p.get("conditions", [])]
        acceptance = "False" if rejected else "True"
        require(any(c["type"] == "Accepted" and c["status"] == acceptance for c in conditions),
                f"{name}: expected Accepted={acceptance}")
        partial = [c for c in conditions if c["type"] == "PartiallyInvalid" and c["status"] == "True"]
        require(bool(partial) == (bool(invalid) and not rejected), f"{name}: unexpected PartiallyInvalid")
        if invalid:
            diagnostics = [c for c in conditions if c["type"] == "Accepted"] if rejected else partial
            require(any("CORS is not supported" in c["message"] for c in diagnostics), "Missing scope diagnostic")
        published = self.read_route(name, True)
        require(published is not None and published["spec"].get("invalidRuleIndices", []) == invalid,
                f"{name}: Gateway publication has not converged")
        require(published["spec"]["rules"] == obj["spec"]["rules"], f"{name}: Gateway rules differ")

    def traffic(self, path, method, status, origin="https://app.example", allow=False):
        headers = {"Host": self.host, "Origin": origin}
        if method == "OPTIONS":
            headers["Access-Control-Request-Method"] = "GET"
        code, response_headers, _ = request(self.args.gateway_port, path, method, headers)
        require(code == status, f"{method} {path}: HTTP {code}, expected {status}")
        expected = origin if allow else None
        require(response_headers.get("access-control-allow-origin") == expected,
                f"{method} {path}: unexpected Allow-Origin")

    def supported(self, path):
        for method, code in [("OPTIONS", 204), ("GET", 200)]:
            self.traffic(path, method, code, allow=True)
            self.traffic(path, method, code, origin="https://denied.example")

    def run(self):
        prefix = "/echo/" + self.name
        positive = self.rule(prefix + "/positive", backend_cors=True)
        different = copy.deepcopy(positive["backendRefs"][0])
        different["weight"] = 3
        different["filters"][0]["cors"]["allowOrigins"] = ["https://different.example"]
        positive["backendRefs"].append(different)
        obj = self.route(self.name, [
            self.rule(prefix + "/backend", backend_cors=True),
            self.rule(prefix + "/mixed", rule_cors=True, backend_cors=True),
            self.rule(prefix + "/weighted", backend_cors=True, weighted=True),
            self.rule(prefix + "/rule", rule_cors=True),
            self.rule(prefix + "/plain"),
            positive,
        ])
        self.publish(obj)

        def rejected():
            self.state(self.name, [0, 1, 2, 5])
            for path in ["/backend", "/mixed", "/weighted", "/positive"]:
                for method in ["OPTIONS", "GET"]:
                    self.traffic(prefix + path, method, 404)
            self.supported(prefix + "/rule")
            self.traffic(prefix + "/plain", "OPTIONS", 204)
            self.traffic(prefix + "/plain", "GET", 200)

        self.wait_phase("backend-only, mixed, zero-weight rejection; rule CORS and fallback", rejected)
        fixed = copy.deepcopy(obj)
        fixed["spec"]["rules"][0] = self.rule(prefix + "/backend", rule_cors=True)
        self.publish(fixed, update=True)

        def restored():
            self.state(self.name, [1, 2, 5])
            self.supported(prefix + "/backend")
            self.supported(prefix + "/rule")

        self.wait_phase("move policy to rule restores preflight and actual responses", restored)
        self.publish(obj, update=True)
        self.wait_phase("reintroducing backend CORS removes stale active rule", rejected)

        all_name, all_path = self.name + "-all", prefix + "/all"
        all_invalid = self.route(all_name, [self.rule(all_path, backend_cors=True)])
        self.publish(all_invalid)

        def all_rejected():
            self.state(all_name, [0], rejected=True)
            for method in ["OPTIONS", "GET"]:
                self.traffic(all_path, method, 404)

        self.wait_phase("sole invalid rule rejects Gateway attachment with reason", all_rejected)
        all_invalid["spec"]["rules"] = [self.rule(all_path, rule_cors=True)]
        self.publish(all_invalid, update=True)

        def all_restored():
            self.state(all_name, [])
            self.supported(all_path)

        self.wait_phase("sole rule recovers acceptance and traffic", all_restored)

        child_name, parent_name = self.name + "-child", self.name + "-parent"
        child_path = prefix + "/delegated"
        child = self.route(child_name, [self.rule(child_path, backend_cors=True)], [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": parent_name}])
        del child["spec"]["hostnames"]
        parent = self.route(parent_name, [{"matches": [{"path": {"type": "PathPrefix", "value": child_path}}],
            "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": child_name}]}])
        self.publish(child)
        self.publish(parent)

        def delegated_rejected():
            current = self.read_route(child_name)
            require(current["spec"].get("invalidRuleIndices") == [0], "Child not marked invalid")
            published = self.read_route(parent_name, True)
            require(published is not None and published["spec"].get("resolvedRules") == [],
                    "Invalid child still present in effective rules")
            parent_status = self.read_route(parent_name).get("status", {})
            require(any("backendRefs[0] CORS is not supported" in c.get("message", "")
                        for p in parent_status.get("parents", []) for c in p.get("conditions", [])),
                    "Parent delegation status lacks child scope diagnostic")
            for method in ["OPTIONS", "GET"]:
                self.traffic(child_path, method, 404)

        self.wait_phase("delegation drops backend CORS child", delegated_rejected)
        child["spec"]["rules"] = [self.rule(child_path, rule_cors=True)]
        self.publish(child, update=True)

        def delegated_restored():
            current = self.read_route(child_name)
            require(not current["spec"].get("invalidRuleIndices"), "Child invalid indices remain")
            parent_status = self.read_route(parent_name).get("status", {})
            require(not any("CORS is not supported" in c.get("message", "")
                            for p in parent_status.get("parents", []) for c in p.get("conditions", [])),
                    "Parent CORS rejection diagnostic remains")
            require(not any(c["type"] == "PartiallyInvalid" and c["status"] == "True"
                            for p in parent_status.get("parents", []) for c in p.get("conditions", [])),
                    "Parent delegation error remains")
            published = self.read_route(parent_name, True)
            require(published is not None and len(published["spec"].get("resolvedRules", [])) == 1,
                    "Corrected child has not been published through parent")
            self.supported(child_path)

        self.wait_phase("delegation requeues parent and restores rule CORS", delegated_restored)

    def cleanup(self):
        errors = []
        for name in reversed(self.owned):
            try:
                code, _, _ = self.admin(False, self.path(name), "DELETE")
                require(code in (200, 204, 404), f"Delete {name}: HTTP {code}")
                def absent():
                    require(self.read_route(name, True, allow_missing=True) is None, f"{name}: still published")
                self.wait_phase("cleanup " + name, absent)
            except (AssertionError, OSError, HTTPException) as error:
                errors.append(str(error))
        require(not errors, "Cleanup failed: " + "; ".join(errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name, default in [("controller-port", 12101), ("gateway-admin-port", 12001),
                          ("gateway-port", 31000), ("backend-port", 30001)]:
        parser.add_argument("--" + name, type=int, default=default)
    parser.add_argument("--namespace", default="edgion-test")
    parser.add_argument("--gateway", default="http-test-gateway")
    parser.add_argument("--service", default="test-http")
    parser.add_argument("--timeout", type=float, default=30)
    scenario = Scenario(parser.parse_args())
    try:
        scenario.run()
    finally:
        scenario.cleanup()


if __name__ == "__main__":
    main()
