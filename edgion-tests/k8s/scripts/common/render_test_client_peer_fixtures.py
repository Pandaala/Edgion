#!/usr/bin/env python3
"""Render only the reviewed peer CIDR fields, retaining fixture semantics."""
import argparse
import copy
import ipaddress
import json
from pathlib import Path
import re
import subprocess

import yaml


class FixtureLoader(yaml.SafeLoader):
    pass


# YAML 1.2 boolean semantics; never turn a plugin's string `off` into False.
FixtureLoader.yaml_implicit_resolvers = {
    key: [(tag, pattern) for tag, pattern in resolvers if tag != "tag:yaml.org,2002:bool"]
    for key, resolvers in yaml.SafeLoader.yaml_implicit_resolvers.items()
}
FixtureLoader.add_implicit_resolver("tag:yaml.org,2002:bool", re.compile(r"^(?:true|false)$", re.I), list("tTfF"))

ECD = "/spec/data/config/items/0/cidrs/0"
CATALOG = [
    ("IpRestrictionRefs/01_EdgionConfigData_ipref-allowlist.yaml", "EdgionConfigData", "edgion-default", "ipref-allowlist", ECD, "192.168.0.0/16", "hit", "loopback", None),
    ("IpRestrictionRefs/01_EdgionConfigData_ipref-blocklist.yaml", "EdgionConfigData", "edgion-default", "ipref-blocklist", ECD, "192.168.0.0/16", "hit", "blocked", None),
    ("IpRestrictionPartialGood/01_EdgionConfigData_ippg-blocklist-hit.yaml", "EdgionConfigData", "edgion-default", "ippg-blocklist-hit", ECD, "192.168.0.0/16", "hit", "mixed-good-and-bad", "not-a-cidr"),
    ("IpRestrictionPartialGood/01_EdgionConfigData_ippg-blocklist-miss.yaml", "EdgionConfigData", "edgion-default", "ippg-blocklist-miss", ECD, "10.0.0.0/8", "miss", "mixed-good-and-bad", "999.0.0.1/8"),
    ("IpRestrictionPartialGood/00_EdgionConfigData_acl-ip-restriction-inline-de-deny-66dd1d77d6.yaml", "EdgionConfigData", "edgion-default", "acl-ip-restriction-inline-de-deny-66dd1d77d6", ECD, "192.168.0.0/16", "hit", "mixed-good-and-bad", "not-a-cidr"),
    ("IpRestrictionPartialGood/00_EdgionConfigData_acl-ip-restriction-inline-de-deny-f1570dac16.yaml", "EdgionConfigData", "edgion-default", "acl-ip-restriction-inline-de-deny-f1570dac16", ECD, "10.0.0.0/8", "miss", "mixed-good-and-bad", "not-a-cidr"),
    ("RequestAccessPolicy/00_EdgionConfigData_gac-lockdown-ips.yaml", "EdgionConfigData", "edgion-test", "gac-lockdown-ips", ECD, "192.168.0.0/16", "hit", "main", None),
    ("RequestAccessPolicy/00_EdgionConfigData_gac-private-ips.yaml", "EdgionConfigData", "edgion-test", "gac-private-ips", ECD, "10.0.0.0/8", "miss", "main", None),
    ("RequestAccessPolicy/08_EdgionConfigData_edgion-test_gac-url-loopback.yaml", "EdgionConfigData", "edgion-test", "gac-url-loopback", ECD, "192.168.0.0/16", "hit", "loopback", None),
]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def cidrs(uid, address):
    require(bool(uid) and not any(char.isspace() for char in uid), "invalid Pod UID")
    peer = ipaddress.ip_address(address)
    hit = str(ipaddress.ip_network(f"{peer}/{peer.max_prefixlen}"))
    candidates = ["198.51.100.0/24", "203.0.113.0/24"] if peer.version == 4 else ["2001:db8::/64", "2001:db8:1::/64"]
    miss = next(candidate for candidate in candidates if peer not in ipaddress.ip_network(candidate))
    require(peer not in ipaddress.ip_network("192.0.2.0/24"), "RAP normal profile would match this peer")
    return str(peer), hit, miss


def pointer_parent(document, pointer):
    parts = pointer.strip("/").split("/")
    current = document
    for part in parts[:-1]:
        current = current[int(part)] if isinstance(current, list) else current[part]
    return current, int(parts[-1]) if isinstance(current, list) else parts[-1]


RAP_CONSUMERS = [
    ("RequestAccessPolicy/03_EdgionPlugins_edgion-test_gac-lockdown.yaml", "gac-lockdown", "gac-lockdown-ips"),
    ("RequestAccessPolicy/04_EdgionPlugins_edgion-test_gac-ip-outside.yaml", "gac-ip-outside", "gac-private-ips"),
]
RAP_DOCUMENTATION = "RequestAccessPolicy/00_EdgionConfigData_gac-documentation-ips.yaml"


def assert_rap_consumers(root):
    for relative, name, target in RAP_CONSUMERS:
        documents = list(yaml.load_all((root / "EdgionPlugins" / relative).read_text(), Loader=FixtureLoader))
        require(len(documents) == 1, f"RAP document count changed: {relative}")
        source = documents[0]
        require(source["apiVersion"] == "edgion.io/v1" and source["kind"] == "EdgionPlugins" and source["metadata"]["name"] == name and source["metadata"]["namespace"] == "edgion-test", f"RAP identity changed: {relative}")
        plugins = source["spec"]["requestPlugins"]
        require(len(plugins) == 1 and plugins[0]["enable"] is True and plugins[0]["type"] == "RequestAccessPolicy", f"RAP entry changed: {relative}")
        config = plugins[0]["config"]
        require(config["defaultProfile"] == "lockdown", f"RAP default profile changed: {relative}")
        groups = config["profiles"]["lockdown"]["requiredRuleGroups"]
        require(len(groups) == 1 and groups[0]["name"] == "source" and len(groups[0]["anyOfRules"]) == 1, f"RAP source group changed: {relative}")
        rule = groups[0]["anyOfRules"][0]
        require(rule["name"] == "cloudflare" and rule["type"] == "directPeerIp" and rule["config"]["ipRefs"] == [{"name": target}] and "ips" not in rule["config"], f"RAP list reference changed: {relative}")
        require(rule["config"]["conditions"] == {"allOf": [{"type": "secretMatch", "key": {"type": "header", "name": "X-CF-Secret"}, "secretRef": {"name": "gac-origin-secret", "namespace": "edgion-test"}, "secretKey": "token"}]}, f"RAP Secret condition changed: {relative}")
        normal = config["profiles"]["normal"]["requiredRuleGroups"]
        require(len(normal) == 1 and normal[0]["name"] == "source" and normal[0]["anyOfRules"] == [{"name": "none", "type": "directPeerIp", "config": {"ipRefs": [{"name": "gac-documentation-ips"}]}}], f"RAP normal profile changed: {relative}")
    documentation = list(yaml.load_all((root / "EdgionPlugins" / RAP_DOCUMENTATION).read_text(), Loader=FixtureLoader))
    require(len(documentation) == 1, "RAP documentation list document count changed")
    source = documentation[0]
    require(source["apiVersion"] == "edgion.io/v1" and source["kind"] == "EdgionConfigData" and source["metadata"]["name"] == "gac-documentation-ips" and source["metadata"]["namespace"] == "edgion-test", "RAP documentation list identity changed")
    require(source["spec"]["data"] == {"type": "IpList", "config": {"items": [{"name": "main", "cidrs": ["192.0.2.0/24"]}]}}, "RAP normal exclusion changed")


def render(root, uid, address):
    address, hit, miss = cidrs(uid, address)
    assert_rap_consumers(root)
    result = []
    for relative, kind, namespace, name, pointer, original, choice, ancestor, invalid in CATALOG:
        documents = list(yaml.load_all((root / "EdgionPlugins" / relative).read_text(), Loader=FixtureLoader))
        require(len(documents) == 1, f"expected one source document: {relative}")
        source = documents[0]
        require(source["apiVersion"] == "edgion.io/v1" and source["kind"] == kind, f"resource type changed: {relative}")
        require(source["metadata"]["name"] == name and source["metadata"]["namespace"] == namespace, f"resource identity changed: {relative}")
        parent, index = pointer_parent(source, pointer)
        require(parent == ([original, invalid] if invalid else [original]), f"CIDR siblings changed: {relative}")
        data = source["spec"]["data"]
        require(pointer == ECD and data["type"] == "IpList" and len(data["config"]["items"]) == 1 and data["config"]["items"][0]["name"] == ancestor, f"IpList shape changed: {relative}")
        document = copy.deepcopy(source)
        target, index = pointer_parent(document, pointer)
        target[index] = hit if choice == "hit" else miss
        annotations = document["metadata"].setdefault("annotations", {})
        annotations.update({"edgion.io/test-client-pod-uid": uid, "edgion.io/test-client-pod-ip": address})
        result.append(document)
    return result


def assert_spec(live, desired):
    spec = copy.deepcopy(live["spec"])
    spec.pop("currentStatus", None)
    expected = copy.deepcopy(desired["spec"])
    if desired["kind"] == "EdgionConfigData":
        # Structural CRD defaults are part of the admitted contract.
        for key, default in (("enable", True), ("visibility", "Namespace")):
            if key not in expected:
                expected[key] = default
                spec.setdefault(key, default)
    require(spec == expected, f"prepared peer fixture changed: {desired['metadata']['name']}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["render", "assert", "environment"])
    parser.add_argument("--conf-root", type=Path, required=True)
    parser.add_argument("--uid", required=True)
    parser.add_argument("--ip", required=True)
    args = parser.parse_args()
    address, hit, miss = cidrs(args.uid, args.ip)
    documents = render(args.conf_root, args.uid, address)
    if args.mode != "render":
        for desired in documents:
            metadata = desired["metadata"]
            output = subprocess.check_output(["kubectl", "get", desired["kind"], metadata["name"], "--namespace", metadata["namespace"], "--output", "json", "--request-timeout=5s"], text=True)
            live = json.loads(output)
            for key, value in metadata["annotations"].items():
                require(live.get("metadata", {}).get("annotations", {}).get(key) == value, f"stale rendered identity: {metadata['name']}")
            assert_spec(live, desired)
    if args.mode == "render":
        print(yaml.safe_dump_all(documents, sort_keys=False), end="")
    elif args.mode == "environment":
        print("\t".join([args.uid, address, hit, miss]))


if __name__ == "__main__":
    main()
