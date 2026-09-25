#!/usr/bin/env python3
"""Provision one manually managed, Gateway-scoped data plane instance.

The Gateway and the Deployment may live in different namespaces, so a cross-
namespace ownerReference is invalid. The Service carries an explicit Gateway
reference and UID; rerun this command after listener changes to roll the Pod.
"""

import argparse
import hashlib
import json
import subprocess
import sys


SYSTEM_NAMESPACE = "edgion-system"
SOURCE_NAME = "edgion-gateway"
REF_ANNOTATION = "edgion.io/gateway-service-ref"
UID_ANNOTATION = "edgion.io/gateway-service-uid"
FINGERPRINT_ANNOTATION = "edgion.io/gateway-listener-fingerprint"
DEDICATED_ANNOTATION = "edgion.io/dedicated-gateway-instance"


def kubectl(*args, input_data=None):
    result = subprocess.run(
        ["kubectl", *args], input=input_data, text=True, capture_output=True, check=False
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or f"kubectl {' '.join(args)} failed")
    return result.stdout


def get_resource(kind, name, namespace):
    return json.loads(kubectl("get", kind, name, "-n", namespace, "-o", "json"))


def existing_owner(kind, name):
    result = subprocess.run(
        ["kubectl", "get", kind, name, "-n", SYSTEM_NAMESPACE, "-o", "json"],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        if "NotFound" in result.stderr or "not found" in result.stderr:
            return None
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout).get("metadata", {}).get("annotations", {})


def assert_owner(kind, name, gateway_ref, uid):
    annotations = existing_owner(kind, name)
    if annotations is not None and (
        annotations.get(REF_ANNOTATION) != gateway_ref
        or annotations.get(UID_ANNOTATION) != uid
    ):
        raise RuntimeError(f"{kind}/{name} exists without matching Gateway ref and UID")


def traffic_ports(gateway):
    ports = {}
    for listener in gateway["spec"].get("listeners", []):
        port = listener["port"]
        protocol = "UDP" if listener["protocol"].upper() == "UDP" else "TCP"
        ports[(port, protocol)] = {
            "name": f"{protocol.lower()}-{port}",
            "port": port,
            "targetPort": port,
            "protocol": protocol,
        }
    return [ports[key] for key in sorted(ports)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("gateway", help="Gateway namespace/name")
    parser.add_argument("--service-type", choices=["ClusterIP", "LoadBalancer"], default="ClusterIP")
    parser.add_argument("--dry-run", action="store_true", help="Print the generated resources without applying them")
    parser.add_argument("--wait", action="store_true", help="Wait for the scoped Deployment rollout")
    parser.add_argument("--restart", action="store_true", help="Roll the scoped Deployment after applying changes")
    parser.add_argument("--delete", action="store_true", help="Delete only an instance with the matching Gateway UID")
    parser.add_argument("--uid", help="Gateway UID for deleting an instance after Gateway deletion")
    args = parser.parse_args()
    if args.delete and (args.dry_run or args.restart or args.wait):
        parser.error("--delete cannot be combined with --dry-run, --restart, or --wait")
    namespace, separator, name = args.gateway.partition("/")
    if not separator or not namespace or not name or "/" in name:
        parser.error("gateway must have namespace/name format")

    if args.uid and not args.delete:
        parser.error("--uid is only valid with --delete")
    gateway = None if args.uid else get_resource("gateway", name, namespace)
    uid = args.uid or gateway["metadata"]["uid"]
    instance = f"edgion-gw-{hashlib.sha256(f'{args.gateway}/{uid}'.encode()).hexdigest()[:16]}"
    assert_owner("deployment", instance, args.gateway, uid)
    assert_owner("service", instance, args.gateway, uid)
    if args.delete:
        kubectl("delete", "deployment,service", instance, "-n", SYSTEM_NAMESPACE, "--ignore-not-found")
        return

    if gateway is None:
        raise RuntimeError("Gateway is required for provisioning")

    ports = traffic_ports(gateway)
    if not ports:
        raise RuntimeError("Gateway has no listeners")
    labels = {"app": instance}
    annotations = {REF_ANNOTATION: args.gateway, UID_ANNOTATION: uid}
    fingerprint = hashlib.sha256(
        json.dumps(gateway["spec"].get("listeners", []), sort_keys=True).encode()
    ).hexdigest()

    source_deployment = get_resource("deployment", SOURCE_NAME, SYSTEM_NAMESPACE)
    deployment_spec = source_deployment["spec"]
    deployment_spec["selector"] = {"matchLabels": labels}
    deployment_spec["template"]["metadata"]["labels"] = labels
    deployment_spec["template"]["metadata"].setdefault("annotations", {})[FINGERPRINT_ANNOTATION] = fingerprint
    pod_spec = deployment_spec["template"]["spec"]
    # Dedicated instances can share a node and listener port because each Pod
    # has its own network namespace. The source Deployment may use hostNetwork.
    pod_spec["hostNetwork"] = False
    pod_spec["dnsPolicy"] = "ClusterFirst"
    container = pod_spec["containers"][0]
    container.setdefault("env", []).append({"name": "EDGION_GATEWAY_SCOPE", "value": args.gateway})
    container["env"].append({"name": "EDGION_GATEWAY_UID", "value": uid})
    container["ports"] = [
        {"name": port["name"], "containerPort": port["port"], "protocol": port["protocol"]}
        for port in ports
    ] + [
        {"name": "admin", "containerPort": 12001, "protocol": "TCP"},
        {"name": "probe", "containerPort": 12000, "protocol": "TCP"},
        {"name": "metrics", "containerPort": 12090, "protocol": "TCP"},
    ]
    deployment = {
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": instance,
            "namespace": SYSTEM_NAMESPACE,
            "labels": labels,
            "annotations": annotations,
        },
        "spec": deployment_spec,
    }
    service = {
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {
            "name": instance,
            "namespace": SYSTEM_NAMESPACE,
            "labels": labels,
            "annotations": annotations,
        },
        "spec": {"type": args.service_type, "selector": labels, "ports": ports},
    }
    manifest = json.dumps({"apiVersion": "v1", "kind": "List", "items": [deployment, service]})
    if args.dry_run:
        print(manifest)
        return
    kubectl("annotate", "gateway", name, "-n", namespace, f"{DEDICATED_ANNOTATION}=true", "--overwrite")
    kubectl("apply", "-f", "-", input_data=manifest)
    if args.restart:
        kubectl("rollout", "restart", f"deployment/{instance}", "-n", SYSTEM_NAMESPACE)
    if args.wait:
        kubectl("rollout", "status", f"deployment/{instance}", "-n", SYSTEM_NAMESPACE, "--timeout=120s")
    print(f"Provisioned {args.gateway} as {SYSTEM_NAMESPACE}/{instance}")


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as error:
        print(error, file=sys.stderr)
        sys.exit(1)
