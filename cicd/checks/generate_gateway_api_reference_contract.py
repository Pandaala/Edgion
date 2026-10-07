#!/usr/bin/env python3
"""Generate the finite source-GVK reference contract, without widening admission."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys
import yaml

from generate_gateway_api_transform_contract import ROOT, STANDARD, EXPERIMENTAL, strip_descriptions

HISTORICAL = ROOT / "config/gateway-api-source-schemas/grpcroute-v1alpha2-v1.1.1-experimental.yaml"
OUTPUT = ROOT / "edgion-resources/src/contracts/gateway_api_references.json"
SOURCES = (
    (STANDARD, "standard", "v1.6.2", {"GatewayClass": {"v1", "v1beta1"}, "Gateway": {"v1", "v1beta1"}, "ReferenceGrant": {"v1", "v1beta1"}, "GRPCRoute": {"v1"}, "BackendTLSPolicy": {"v1"}}),
    (EXPERIMENTAL, "experimental", "v1.6.2", {"HTTPRoute": {"v1", "v1beta1"}, "TLSRoute": {"v1", "v1alpha3"}, "TCPRoute": {"v1", "v1alpha2"}, "UDPRoute": {"v1", "v1alpha2"}, "BackendTLSPolicy": {"v1alpha3"}}),
    (HISTORICAL, "historicalGrpc", "v1.1.1", {"GRPCRoute": {"v1alpha2"}}),
)


def reference_fields(weight=True):
    return {name: True for name in ("group", "kind", "name", "namespace", "port") + (("weight",) if weight else ())}


def filters(mirror):
    fields = {"type": True, "extensionRef": True}
    markers = ["extensionRef", "ExtensionRef"]
    if mirror:
        fields["requestMirror"] = {"backendRef": {**reference_fields(weight=False), "_rules": ["self.group"]}}
        markers += ["requestMirror", "RequestMirror"]
    return {"_items": {**fields, "_rules": markers}, "_bounds": True}


def selected_tree(kind):
    if kind.endswith("Route"):
        backend = {**reference_fields(), "_rules": ["self.group"]}
        rule = {"backendRefs": {"_items": backend, "_bounds": True}}
        if kind in {"HTTPRoute", "GRPCRoute"}:
            rule["filters"] = filters(kind == "HTTPRoute")
            backend["filters"] = filters(kind == "HTTPRoute")
        return {"spec": {"parentRefs": True, "rules": {"_items": rule}}}
    if kind == "Gateway":
        validation = {"caCertificateRefs": True}
        return {"spec": {
            "gatewayClassName": True,
            "listeners": {"_items": {
                "allowedRoutes": {"kinds": True},
                "tls": {"certificateRefs": True, "mode": True, "options": True, "_rules": ["self.certificateRefs"]},
            }},
            "tls": {
                "backend": {"clientCertificateRef": True},
                "frontend": {"default": {"validation": validation}, "perPort": {"_items": {"tls": {"validation": validation}}}},
            },
        }}
    if kind == "GatewayClass":
        return {"spec": {"parametersRef": True}}
    if kind == "ReferenceGrant":
        return {"spec": {"from": True, "to": True}}
    if kind == "BackendTLSPolicy":
        return {"spec": {"targetRefs": True, "validation": {
            "caCertificateRefs": True, "wellKnownCACertificates": True, "_rules": ["caCertificateRefs"],
        }}}
    raise RuntimeError(f"unregistered reference owner {kind}")


def project_schema(node, tree, path="", inventory=None):
    if inventory is not None:
        inventory.append({"path": path, "keywords": strip_descriptions({key: value for key, value in node.items() if key not in {"properties", "items", "description", "title"}})})
    if tree is True:
        return strip_descriptions(node)
    result = {key: node[key] for key in ("type", "nullable") if key in node}
    if tree.get("_bounds") or "_items" in tree:
        result.update({key: node[key] for key in ("minItems", "maxItems", "uniqueItems", "x-kubernetes-list-type", "x-kubernetes-list-map-keys") if key in node})
    properties = node.get("properties", {})
    children = {}
    for key, child in tree.items():
        if key.startswith("_"):
            continue
        if key not in properties:
            raise RuntimeError(f"missing selected property {path}/{key}")
        children[key] = project_schema(properties[key], child, f"{path}/{key}", inventory)
    if children:
        result["properties"] = children
        required = [key for key in node.get("required", []) if key in children]
        if required:
            result["required"] = required
    if "_items" in tree:
        if "items" not in node:
            raise RuntimeError(f"missing selected items {path}")
        result["items"] = project_schema(node["items"], tree["_items"], f"{path}/*", inventory)
    if "_rules" in tree:
        rules = [rule for rule in node.get("x-kubernetes-validations", []) if any(marker in rule["rule"] for marker in tree["_rules"])]
        if rules:
            result["x-kubernetes-validations"] = strip_descriptions(rules)
    return result


def generate():
    contracts = []
    for artifact, channel, release, inventory in SOURCES:
        seen = set()
        with artifact.open(encoding="utf-8") as stream:
            for document in yaml.safe_load_all(stream):
                if not isinstance(document, dict) or document.get("kind") != "CustomResourceDefinition":
                    continue
                spec = document["spec"]
                kind = spec["names"]["kind"]
                if spec["group"] != "gateway.networking.k8s.io" or kind not in inventory:
                    continue
                for version in spec["versions"]:
                    if version["name"] not in inventory[kind]:
                        continue
                    scope_inventory = []
                    schema = project_schema(version["schema"]["openAPIV3Schema"], selected_tree(kind), inventory=scope_inventory)
                    canonical = json.dumps(schema, sort_keys=True, separators=(",", ":")).encode()
                    contracts.append({"group": spec["group"], "version": version["name"], "kind": kind, "channel": channel, "gatewayApiVersion": release, "artifact": str(artifact.relative_to(ROOT)), "selectedSha256": hashlib.sha256(canonical).hexdigest(), "schema": schema, "scopeInventory": scope_inventory})
                    seen.add((kind, version["name"]))
        expected = {(kind, version) for kind, versions in inventory.items() for version in versions}
        if seen != expected:
            raise RuntimeError(f"source inventory mismatch in {artifact}: {expected - seen}")
    return json.dumps({"contracts": contracts}, sort_keys=True, indent=2) + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    rendered = generate()
    if args.check:
        if not OUTPUT.exists() or OUTPUT.read_text(encoding="utf-8") != rendered:
            print(f"stale generated contract: {OUTPUT.relative_to(ROOT)}", file=sys.stderr)
            return 1
        return 0
    OUTPUT.write_text(rendered, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
