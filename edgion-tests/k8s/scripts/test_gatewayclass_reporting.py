#!/usr/bin/env python3
"""Exercise matching and foreign GatewayClasses against an existing K8s Controller.

Reuses the running topology; creates only uniquely named GatewayClasses and
deletes them in finally. Requires a reference class to be Accepted and the
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
    output = result.stdout.strip()
    return json.loads(output) if output.startswith(("{", "[")) else output or None


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
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    created = False
    foreign_name = f"foreign-{uuid.uuid4().hex[:16]}"
    foreign_created = False

    def get():
        return kubectl("get", "gatewayclass", name, "-o", "json")

    def reconciled():
        value = get()
        condition = accepted(value)
        if (condition.get("status") == "True" and condition.get("reason") == "Accepted"
                and condition.get("observedGeneration") == value["metadata"]["generation"]
                and value.get("status", {}).get("supportedFeatures")):
            return value
        return None

    def publication_matches(expected):
        request = urllib.request.Request(
            args.controller_url.rstrip("/") + "/configserver/GatewayClass/list",
            headers={"Authorization": f"Bearer {token}"},
        )
        with opener.open(request, timeout=10) as response:
            values = json.load(response)["data"]
        assert isinstance(values, list), "expected published class list"
        assert not any(value["metadata"]["name"] == foreign_name for value in values)
        return any(value["metadata"]["name"] == name
                   and value["metadata"]["uid"] == expected["metadata"]["uid"]
                   and {key: item for key, item in value["spec"].items() if key != "currentStatus"}
                   == expected["spec"] for value in values)

    def create():
        # Deliberately omit labels: global label_selector must not hide this class.
        return kubectl("create", "-f", "-", "-o", "json", body={
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
            "metadata": {"name": name}, "spec": selected["spec"],
        })

    try:
        create()
        created = True
        first = wait_for(reconciled, "acceptance of an unlabelled matching class")
        wait_for(lambda: publication_matches(first), "matching class publication")
        kubectl("patch", "gatewayclass", name, "--type=merge", "-p",
                json.dumps({"spec": {"description": "Updated matching class"}}), "-o", "json")
        updated = wait_for(reconciled, "updated observedGeneration")
        wait_for(lambda: publication_matches(updated), "updated published spec")
        kubectl("patch", "gatewayclass", name, "--subresource=status", "--type=merge", "-p",
                json.dumps({"status": {"supportedFeatures": []}}), "-o", "json")
        stable = wait_for(reconciled, "restoring supportedFeatures")
        # Observe a bounded quiet interval; this is an oscillation assertion,
        # not a fixed delay used to assume reconciliation has completed.
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            assert get()["metadata"]["resourceVersion"] == stable["metadata"]["resourceVersion"], \
                "matching class keeps being rewritten"
            time.sleep(0.1)
        kubectl("delete", "gatewayclass", name, "--wait=true", "-o", "name")
        created = False
        create()
        created = True
        recreated = wait_for(reconciled, "recreated class status")
        assert recreated["metadata"]["uid"] != first["metadata"]["uid"]
        wait_for(lambda: publication_matches(recreated), "recreated class publication")
        kubectl("create", "-f", "-", "-o", "json", body={
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
            "metadata": {"name": foreign_name},
            "spec": {"controllerName": "foreign.example.com/controller"},
        })
        foreign_created = True
        foreign_status = {"conditions": [{"type": "Accepted", "status": "False",
            "reason": "Pending", "message": "Foreign controller observation",
            "lastTransitionTime": "2026-01-01T00:00:00Z", "observedGeneration": 1}]}
        kubectl("patch", "gatewayclass", foreign_name, "--subresource=status", "--type=merge",
                "-p", json.dumps({"status": foreign_status}), "-o", "json")
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            foreign = kubectl("get", "gatewayclass", foreign_name, "-o", "json")
            assert foreign.get("status") == foreign_status, "foreign status was overwritten"
            assert publication_matches(recreated), "matching publication disappeared"
            time.sleep(0.1)
        assert accepted(kubectl("get", "gatewayclass", args.gateway_class, "-o", "json"))["status"] == "True"
        print("PASS: matching GatewayClass publication, updates, stable echoes, recreation, foreign isolation")
    finally:
        if foreign_created:
            kubectl("delete", "gatewayclass", foreign_name, "--ignore-not-found", "--wait=true", "-o", "name")
        if created:
            kubectl("delete", "gatewayclass", name, "--ignore-not-found", "--wait=true", "-o", "name")


if __name__ == "__main__":
    main()
