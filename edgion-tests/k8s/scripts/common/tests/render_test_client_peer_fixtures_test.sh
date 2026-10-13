#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "${SCRIPT_DIR}/../render_test_client_peer_fixtures.py" "${SCRIPT_DIR}/../../../conf" <<'PY'
import copy
import importlib.util
import ipaddress
from pathlib import Path
import shutil
import sys
import tempfile

spec = importlib.util.spec_from_file_location("renderer", sys.argv[1])
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
root = Path(sys.argv[2])

def rejects(action):
    try:
        action()
    except (ValueError, KeyError):
        return
    raise AssertionError("Invalid contract unexpectedly accepted")

for address in ("10.244.0.7", "192.168.65.4", "198.51.100.8", "2001:db8::20"):
    docs = r.render(root, "uid-test", address)
    ip, hit, miss = r.cidrs("uid-test", address)
    assert ipaddress.ip_address(ip) in ipaddress.ip_network(hit)
    assert ipaddress.ip_address(ip) not in ipaddress.ip_network(miss)
    assert len(docs) == 9
    for row, desired in zip(r.CATALOG, docs):
        source = r.yaml.load((root / "EdgionPlugins" / row[0]).read_text(), Loader=r.FixtureLoader)
        expected = copy.deepcopy(source)
        parent, index = r.pointer_parent(expected, row[4])
        parent[index] = hit if row[6] == "hit" else miss
        expected["metadata"].setdefault("annotations", {}).update({"edgion.io/test-client-pod-uid": "uid-test", "edgion.io/test-client-pod-ip": ip})
        assert desired == expected
        live = copy.deepcopy(desired)
        if desired["kind"] == "EdgionConfigData":
            live["spec"].setdefault("enable", True)
            live["spec"].setdefault("visibility", "Namespace")
        live["spec"]["currentStatus"] = {"ignored": True}
        r.assert_spec(live, desired)
        bad = copy.deepcopy(live)
        bad["spec"]["unexpectedMeaningfulField"] = True
        rejects(lambda: r.assert_spec(bad, desired))
        if desired["kind"] == "EdgionConfigData":
            for key, wrong in (("enable", False), ("visibility", "Cluster"), ("active", "Namespace")):
                bad = copy.deepcopy(live)
                bad["spec"][key] = wrong
                rejects(lambda: r.assert_spec(bad, desired))

for uid, address in (("", "10.0.0.1"), ("bad uid", "10.0.0.1"), ("uid", "not-an-ip"), ("uid", "192.0.2.5")):
    rejects(lambda: r.cidrs(uid, address))

with tempfile.TemporaryDirectory() as directory:
    target = Path(directory)
    sources = [row[0] for row in r.CATALOG] + [row[0] for row in r.RAP_CONSUMERS] + [r.RAP_DOCUMENTATION]
    for relative in sources:
        path = target / "EdgionPlugins" / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(root / "EdgionPlugins" / relative, path)
    for relative, mutate in (
        (r.CATALOG[0][0], lambda d: d["metadata"].update(name="wrong")),
        (r.CATALOG[2][0], lambda d: d["spec"]["data"]["config"]["items"][0]["cidrs"].pop()),
        (r.RAP_CONSUMERS[0][0], lambda d: d["spec"]["requestPlugins"][0]["config"]["profiles"]["lockdown"]["requiredRuleGroups"][0]["anyOfRules"][0]["config"]["conditions"]["allOf"][0].update(secretKey="wrong")),
        (r.RAP_CONSUMERS[1][0], lambda d: d["metadata"].update(name="wrong")),
        (r.RAP_CONSUMERS[0][0], lambda d: d["spec"]["requestPlugins"][0]["config"].update(defaultProfile="normal")),
        (r.RAP_CONSUMERS[0][0], lambda d: d["spec"]["requestPlugins"][0]["config"]["profiles"]["lockdown"]["requiredRuleGroups"][0]["anyOfRules"][0]["config"].update(ipRefs=[{"name":"wrong"}])),
        (r.RAP_CONSUMERS[0][0], lambda d: d["spec"]["requestPlugins"][0]["config"]["profiles"]["normal"]["requiredRuleGroups"][0]["anyOfRules"][0]["config"].update(ipRefs=[{"name":"wrong"}])),
        (r.RAP_DOCUMENTATION, lambda d: d["spec"]["data"]["config"]["items"][0].update(cidrs=["198.51.100.0/24"])),
    ):
        path = target / "EdgionPlugins" / relative
        original = path.read_text()
        document = r.yaml.load(original, Loader=r.FixtureLoader)
        mutate(document)
        path.write_text(r.yaml.safe_dump(document))
        rejects(lambda: r.render(target, "uid-test", "10.244.0.7"))
        path.write_text(original)
print("render_test_client_peer_fixtures_test.sh: PASS")
PY
