#!/usr/bin/env python3
"""Generate the scoped HTTPRoute request-transform validation contract."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys

import yaml


ROOT = Path(__file__).resolve().parents[2]
STANDARD = ROOT / "config/crd/gateway-api/gateway-api-standard-v1.6.2.yaml"
EXPERIMENTAL = ROOT / "config/crd/gateway-api/gateway-api-experimental-v1.6.2.yaml"
OUTPUT = ROOT / "edgion-resources/src/contracts/http_route_transform_v1_6.json"

TRANSFORM_RULE_MARKERS = (
    "requestRedirect",
    "urlRewrite",
    "RequestRedirect",
    "URLRewrite",
)


def http_route_v1_schema(path: Path) -> dict:
    with path.open(encoding="utf-8") as stream:
        for document in yaml.safe_load_all(stream):
            if not isinstance(document, dict) or document.get("kind") != "CustomResourceDefinition":
                continue
            spec = document.get("spec", {})
            if spec.get("group") != "gateway.networking.k8s.io":
                continue
            if spec.get("names", {}).get("kind") != "HTTPRoute":
                continue
            for version in spec.get("versions", []):
                if version.get("name") == "v1":
                    return version["schema"]["openAPIV3Schema"]
    raise RuntimeError(f"HTTPRoute/v1 schema not found in {path}")


def validation_inventory(node: dict) -> list[dict]:
    return strip_descriptions(node.get("x-kubernetes-validations", []))


def filter_list_contract(node: dict) -> dict:
    """Keep every list-level schema keyword except separately compiled CEL/items."""
    return strip_descriptions(
        {
            key: value
            for key, value in node.items()
            if key not in {"items", "x-kubernetes-validations"}
        }
    )


def select_transform_rules(node: dict, location: str) -> list[dict]:
    selected = [
        item
        for item in validation_inventory(node)
        if any(marker in item.get("rule", "") for marker in TRANSFORM_RULE_MARKERS)
    ]
    if not selected:
        raise RuntimeError(f"no transform CEL rules found at {location}")
    return selected


def strip_descriptions(value):
    if isinstance(value, dict):
        return {
            key: strip_descriptions(child)
            for key, child in value.items()
            if key not in {"description", "title"}
        }
    if isinstance(value, list):
        return [strip_descriptions(child) for child in value]
    return value


def selected_contract(path: Path) -> tuple[dict, dict]:
    schema = http_route_v1_schema(path)
    rule = schema["properties"]["spec"]["properties"]["rules"]["items"]
    rule_filter_list = rule["properties"]["filters"]
    rule_filter = rule_filter_list["items"]
    backend_filter_list = rule["properties"]["backendRefs"]["items"]["properties"]["filters"]
    backend_filter = backend_filter_list["items"]

    redirect = strip_descriptions(rule_filter["properties"]["requestRedirect"])
    rewrite = strip_descriptions(rule_filter["properties"]["urlRewrite"])
    if redirect != strip_descriptions(backend_filter["properties"]["requestRedirect"]):
        raise RuntimeError("rule and backend RequestRedirect schemas differ")
    if rewrite != strip_descriptions(backend_filter["properties"]["urlRewrite"]):
        raise RuntimeError("rule and backend URLRewrite schemas differ")

    filter_rules = select_transform_rules(rule_filter, "rule.filters.items")
    backend_filter_rules = select_transform_rules(backend_filter, "backendRefs.filters.items")
    if filter_rules != backend_filter_rules:
        raise RuntimeError("rule and backend transform filter CEL rules differ")

    filter_list_rules = select_transform_rules(rule_filter_list, "rule.filters")
    backend_filter_list_rules = select_transform_rules(backend_filter_list, "backendRefs.filters")
    if filter_list_rules != backend_filter_list_rules:
        raise RuntimeError("rule and backend transform filter-list CEL rules differ")

    matches = rule["properties"]["matches"]
    path_schema = matches["items"]["properties"]["path"]
    defaults = {
        "matches": matches.get("default"),
        "path": path_schema.get("default"),
        "pathType": path_schema["properties"]["type"].get("default"),
        "pathValue": path_schema["properties"]["value"].get("default"),
        "redirectStatusCode": redirect["properties"]["statusCode"].get("default"),
    }
    if any(value is None for value in defaults.values()):
        raise RuntimeError("one or more required Gateway API defaults are missing")

    filter_list_fragment = filter_list_contract(rule_filter_list)
    backend_filter_list_fragment = filter_list_contract(backend_filter_list)
    if filter_list_fragment != backend_filter_list_fragment:
        raise RuntimeError("rule and backend transform filter-list bounds differ")

    contract = {
        "fragments": {
            "filterList": filter_list_fragment,
            "requestRedirect": redirect,
            "urlRewrite": rewrite,
        },
        "rules": {
            "filter": filter_rules,
            "filterList": filter_list_rules,
            "rule": select_transform_rules(rule, "rule"),
        },
        "defaults": defaults,
    }
    inventory = {
        "ruleFilter": validation_inventory(rule_filter),
        "ruleFilterList": validation_inventory(rule_filter_list),
        "backendFilter": validation_inventory(backend_filter),
        "backendFilterList": validation_inventory(backend_filter_list),
        "rule": validation_inventory(rule),
    }
    return contract, inventory


def generate() -> str:
    standard, standard_inventory = selected_contract(STANDARD)
    experimental, experimental_inventory = selected_contract(EXPERIMENTAL)
    if standard != experimental:
        raise RuntimeError("selected HTTPRoute transform contract differs between standard and experimental bundles")

    canonical = json.dumps(standard, sort_keys=True, separators=(",", ":")).encode()
    output = {
        "source": {
            "group": "gateway.networking.k8s.io",
            "version": "v1",
            "kind": "HTTPRoute",
            "channel": "standard",
            "gatewayApiVersion": "v1.6.2",
            "selectedSha256": hashlib.sha256(canonical).hexdigest(),
            "scopeInventorySha256": {
                "standard": hashlib.sha256(
                    json.dumps(standard_inventory, sort_keys=True, separators=(",", ":")).encode()
                ).hexdigest(),
                "experimental": hashlib.sha256(
                    json.dumps(experimental_inventory, sort_keys=True, separators=(",", ":")).encode()
                ).hexdigest(),
            },
        },
        "scopeInventories": {
            "standard": standard_inventory,
            "experimental": experimental_inventory,
        },
        **standard,
    }
    return json.dumps(output, indent=2, sort_keys=True) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if the committed contract is stale")
    args = parser.parse_args()
    rendered = generate()
    if args.check:
        current = OUTPUT.read_text(encoding="utf-8") if OUTPUT.exists() else ""
        if current != rendered:
            print(f"stale generated contract: {OUTPUT.relative_to(ROOT)}", file=sys.stderr)
            return 1
        return 0
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(rendered, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
