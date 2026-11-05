#!/usr/bin/env python3
"""D0086: credentialed native CORS exposure policy and hot reload.

Run against the shared HTTPRoute/Basic topology started by start_all_with_conf.sh.
This client never starts or stops servers; existing integration files stay frozen.
"""
import argparse
import copy
from test_native_cors_scope import Scenario, require, request


class ExposureScenario(Scenario):
    def state(self, name, invalid, rejected=False):
        current = self.read_route(name)
        require(current["spec"].get("invalidRuleIndices", []) == invalid, "Controller invalid indices differ")
        conditions = [c for p in current.get("status", {}).get("parents", []) for c in p.get("conditions", [])]
        require(any(c["type"] == "Accepted" and c["status"] == ("False" if rejected else "True")
                    for c in conditions), "Unexpected acceptance")
        partial = [c for c in conditions if c["type"] == "PartiallyInvalid" and c["status"] == "True"]
        require(bool(partial) == (bool(invalid) and not rejected), "Unexpected partial invalidity")
        if invalid:
            require(any("exposeHeaders" in c.get("message", "") for c in conditions), "Missing diagnostic")
            if rejected:
                require(any(c["type"] == "Accepted" and c["reason"] == "UnsupportedValue"
                            for c in conditions), "Missing unsupported-value reason")
        else:
            require(not any("exposeHeaders" in c.get("message", "") for c in conditions), "Stale diagnostic")
        published = self.read_route(name, True)
        require(published["spec"] == current["spec"], "Gateway publication has not converged")

    def exposed(self, path, expected, credentials):
        for method, expected_status in [("GET", 200), ("OPTIONS", 204)]:
            # Even credentials-enabled policies must not depend on Cookie presence.
            for extra in [{}, {"Cookie": "test=value"}, {"Authorization": "Bearer test"}]:
                headers = {"Host": self.host, "Origin": "https://app.example", **extra}
                if method == "OPTIONS":
                    headers["Access-Control-Request-Method"] = "GET"
                code, response, _ = request(self.args.gateway_port, path, method, headers)
                require(code == expected_status, f"{path} {method}: HTTP {code}")
                require(response.get("access-control-expose-headers") == expected, "Exposure header differs")
                require(response.get("access-control-allow-credentials") == ("true" if credentials else None),
                        "Credentials header differs")
                require(response.get("access-control-allow-origin") == "https://app.example", "Origin differs")

    def run(self):
        prefix = "/echo/" + self.name
        rules = []
        for suffix, credentials, exposed in [
                ("invalid", True, ["*"]), ("mixed", True, ["X-Request-Id", "*"]),
                ("explicit", True, ["X-Request-Id", "X-Total-Count"]),
                ("wildcard", False, ["*"]), ("default", None, ["*"]), ("empty", True, [])]:
            rule = self.rule(prefix + "/" + suffix, rule_cors=True)
            config = rule["filters"][0]["cors"]
            config["exposeHeaders"] = exposed
            if credentials is not None:
                config["allowCredentials"] = credentials
            rules.append(rule)
        obj = self.route(self.name, rules)
        self.publish(obj)

        def invalid_phase():
            self.state(self.name, [0, 1])
            for suffix in ["invalid", "mixed"]:
                for method in ["GET", "OPTIONS"]:
                    self.traffic(prefix + "/" + suffix, method, 404)
            self.exposed(prefix + "/explicit", "X-Request-Id, X-Total-Count", True)
            self.exposed(prefix + "/wildcard", "*", False)
            self.exposed(prefix + "/default", "*", False)
            self.exposed(prefix + "/empty", None, True)
        self.wait_phase("invalid combinations rejected; explicit/default/empty wire behavior", invalid_phase)
        fixed = copy.deepcopy(obj)
        for rule in fixed["spec"]["rules"][:2]:
            rule["filters"][0]["cors"]["exposeHeaders"] = ["X-Request-Id"]
        self.publish(fixed, update=True)

        def recovered():
            self.state(self.name, [])
            for suffix in ["invalid", "mixed"]:
                self.exposed(prefix + "/" + suffix, "X-Request-Id", True)
        self.wait_phase("explicit lists restore both rules", recovered)
        self.publish(obj, update=True)
        self.wait_phase("invalid update removes previously active rules", invalid_phase)
        sole = self.route(self.name + "-sole", [copy.deepcopy(rules[0])])
        sole["spec"]["rules"][0]["matches"][0]["path"]["value"] = prefix + "/sole"
        self.publish(sole)

        def sole_invalid():
            self.state(self.name + "-sole", [0], rejected=True)
            for method in ["GET", "OPTIONS"]:
                self.traffic(prefix + "/sole", method, 404)
        self.wait_phase("sole invalid rule rejects attachment", sole_invalid)
        sole["spec"]["rules"][0]["filters"][0]["cors"]["exposeHeaders"] = ["X-Request-Id"]
        self.publish(sole, update=True)

        def sole_fixed():
            self.state(self.name + "-sole", [])
            self.exposed(prefix + "/sole", "X-Request-Id", True)
        self.wait_phase("sole rule recovers", sole_fixed)

        child_name, parent_name = self.name + "-child", self.name + "-parent"
        child_path = prefix + "/delegated"
        child_rule = copy.deepcopy(rules[0])
        child_rule["matches"][0]["path"]["value"] = child_path
        child = self.route(child_name, [child_rule], [{
            "group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": parent_name}])
        del child["spec"]["hostnames"]
        parent = self.route(parent_name, [{
            "matches": [{"path": {"type": "PathPrefix", "value": child_path}}],
            "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": child_name}]}])
        self.publish(child)
        self.publish(parent)

        def delegation_state(invalid):
            # Cascading match: the child is published on its own with its own
            # invalid indices and confirmed delegation key; the parent carries
            # only its authored rules and no child-caused diagnostic (§4.2, §7.1).
            current = self.read_route(child_name)
            require(current["spec"].get("invalidRuleIndices", []) == ([0] if invalid else []),
                    "Child validation has not converged")
            published_child = self.read_route(child_name, True)
            require(published_child is not None
                    and published_child["spec"].get("invalidRuleIndices", []) == ([0] if invalid else []),
                    "Child publication has not converged")
            keys = published_child["spec"].get("resolvedDelegationKeys", [])
            require([(k.get("parent_ns"), k.get("parent_name")) for k in keys]
                    == [(self.args.namespace, parent_name)], f"Child delegation keys differ: {keys}")
            child_conditions = [c for p in current.get("status", {}).get("parents", [])
                                for c in p.get("conditions", [])]
            require(any("exposeHeaders" in c.get("message", "") for c in child_conditions) == invalid,
                    "Child exposeHeaders diagnostic differs")
            controller = self.read_route(parent_name)
            published = self.read_route(parent_name, True)
            require(published is not None and "resolvedRules" not in published["spec"],
                    "Parent publication carries flattened rules")
            require(published["spec"] == controller["spec"], "Parent publication differs")
            conditions = [c for p in controller.get("status", {}).get("parents", [])
                          for c in p.get("conditions", [])]
            require(not any("exposeHeaders" in c.get("message", "") for c in conditions),
                    "Child-caused diagnostic appears on the parent")
            if invalid:
                for method in ["GET", "OPTIONS"]:
                    self.traffic(child_path, method, 404)
            else:
                self.exposed(child_path, "X-Request-Id", True)
        self.wait_phase("delegated invalid child rule is not served", lambda: delegation_state(True))
        child["spec"]["rules"][0]["filters"][0]["cors"]["exposeHeaders"] = ["X-Request-Id"]
        self.publish(child, update=True)
        self.wait_phase("child correction is served through the parent", lambda: delegation_state(False))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name, default in [("controller-port", 12101), ("gateway-admin-port", 12001),
                          ("gateway-port", 31000), ("backend-port", 30001)]:
        parser.add_argument("--" + name, type=int, default=default)
    parser.add_argument("--namespace", default="edgion-test")
    parser.add_argument("--gateway", default="http-test-gateway")
    parser.add_argument("--service", default="test-http")
    parser.add_argument("--timeout", type=float, default=30)
    scenario = ExposureScenario(parser.parse_args())
    scenario.name = scenario.name.replace("d0084-", "d0086-")
    scenario.host = scenario.name + ".example.com"
    try:
        scenario.run()
    finally:
        scenario.cleanup()


if __name__ == "__main__":
    main()
