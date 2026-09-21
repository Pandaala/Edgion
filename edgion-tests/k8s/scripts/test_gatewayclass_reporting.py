#!/usr/bin/env python3
"""Exercise status-only GatewayClass hot updates against an existing K8s Controller.

Reuses the running topology; creates only uniquely named GatewayClasses and
deletes them in finally. Requires the selected class to be Accepted and the
Controller admin URL/token. This is a focused regression, not conformance.
"""

import argparse
import json
import os
import subprocess
import time
import urllib.request
import uuid


def kubectl(*args, body=None):
    result = subprocess.run(
        ["kubectl", "--request-timeout=10s", *args],
        input=json.dumps(body) if body is not None else None,
        text=True, capture_output=True, check=True, timeout=15,
    )
    return json.loads(result.stdout) if result.stdout.strip() else None


def wait_for(check, description):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError(f"Timed out waiting for {description}")


def accepted(value):
    return next((condition for condition in value.get("status", {}).get("conditions", [])
                 if condition["type"] == "Accepted"), {})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gateway-class", required=True)
    parser.add_argument("--controller-url", required=True)
    args = parser.parse_args()
    token = os.environ["EDGION_ADMIN_TOKEN"]
    selected = kubectl("get", "gatewayclass", args.gateway_class, "-o", "json")
    assert accepted(selected).get("status") == "True", "selected class must be accepted"
    name = f"reporting-{uuid.uuid4().hex[:16]}"
    owner = selected["spec"]["controllerName"]
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    created = False

    def get():
        return kubectl("get", "gatewayclass", name, "-o", "json")

    def rejected():
        value = get()
        condition = accepted(value)
        if (condition.get("status") == "False" and condition.get("reason") == "Unsupported"
                and condition.get("observedGeneration") == value["metadata"]["generation"]
                and value.get("status", {}).get("supportedFeatures") == []):
            return value
        return None

    def not_published():
        request = urllib.request.Request(
            args.controller_url.rstrip("/") + "/configserver/GatewayClass/list",
            headers={"Authorization": f"Bearer {token}"},
        )
        with opener.open(request, timeout=10) as response:
            values = json.load(response)["data"]
        assert isinstance(values, list), "expected published class list"
        return not any(value["metadata"]["name"] == name for value in values)

    def create():
        # Deliberately omit labels: global label_selector must not hide this class.
        return kubectl("create", "-f", "-", "-o", "json", body={
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
            "metadata": {"name": name}, "spec": {"controllerName": owner},
        })

    try:
        create()
        created = True
        first = wait_for(rejected, "unsupported status on an unlabelled class")
        wait_for(not_published, "absence from publication cache")
        kubectl("patch", "gatewayclass", name, "--type=merge", "-p",
                json.dumps({"spec": {"description": "Updated unselected class"}}), "-o", "json")
        wait_for(rejected, "updated observedGeneration")
        kubectl("patch", "gatewayclass", name, "--subresource=status", "--type=merge", "-p",
                json.dumps({"status": {"supportedFeatures": [{"name": "Gateway"}]}}), "-o", "json")
        stable = wait_for(rejected, "clearing stale supportedFeatures")
        # Observe a bounded quiet interval; this is an oscillation assertion,
        # not a fixed delay used to assume reconciliation has completed.
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            assert get()["metadata"]["resourceVersion"] == stable["metadata"]["resourceVersion"], \
                "status-only class keeps being rewritten"
            time.sleep(0.1)
        kubectl("delete", "gatewayclass", name, "--wait=true", "-o", "json")
        created = False
        create()
        created = True
        recreated = wait_for(rejected, "recreated class status")
        assert recreated["metadata"]["uid"] != first["metadata"]["uid"]
        wait_for(not_published, "recreated class remains unpublished")
        assert accepted(kubectl("get", "gatewayclass", args.gateway_class, "-o", "json"))["status"] == "True"
        print("PASS: unselected GatewayClass reporting, updates, feature clearing, stable echoes, recreation")
    finally:
        if created:
            kubectl("delete", "gatewayclass", name, "--ignore-not-found", "--wait=true", "-o", "json")


if __name__ == "__main__":
    main()
