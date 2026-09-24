#!/usr/bin/env python3
"""Check exact Gateway API release CRDs with a small, reproducible OpenAPI fixture corpus.

This checks JSON Schema portions of the OpenAPI v3 CRDs only. Kubernetes CEL,
defaulting, pruning, conversion, and cluster conformance are outside its scope.
"""

import argparse
import copy
import hashlib
import json
from pathlib import Path
import sys
import urllib.request

import yaml
from jsonschema import Draft202012Validator


VERSIONS = (
    "v1.0.0", "v1.1.0", "v1.1.1", "v1.2.0", "v1.2.1", "v1.3.0",
    "v1.4.0", "v1.4.1", "v1.5.0", "v1.5.1", "v1.6.0", "v1.6.1", "v1.6.2",
)
CHANNELS = ("standard", "experimental")
SOURCES = (
    ("GatewayClass", "v1"), ("GatewayClass", "v1beta1"),
    ("Gateway", "v1"), ("Gateway", "v1beta1"),
    ("HTTPRoute", "v1"), ("HTTPRoute", "v1beta1"),
    ("GRPCRoute", "v1"), ("TLSRoute", "v1"),
    ("ReferenceGrant", "v1"), ("ReferenceGrant", "v1beta1"),
    ("TCPRoute", "v1alpha2"), ("UDPRoute", "v1alpha2"),
    ("BackendTLSPolicy", "v1"),
)
ROOT = Path(__file__).resolve().parent
LOCK = ROOT / "artifacts.json"
FIXTURES = json.loads((ROOT / "fixtures.json").read_text())
FIELD_PATHS = (
    "spec.useDefaultGateways",
    "spec.rules[].retry",
    "spec.rules[].sessionPersistence",
    "spec.rules[].filters[].cors",
    "spec.rules[].filters[].externalAuth",
    "spec.rules[].backendRefs[].filters[].externalAuth",
)
DRIFT_CASES = (
    ("loosened-match-limit", "HTTPRoute", "v1.1.1", "v1.2.0", "spec.rules[].matches", "FAIL", "PASS"),
    ("newly-optional-address-value", "Gateway", "v1.2.1", "v1.3.0", "spec.addresses[].value", "FAIL", "PASS"),
    ("loosened-hostname-limit", "TLSRoute", "v1.5.1", "v1.6.0", "spec.hostnames", "FAIL", "PASS"),
    ("loosened-well-known-ca", "BackendTLSPolicy", "v1.4.1", "v1.5.0",
     "spec.validation.wellKnownCACertificates", "FAIL", "PASS"),
)
SCHEMA_CHANGE_CASES = (
    ("added-parent-ref-port", "HTTPRoute", "v1.0.0", "v1.1.0", "spec.parentRefs[].port", "type", None, "integer"),
    ("added-mirror-denominator-default", "HTTPRoute", "v1.2.1", "v1.3.0",
     "spec.rules[].filters[].requestMirror.fraction.denominator", "default", None, 100),
    ("corrected-listener-protocol-pattern", "Gateway", "v1.1.1", "v1.2.0",
     "spec.listeners[].protocol", "pattern",
     "^[a-zA-Z0-9]([-a-zSA-Z0-9]*[a-zA-Z0-9])?$|[a-z0-9]([-a-z0-9]*[a-z0-9])?(\\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*\\/[A-Za-z0-9]+$",
     "^[a-zA-Z0-9]([-a-zA-Z0-9]*[a-zA-Z0-9])?$|[a-z0-9]([-a-z0-9]*[a-z0-9])?(\\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*\\/[A-Za-z0-9]+$"),
)


def drift_fixture(case, kind):
    fixture = {"apiVersion": "gateway.networking.k8s.io/v1", "kind": kind,
               "metadata": {"name": "fixture"}, "spec": copy.deepcopy(FIXTURES[kind])}
    if case == "loosened-match-limit":
        fixture["spec"]["rules"][0]["matches"] = [{"path": {"type": "PathPrefix", "value": "/"}} for _ in range(9)]
    elif case == "newly-optional-address-value":
        fixture["spec"]["addresses"] = [{"type": "IPAddress"}]
    elif case == "loosened-hostname-limit":
        fixture["spec"]["hostnames"] = [f"x{index}.example.com" for index in range(17)]
    elif case == "loosened-well-known-ca":
        fixture["spec"]["validation"]["wellKnownCACertificates"] = "example.com/Custom"
    else:
        raise ValueError(case)
    return fixture


def field_at(schema, path):
    node = schema
    for component in path.split("."):
        array = component.endswith("[]")
        name = component[:-2] if array else component
        node = node.get("properties", {}).get(name)
        if node is None:
            return None
        if array:
            node = node.get("items", {})
    return node


def artifact(version, channel, cache, update, locked):
    url = f"https://github.com/kubernetes-sigs/gateway-api/releases/download/{version}/{channel}-install.yaml"
    key = f"{version}/{channel}"
    path = cache / f"{version}-{channel}-install.yaml"
    if not path.exists():
        with urllib.request.urlopen(url, timeout=45) as response:
            path.write_bytes(response.read())
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if not update and locked[key]["sha256"] != digest:
        raise ValueError(f"artifact digest mismatch: {key}")
    return path, {"url": url, "sha256": digest}


def find_schema(documents, kind, version):
    for crd in documents:
        if not isinstance(crd, dict) or crd.get("kind") != "CustomResourceDefinition":
            continue
        if crd["spec"]["names"]["kind"] != kind:
            continue
        for source in crd["spec"]["versions"]:
            if source["name"] == version and source.get("served", False):
                return source["schema"]["openAPIV3Schema"]
    return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", type=Path, default=Path("/tmp/edgion-gateway-api-bundles"))
    parser.add_argument("--update-lock", action="store_true")
    args = parser.parse_args()
    args.cache.mkdir(parents=True, exist_ok=True)
    locked = {} if args.update_lock else json.loads(LOCK.read_text())
    artifacts = {}
    results = []
    fields = []
    drift = []
    schema_changes = []
    corrected_validation = []
    failures = []
    for version in VERSIONS:
        for channel in CHANNELS:
            path, entry = artifact(version, channel, args.cache, args.update_lock, locked)
            artifacts[f"{version}/{channel}"] = entry
            documents = list(yaml.safe_load_all(path.read_bytes()))
            http_schema = find_schema(documents, "HTTPRoute", "v1")
            for field_path in FIELD_PATHS:
                field = field_at(http_schema, field_path) if http_schema else None
                fields.append([version, channel, "HTTPRoute", "v1", field_path,
                               "N/A" if http_schema is None else "present" if field is not None else "absent"])
            for kind, source_version in SOURCES:
                schema = find_schema(documents, kind, source_version)
                if schema is None:
                    results.append([version, channel, kind, source_version, "N/A"])
                    continue
                fixture = {"apiVersion": f"gateway.networking.k8s.io/{source_version}",
                           "kind": kind, "metadata": {"name": "fixture"}, "spec": FIXTURES[kind]}
                errors = sorted(Draft202012Validator(schema).iter_errors(fixture), key=lambda e: str(e.path))
                result = "PASS" if not errors else "FAIL"
                results.append([version, channel, kind, source_version, result])
                if errors:
                    failures.append(f"{version}/{channel}/{kind}/{source_version}: {errors[0].message}")
            if channel == "standard":
                for case, kind, before, after, field_path, before_expected, after_expected in DRIFT_CASES:
                    if version not in (before, after):
                        continue
                    schema = find_schema(documents, kind, "v1")
                    observed = "FAIL" if list(Draft202012Validator(schema).iter_errors(drift_fixture(case, kind))) else "PASS"
                    expected = before_expected if version == before else after_expected
                    drift.append([case, kind, "v1", field_path, version, observed, expected])
                    if observed != expected:
                        failures.append(f"drift case {case} at {version}: expected {expected}, got {observed}")
                for case, kind, before, after, field_path, attribute, before_expected, after_expected in SCHEMA_CHANGE_CASES:
                    if version not in (before, after):
                        continue
                    schema = find_schema(documents, kind, "v1")
                    field = field_at(schema, field_path)
                    observed = None if field is None else field.get(attribute)
                    expected = before_expected if version == before else after_expected
                    schema_changes.append([case, kind, "v1", field_path, version, attribute,
                                           json.dumps(observed, sort_keys=True), json.dumps(expected, sort_keys=True)])
                    if observed != expected:
                        failures.append(f"schema change {case} at {version}: expected {expected!r}, got {observed!r}")
                if version in ("v1.1.1", "v1.2.0"):
                    schema = find_schema(documents, "Gateway", "v1")
                    for protocol, expected in (("HTTP", "PASS"), ("example.com/Proto", "PASS"), ("_", "FAIL")):
                        fixture = {"apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
                                   "metadata": {"name": "fixture"}, "spec": copy.deepcopy(FIXTURES["Gateway"])}
                        fixture["spec"]["listeners"][0]["protocol"] = protocol
                        observed = "FAIL" if list(Draft202012Validator(schema).iter_errors(fixture)) else "PASS"
                        corrected_validation.append(["listener-protocol-regex-correction", "Gateway", "v1",
                                                     "spec.listeners[].protocol", version, protocol, observed, expected])
                        if observed != expected:
                            failures.append(f"listener protocol fixture {protocol!r} at {version}: {observed}")
    if args.update_lock:
        LOCK.write_text(json.dumps(artifacts, indent=2, sort_keys=True) + "\n")
    elif artifacts != locked:
        raise ValueError("release artifact set differs from the lock")
    output = ROOT / "openapi-results.tsv"
    output.write_text("release\tchannel\tkind\tsourceGVKVersion\tminimalFixtureOpenAPI\n" +
                      "\n".join("\t".join(row) for row in results) + "\n")
    (ROOT / "field-path-results.tsv").write_text(
        "release\tchannel\tkind\tsourceGVKVersion\tfieldPath\tOpenAPIFieldPresence\n" +
        "\n".join("\t".join(row) for row in fields) + "\n"
    )
    (ROOT / "drift-results.tsv").write_text(
        "case\tkind\tsourceGVKVersion\tfieldPath\tstandardRelease\tOpenAPIObserved\tOpenAPIExpected\n" +
        "\n".join("\t".join(row) for row in drift) + "\n"
    )
    (ROOT / "schema-change-results.tsv").write_text(
        "case\tkind\tsourceGVKVersion\tfieldPath\tstandardRelease\tattribute\tobserved\texpected\n" +
        "\n".join("\t".join(row) for row in schema_changes) + "\n"
    )
    (ROOT / "corrected-validation-results.tsv").write_text(
        "case\tkind\tsourceGVKVersion\tfieldPath\tstandardRelease\tfixtureValue\tOpenAPIObserved\tOpenAPIExpected\n" +
        "\n".join("\t".join(row) for row in corrected_validation) + "\n"
    )
    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"Checked {len(artifacts)} exact release artifacts and {len(results)} source rows: "
          f"{sum(row[-1] == 'PASS' for row in results)} PASS, "
          f"{sum(row[-1] == 'N/A' for row in results)} N/A, {len(failures)} FAIL")
    return bool(failures)


if __name__ == "__main__":
    sys.exit(main())
