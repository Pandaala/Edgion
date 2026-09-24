#!/usr/bin/env python3
"""Run locked Gateway API releases against a dedicated Kubernetes cluster."""

import argparse
import hashlib
import json
import os
import re
import selectors
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
K8S = ROOT / "edgion-tests/k8s"
ARTIFACTS = ROOT / "edgion-tests/gateway_api_bundles/artifacts.json"
TEST_NAMESPACES = ("edgion-default", "edgion-test")
NAMESPACES = ("edgion-system", *TEST_NAMESPACES)
SOURCES = {
    "GatewayClass": ("v1",), "Gateway": ("v1",), "HTTPRoute": ("v1",),
    "GRPCRoute": ("v1", "v1alpha2"), "ReferenceGrant": ("v1", "v1beta1"),
    "TCPRoute": ("v1", "v1alpha2"), "UDPRoute": ("v1", "v1alpha2"),
    "TLSRoute": ("v1", "v1alpha3"),
    "BackendTLSPolicy": ("v1", "v1alpha3"),
}


def call(context, *args, data=None, check=True, timeout=360):
    long_wait = args and (args[0] in ("wait", "rollout") or
                          (args[0] == "delete" and "--wait=true" in args))
    request_timeout = "300s" if long_wait else "30s"
    command = ["kubectl", "--context", context, f"--request-timeout={request_timeout}",
               *map(str, args)]
    result = subprocess.run(command, input=data, text=True, capture_output=True,
                            timeout=timeout, check=False)
    if check and result.returncode:
        raise RuntimeError(f"{' '.join(command)}: {result.stderr.strip() or result.stdout.strip()}")
    return result


def apply(context, obj):
    data = yaml.safe_dump(obj, sort_keys=False) if isinstance(obj, dict) else obj
    call(context, "apply", "--server-side", "--force-conflicts", "-f", "-", data=data)


def wait_condition(context, kind, name, namespace, condition="Accepted", seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = call(context, "get", kind, name, "-n", namespace, "-o", "json", check=False)
        if result.returncode == 0:
            obj = json.loads(result.stdout)
            conditions = (obj.get("status", {}).get("parents") or [{}])[0].get("conditions", [])
            if kind in ("gateway", "gatewayclass"):
                conditions = obj.get("status", {}).get("conditions", [])
            if kind == "backendtlspolicy":
                conditions = (obj.get("status", {}).get("ancestors") or [{}])[0].get("conditions", [])
            if any(c.get("type") == condition and c.get("status") == "True" for c in conditions):
                return
        time.sleep(2)
    raise RuntimeError(f"{kind}/{namespace}/{name} did not report {condition}=True")


def fetch_locked(version, channel, cache):
    cache.mkdir(parents=True, exist_ok=True)
    entry = json.loads(ARTIFACTS.read_text())[f"{version}/{channel}"]
    target = cache / f"{version}-{channel}.yaml"
    if not target.exists() or hashlib.sha256(target.read_bytes()).hexdigest() != entry["sha256"]:
        with urllib.request.urlopen(entry["url"], timeout=120) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != entry["sha256"]:
            raise RuntimeError(f"Gateway API artifact digest mismatch: {version}/{channel}")
        target.write_bytes(data)
    docs = [doc for doc in yaml.safe_load_all(target.read_text())
            if isinstance(doc, dict) and doc.get("kind") == "CustomResourceDefinition"]
    if not docs:
        raise RuntimeError(f"No CRDs in {target}")
    return docs


def served_versions(crds):
    result = {}
    for crd in crds:
        kind = crd["spec"]["names"]["kind"]
        if kind in SOURCES:
            versions = {v["name"] for v in crd["spec"]["versions"] if v.get("served")}
            result[kind] = next((v for v in SOURCES[kind] if v in versions), None)
    return result


def served(crds, kind, version):
    return any(crd["spec"]["names"]["kind"] == kind and
               any(v["name"] == version and v.get("served") for v in crd["spec"]["versions"])
               for crd in crds)


def resource(kind, version, name, namespace, spec):
    return {"apiVersion": f"gateway.networking.k8s.io/{version}", "kind": kind,
            "metadata": {"name": name, "namespace": namespace}, "spec": spec}


def fixtures(versions, beta):
    gateway_listeners = [{"name": "http", "protocol": "HTTP", "port": 80,
                          "allowedRoutes": {"namespaces": {"from": "All"}}}]
    for kind, protocol, port in (("GRPCRoute", "HTTP", 30021), ("TCPRoute", "TCP", 31092),
                                 ("UDPRoute", "UDP", 31101), ("TLSRoute", "TLS", 8443)):
        if versions.get(kind):
            listener = {"name": kind.lower(), "protocol": protocol, "port": port,
                        "allowedRoutes": {"kinds": [{"kind": kind}]}}
            if kind == "TLSRoute":
                listener["tls"] = {"mode": "Passthrough"}
            gateway_listeners.append(listener)
    objects = [resource("GatewayClass", versions["GatewayClass"], "compatible-gateway", "",
                        {"controllerName": "edgion.io/gateway-controller"}),
               resource("Gateway", versions["Gateway"], "compatible-gateway", "edgion-test",
                        {"gatewayClassName": "compatible-gateway", "listeners": gateway_listeners}),
               resource("HTTPRoute", versions["HTTPRoute"], "compatible-http", "edgion-test",
                        {"parentRefs": [{"name": "compatible-gateway", "sectionName": "http"}],
                         "rules": [{"backendRefs": [{"name": "test-http", "port": 30001}]}]})]
    if versions.get("ReferenceGrant"):
        objects.append(resource("ReferenceGrant", versions["ReferenceGrant"], "compatible-grant",
                                "edgion-test", {"from": [{"group": "gateway.networking.k8s.io",
                                "kind": "HTTPRoute", "namespace": "edgion-default"}],
                                "to": [{"group": "", "kind": "Service", "name": "test-http"}]}))
        objects.append(resource("HTTPRoute", versions["HTTPRoute"], "compatible-cross-namespace",
                                "edgion-default", {"parentRefs": [{"name": "compatible-gateway",
                                "namespace": "edgion-test", "sectionName": "http"}],
                                "hostnames": ["cross.compatible.example"],
                                "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": "/health"}}],
                                "backendRefs": [{"name": "test-http", "namespace": "edgion-test",
                                                 "port": 30001}]}]}))
    for kind, service, port in (("GRPCRoute", "test-grpc", 30021),
                                ("TCPRoute", "test-tcp", 30010),
                                ("UDPRoute", "test-udp", 30011),
                                ("TLSRoute", "test-https", 30051)):
        if versions.get(kind):
            spec = {"parentRefs": [{"name": "compatible-gateway", "sectionName": kind.lower()}],
                    "rules": [{"backendRefs": [{"name": service, "port": port}]}]}
            if kind == "TLSRoute":
                spec["hostnames"] = ["compatible.example.com"]
            objects.append(resource(kind, versions[kind], f"compatible-{kind.lower()}", "edgion-test", spec))
    if versions.get("BackendTLSPolicy"):
        objects.append(resource("HTTPRoute", versions["HTTPRoute"], "compatible-policy-http",
                                "edgion-test", {"parentRefs": [{"name": "compatible-gateway",
                                "sectionName": "http"}], "hostnames": ["policy.compatible.example"],
                                "rules": [{"backendRefs": [{"name": "test-tls-policy",
                                                             "port": 30051}]}]}))
        objects.append(resource("HTTPRoute", versions["HTTPRoute"], "compatible-policy-invalid-http",
                                "edgion-test", {"parentRefs": [{"name": "compatible-gateway",
                                "sectionName": "http"}], "hostnames": ["policy-invalid.compatible.example"],
                                "rules": [{"backendRefs": [{"name": "test-tls-policy-invalid",
                                                             "port": 30051}]}]}))
        objects.append(resource("BackendTLSPolicy", versions["BackendTLSPolicy"], "compatible-backend-tls",
                                "edgion-test", {"targetRefs": [{"group": "", "kind": "Service",
                                "name": "test-tls-policy"}], "validation": {"hostname": "test-backend-tls.edgion-test.svc.cluster.local",
                                "caCertificateRefs": [{"group": "", "kind": "Secret", "name": "backend-ca"}]}}))
        objects.append(resource("BackendTLSPolicy", versions["BackendTLSPolicy"], "compatible-backend-tls-invalid",
                                "edgion-test", {"targetRefs": [{"group": "", "kind": "Service",
                                "name": "test-tls-policy-invalid"}], "validation": {"hostname": "wrong.compatible.example",
                                "caCertificateRefs": [{"group": "", "kind": "Secret", "name": "backend-ca"}]}}))
    if beta:
        objects.extend([
            resource("GatewayClass", "v1beta1", "compatible-beta-class", "",
                     {"controllerName": "edgion.io/gateway-controller"}),
            resource("Gateway", "v1beta1", "compatible-beta-gateway", "edgion-test",
                     {"gatewayClassName": "compatible-beta-class",
                      "listeners": [{"name": "http", "protocol": "HTTP", "port": 10080}]}),
            resource("HTTPRoute", "v1beta1", "compatible-beta-http", "edgion-test",
                     {"parentRefs": [{"name": "compatible-beta-gateway"}],
                      "hostnames": ["beta.compatible.example"],
                      "rules": [{"backendRefs": [{"name": "test-http", "port": 30001}]}]})])
        if versions.get("ReferenceGrant") != "v1beta1":
            objects.append(resource("ReferenceGrant", "v1beta1", "compatible-beta-grant",
                                    "edgion-test", {"from": [{"group": "gateway.networking.k8s.io",
                                    "kind": "HTTPRoute", "namespace": "edgion-default"}],
                                    "to": [{"group": "", "kind": "Service", "name": "test-http-beta-grant"}]}))
            objects.append(resource("HTTPRoute", versions["HTTPRoute"], "compatible-beta-cross-namespace",
                                    "edgion-default", {"parentRefs": [{"name": "compatible-gateway",
                                    "namespace": "edgion-test", "sectionName": "http"}],
                                    "hostnames": ["cross-beta.compatible.example"],
                                    "rules": [{"backendRefs": [{"name": "test-http-beta-grant",
                                                                 "namespace": "edgion-test", "port": 30001}]}]}))
    return objects


def apply_file(context, path):
    call(context, "apply", "-f", path)


def apply_namespace_documents(context, path, namespace):
    for obj in yaml.safe_load_all(path.read_text()):
        if obj and obj["metadata"].get("namespace") == namespace:
            apply(context, obj)


def standard_suite(version, crds, versions, beta):
    directory = HERE / version / "standard"
    suite = yaml.safe_load((directory / "suite.yaml").read_text())
    expected_no_watch = [kind for kind in ("GRPCRoute", "TLSRoute", "TCPRoute",
                                           "UDPRoute", "BackendTLSPolicy") if not versions.get(kind)]
    if suite.get("release") != version or suite.get("channel") != "standard":
        raise RuntimeError(f"invalid Standard suite identity: {version}")
    if suite.get("noWatchKinds") != expected_no_watch:
        raise RuntimeError(f"Standard noWatchKinds inventory drift: {version}")
    objects = []
    for filename in suite["resources"]:
        path = directory / filename
        if path.parent != directory or path.suffix != ".yaml":
            raise RuntimeError(f"invalid Standard suite file: {filename}")
        objects.extend(obj for obj in yaml.safe_load_all(path.read_text()) if obj)
    sources = {}
    for obj in objects:
        kind, source = obj["kind"], obj["apiVersion"].split("/")[-1]
        if not served(crds, kind, source):
            raise RuntimeError(f"unserved Standard suite source: {version} {kind}/{source}")
        sources.setdefault(kind, set()).add(source)
    sources = {kind: sorted(items) for kind, items in sources.items()}
    if sources != suite["supportedSources"]:
        raise RuntimeError(f"Standard supportedSources inventory drift: {version}")
    required = {(obj["kind"], obj["apiVersion"].split("/")[-1])
                for obj in fixtures(versions, beta)}
    if not required.issubset({(kind, source) for kind, items in sources.items() for source in items}):
        raise RuntimeError(f"Standard suite misses a supported source: {version}")
    expected = sorted(json.dumps(obj, sort_keys=True) for obj in fixtures(versions, beta))
    actual = sorted(json.dumps(obj, sort_keys=True) for obj in objects)
    if actual != expected:
        raise RuntimeError(f"Standard suite resource content drift: {version}")
    return objects, expected_no_watch


def set_case_watch_config(context, no_watch_kinds):
    obj = json.loads(call(context, "get", "configmap", "edgion-controller-config",
                          "-n", "edgion-system", "-o", "json").stdout)
    document = yaml.safe_load(obj["data"]["edgion-controller.yaml"])
    document["conf_center"]["no_watch_kinds"] = no_watch_kinds
    patch_controller_config(context, yaml.safe_dump(document, sort_keys=False))


def deploy(context, crds, versions, beta, objects, no_watch_kinds, work, images, reuse=False):
    for crd in crds:
        apply(context, crd)
    for crd in crds:
        call(context, "wait", "--for=condition=Established", "--timeout=120s", "crd/" + crd["metadata"]["name"])
    if reuse:
        for obj in yaml.safe_load_all((K8S / "kubernetes/namespace.yaml").read_text()):
            if obj and obj["metadata"]["name"] in TEST_NAMESPACES:
                apply(context, obj)
    else:
        for path in sorted((ROOT / "config/crd/edgion-crd").glob("*.yaml")):
            apply_file(context, path)
        for obj in yaml.safe_load_all((K8S / "kubernetes/namespace.yaml").read_text()):
            if obj and obj["metadata"]["name"] in NAMESPACES:
                apply(context, obj)
    generated = work / "generated"
    subprocess.run([str(K8S / "scripts/common/generate_runtime_certs.sh"), str(generated)], check=True,
                   stdout=(work / "certs.log").open("w"))
    for path in sorted((generated / "secrets").glob("*.yaml")):
        # Some generated fixtures target namespaces outside this compact topology.
        docs = list(yaml.safe_load_all(path.read_text()))
        for obj in docs:
            if obj and obj["metadata"].get("namespace") in NAMESPACES and (
                    not reuse or obj["metadata"]["namespace"] != "edgion-system"):
                apply(context, obj)
    if reuse:
        set_case_watch_config(context, no_watch_kinds)
        call(context, "scale", "deployment/edgion-controller", "-n", "edgion-system", "--replicas=1")
    else:
        call(context, "-n", "edgion-system", "create", "secret", "generic",
             "edgion-gateway-admin-credentials", "--from-literal=token=integration-gateway-admin-token-0123456789")
        wasm = ROOT / "edgion-tests/wasm/artifacts/header_demo.wasm"
        call(context, "-n", "edgion-system", "create", "configmap", "edgion-test-wasm",
             f"--from-file=header_demo.wasm={wasm}")
        for component in ("controller", "gateway"):
            apply_file(context, K8S / f"kubernetes/{component}")
        set_case_watch_config(context, no_watch_kinds)
        call(context, "rollout", "restart", "deployment/edgion-controller", "-n", "edgion-system")
        for name, namespace in (("edgion-controller", "edgion-system"),
                                ("edgion-gateway", "edgion-system")):
            if images.get(name):
                call(context, "set", "image", f"deployment/{name}", "-n", namespace,
                     f"{name}={images[name]}")
    for path in ("deployment.yaml", "service.yaml"):
        apply_namespace_documents(context, K8S / f"kubernetes/test/test-server/{path}",
                                  "edgion-test")
    if images.get("test-server"):
        call(context, "set", "image", "deployment/edgion-test-server", "-n", "edgion-test",
             f"test-server={images['test-server']}")
    for path in ("Service_edgion-test_test-http.yaml", "Service_edgion-test_test-grpc.yaml",
                 "Service_edgion-test_test-tcp.yaml"):
        apply_file(context, K8S / "conf/base" / path)
    apply(context, {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "test-udp",
          "namespace": "edgion-test"}, "spec": {"selector": {"app": "edgion-test-server"},
          "ports": [{"port": 30011, "targetPort": 30011, "protocol": "UDP"}]}})
    apply(context, {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "test-http-beta-grant",
          "namespace": "edgion-test"}, "spec": {"selector": {"app": "edgion-test-server"},
          "ports": [{"port": 30001, "targetPort": 30001, "protocol": "TCP"}]}})
    apply(context, {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "test-tls-policy",
          "namespace": "edgion-test"}, "spec": {"selector": {"app": "edgion-test-server"},
          "ports": [{"port": 30051, "targetPort": 30051, "protocol": "TCP"}]}})
    apply(context, {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "test-tls-policy-invalid",
          "namespace": "edgion-test"}, "spec": {"selector": {"app": "edgion-test-server"},
          "ports": [{"port": 30051, "targetPort": 30051, "protocol": "TCP"}]}})
    apply(context, {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "test-https",
          "namespace": "edgion-test"}, "spec": {"selector": {"app": "edgion-test-server"},
          "ports": [{"port": 30051, "targetPort": 30051, "protocol": "TCP"}]}})
    for name, namespace in (("edgion-controller", "edgion-system"),
                            ("edgion-test-server", "edgion-test")):
        call(context, "rollout", "status", f"deployment/{name}", "-n", namespace,
             "--timeout=300s", timeout=330)
    for obj in objects:
        apply(context, obj)
    wait_condition(context, "gatewayclass", "compatible-gateway", "", "Accepted")
    wait_condition(context, "gateway", "compatible-gateway", "edgion-test", "Accepted")
    for obj in objects:
        if obj["kind"] == "GatewayClass" and obj["metadata"]["name"] != "compatible-gateway":
            wait_condition(context, "gatewayclass", obj["metadata"]["name"], "", "Accepted")
        if obj["kind"] == "Gateway" and obj["metadata"]["name"] != "compatible-gateway":
            wait_condition(context, "gateway", obj["metadata"]["name"], obj["metadata"]["namespace"])
        if obj["kind"] in ("HTTPRoute", "GRPCRoute", "TCPRoute", "UDPRoute", "TLSRoute"):
            wait_condition(context, obj["kind"].lower(), obj["metadata"]["name"],
                           obj["metadata"]["namespace"])
        if obj["kind"] == "BackendTLSPolicy":
            wait_condition(context, "backendtlspolicy", obj["metadata"]["name"],
                           obj["metadata"]["namespace"])
    if reuse:
        call(context, "scale", "deployment/edgion-gateway", "-n", "edgion-system", "--replicas=1")
    else:
        call(context, "rollout", "restart", "deployment/edgion-gateway", "-n", "edgion-system")
    call(context, "rollout", "status", "deployment/edgion-gateway", "-n", "edgion-system",
         "--timeout=300s", timeout=330)


PROBE_CODE = """
import socket
import ssl
import sys
import urllib.error
import urllib.request

mode, host, port = sys.argv[1], sys.argv[2], int(sys.argv[3])
if mode in ('http', 'http-fail'):
    request = urllib.request.Request(f'http://{host}:{port}/health', headers={'Host': sys.argv[4]})
    direct = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with direct.open(request, timeout=8) as response:
            status = response.status
    except urllib.error.HTTPError as error:
        status = error.code
    if mode == 'http':
        assert status == 200, status
    else:
        assert 500 <= status < 600, status
elif mode == 'tcp':
    with socket.create_connection((host, port), timeout=8) as stream:
        stream.sendall(b'compatible-tcp')
        assert stream.recv(64) == b'compatible-tcp'
elif mode == 'udp':
    family, socktype, protocol, _, address = socket.getaddrinfo(host, port, type=socket.SOCK_DGRAM)[0]
    with socket.socket(family, socktype, protocol) as stream:
        stream.settimeout(8)
        stream.sendto(b'compatible-udp', address)
        assert stream.recv(64) == b'compatible-udp'
elif mode == 'tls':
    tls = ssl.create_default_context()
    tls.check_hostname = False
    tls.verify_mode = ssl.CERT_NONE
    with socket.create_connection((host, port), timeout=8) as connection:
        with tls.wrap_socket(connection, server_hostname='compatible.example.com') as stream:
            stream.sendall(b'GET /health HTTP/1.1\\r\\nHost: compatible.example.com\\r\\nConnection: close\\r\\n\\r\\n')
            assert b' 200 ' in stream.recv(1024)
else:
    raise ValueError(mode)
"""


def traffic(context, cross, beta, versions):
    # One in-cluster probe Pod exercises real Gateway -> Service forwarding.
    call(context, "run", "compatible-probe", "-n", "edgion-test", "--restart=Never",
         "--image=python:3.12-alpine", "--command", "--", "sleep", "3600")
    call(context, "wait", "--for=condition=Ready", "--timeout=120s",
         "pod/compatible-probe", "-n", "edgion-test")
    hosts = [("compatible.example", 80)]
    if cross:
        hosts.append(("cross.compatible.example", 80))
        if versions["ReferenceGrant"] == "v1":
            hosts.append(("cross-beta.compatible.example", 80))
    if beta:
        hosts.append(("beta.compatible.example", 10080))
    checks = [("http", "edgion-gateway.edgion-system.svc.cluster.local", port, host)
              for host, port in hosts]
    if versions.get("BackendTLSPolicy"):
        checks.extend((mode, "edgion-gateway.edgion-system.svc.cluster.local", 80, host)
                      for mode, host in (("http", "policy.compatible.example"),
                                         ("http-fail", "policy-invalid.compatible.example")))
    pod_ip = call(context, "get", "pod", "-n", "edgion-system", "-l", "app=edgion-gateway",
                  "-o", "jsonpath={.items[0].status.podIP}").stdout.strip()
    if not pod_ip:
        raise RuntimeError("Gateway Pod has no IP for L4 probes")
    for kind, protocol, port in (("TCPRoute", "tcp", 31092),
                                 ("UDPRoute", "udp", 31101),
                                 ("TLSRoute", "tls", 8443)):
        if versions.get(kind):
            checks.append((protocol, pod_ip, port, kind))
    for protocol, host, port, label in checks:
        command = ["exec", "-n", "edgion-test", "compatible-probe", "--", "python", "-c",
                   PROBE_CODE, protocol, host, str(port)]
        if protocol in ("http", "http-fail"):
            command.append(label)
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            result = call(context, *command, check=False, timeout=45)
            if result.returncode == 0:
                break
            time.sleep(3)
        else:
            raise RuntimeError(f"Gateway forwarding failed for {protocol}/{label}: "
                               f"{result.stdout} {result.stderr}")
    if versions.get("GRPCRoute"):
        grpc_traffic(context)


def grpc_traffic(context):
    pod = call(context, "get", "pod", "-n", "edgion-system", "-l", "app=edgion-gateway",
               "-o", "jsonpath={.items[0].metadata.name}").stdout.strip()
    if not pod:
        raise RuntimeError("Gateway Pod is missing for the GRPCRoute probe")
    command = ["kubectl", "--context", context, "port-forward", "-n", "edgion-system",
               f"pod/{pod}", ":30021"]
    forward = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               text=True, bufsize=1)
    try:
        port = None
        deadline = time.monotonic() + 30
        with selectors.DefaultSelector() as selector:
            selector.register(forward.stdout, selectors.EVENT_READ)
            while time.monotonic() < deadline and port is None:
                if forward.poll() is not None:
                    raise RuntimeError(f"GRPCRoute port-forward exited with {forward.returncode}")
                if not selector.select(timeout=1):
                    continue
                line = forward.stdout.readline()
                match = re.search(r"Forwarding from 127\.0\.0\.1:(\d+) -> 30021", line)
                if match:
                    port = int(match.group(1))
        if port is None:
            raise RuntimeError("GRPCRoute port-forward did not become ready")
        probe = ["grpcurl", "-plaintext", "-connect-timeout", "5", "-max-time", "10",
                 "-import-path", str(HERE / "proto"), "-proto", "health.proto",
                 "-d", '{"service":""}', f"127.0.0.1:{port}", "grpc.health.v1.Health/Check"]
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            result = subprocess.run(probe, text=True, capture_output=True, timeout=15,
                                    check=False)
            if result.returncode == 0 and json.loads(result.stdout).get("status") == "SERVING":
                return
            time.sleep(3)
        raise RuntimeError(f"GRPCRoute health request failed: {result.stdout} {result.stderr}")
    finally:
        forward.terminate()
        try:
            forward.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            forward.kill()
            forward.communicate()


def cleanup(context, crds):
    for namespace in NAMESPACES:
        call(context, "delete", "namespace", namespace, "--ignore-not-found", "--wait=true",
             check=False, timeout=300)
    for path in (K8S / "kubernetes/controller/rbac.yaml",
                 K8S / "kubernetes/controller/rbac-leader-election.yaml"):
        call(context, "delete", "-f", path, "--ignore-not-found", check=False)
    for crd in crds:
        call(context, "delete", "crd", crd["metadata"]["name"], "--ignore-not-found",
             "--wait=true", check=False, timeout=300)
    for path in sorted((ROOT / "config/crd/edgion-crd").glob("*.yaml")):
        call(context, "delete", "-f", path, "--ignore-not-found", check=False)


def gateway_crds(context):
    items = json.loads(call(context, "get", "crd", "-o", "json").stdout)["items"]
    return [obj for obj in items if obj["spec"]["group"] == "gateway.networking.k8s.io"]


def wait_scaled_down(context, name):
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        pods = json.loads(call(context, "get", "pods", "-n", "edgion-system", "-l",
                               f"app={name}", "-o", "json").stdout)["items"]
        if not pods:
            return
        time.sleep(2)
    raise RuntimeError(f"deployment/{name} did not scale to zero")


def scale_system_down(context):
    for name in ("edgion-controller", "edgion-gateway"):
        call(context, "scale", f"deployment/{name}", "-n", "edgion-system", "--replicas=0")
    for name in ("edgion-controller", "edgion-gateway"):
        wait_scaled_down(context, name)


def delete_crds(context, crds):
    for crd in crds:
        call(context, "delete", "crd", crd["metadata"]["name"], "--ignore-not-found",
             "--wait=true", timeout=330)


def baseline_reuse(context, work):
    work.mkdir(parents=True, exist_ok=True)
    baseline = {"crds": gateway_crds(context), "deployments": {}}
    if not baseline["crds"]:
        raise RuntimeError("reuse mode requires existing Gateway API CRDs to restore")
    for crd in baseline["crds"]:
        result = call(context, "get", crd["metadata"]["name"], "-A", "-o", "json", check=False)
        if result.returncode != 0:
            raise RuntimeError(f"cannot inspect existing {crd['metadata']['name']}: {result.stderr}")
        if json.loads(result.stdout)["items"]:
            raise RuntimeError(f"existing {crd['metadata']['name']} objects prevent CRD replacement")
    for name in ("edgion-controller", "edgion-gateway"):
        obj = json.loads(call(context, "get", "deployment", name, "-n", "edgion-system", "-o", "json").stdout)
        container = next(item for item in obj["spec"]["template"]["spec"]["containers"]
                         if item["name"] == name)
        baseline["deployments"][name] = {"image": container["image"],
                                          "replicas": obj["spec"].get("replicas", 1)}
    config = json.loads(call(context, "get", "configmap", "edgion-controller-config",
                             "-n", "edgion-system", "-o", "json").stdout)
    baseline["controller_config"] = config["data"]["edgion-controller.yaml"]
    (work / "baseline.json").write_text(json.dumps({"deployments": baseline["deployments"],
        "gateway_crds": [obj["metadata"]["name"] for obj in baseline["crds"]]}, indent=2) + "\n")
    return baseline


def save_reuse_baseline(path, baseline, created_namespaces):
    state = {**baseline, "created_namespaces": created_namespaces}
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(state, output)
        output.flush()
        os.fsync(output.fileno())


def patch_controller_config(context, content):
    call(context, "patch", "configmap", "edgion-controller-config", "-n", "edgion-system",
         "--type=merge", "-p", json.dumps({"data": {"edgion-controller.yaml": content}}))


def prepare_reuse(context, baseline, images):
    scale_system_down(context)
    original = baseline["controller_config"]
    updated = "\n".join(line for line in original.split("\n")
                        if not line.strip().startswith("gateway_class:"))
    patch_controller_config(context, updated)
    for name in ("edgion-controller", "edgion-gateway"):
        call(context, "set", "image", f"deployment/{name}", "-n", "edgion-system",
             f"{name}={images[name]}")
    delete_crds(context, baseline["crds"])


def cleanup_reuse(context, crds, created_namespaces):
    scale_system_down(context)
    for namespace in created_namespaces:
        call(context, "delete", "namespace", namespace, "--ignore-not-found", "--wait=true",
             timeout=330)
    delete_crds(context, crds)
    if gateway_crds(context):
        raise RuntimeError("Gateway API CRDs remain after case cleanup")
    for namespace in created_namespaces:
        if call(context, "get", "namespace", namespace, "-o", "name", check=False).returncode == 0:
            raise RuntimeError(f"namespace/{namespace} remains after case cleanup")


def restore_reuse(context, baseline, created_namespaces):
    scale_system_down(context)
    for namespace in created_namespaces:
        call(context, "delete", "namespace", namespace, "--ignore-not-found", "--wait=true",
             timeout=330)
    delete_crds(context, gateway_crds(context))
    for original in baseline["crds"]:
        metadata = original["metadata"]
        apply(context, {"apiVersion": original["apiVersion"], "kind": original["kind"],
                        "metadata": {"name": metadata["name"],
                                     "annotations": metadata.get("annotations", {}),
                                     "labels": metadata.get("labels", {})},
                        "spec": original["spec"]})
    for original in baseline["crds"]:
        call(context, "wait", "--for=condition=Established", "--timeout=120s",
             "crd/" + original["metadata"]["name"])
    patch_controller_config(context, baseline["controller_config"])
    for name, deployment in baseline["deployments"].items():
        call(context, "set", "image", f"deployment/{name}", "-n", "edgion-system",
             f"{name}={deployment['image']}")
        call(context, "scale", f"deployment/{name}", "-n", "edgion-system",
             f"--replicas={deployment['replicas']}")
    for name, deployment in baseline["deployments"].items():
        if deployment["replicas"]:
            call(context, "rollout", "status", f"deployment/{name}", "-n", "edgion-system",
                 "--timeout=300s", timeout=330)
    restored = {obj["metadata"]["name"]: obj["spec"] for obj in gateway_crds(context)}
    if restored != {obj["metadata"]["name"]: obj["spec"] for obj in baseline["crds"]}:
        raise RuntimeError("restored Gateway API CRD specs differ from the baseline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", help="Dedicated, empty kubectl context")
    parser.add_argument("--version", help="Run one locked stable release (default: all)")
    parser.add_argument("--from-version", help="Resume at this locked stable release and run newer releases")
    parser.add_argument("--cache-dir", type=Path, default=Path("/tmp/edgion-gateway-api-bundles"))
    parser.add_argument("--report-dir", type=Path, default=ROOT / "tmp/compatible_testing")
    parser.add_argument("--controller-image", default=os.environ.get("CONTROLLER_IMAGE"))
    parser.add_argument("--gateway-image", default=os.environ.get("GATEWAY_IMAGE"))
    parser.add_argument("--test-server-image", default=os.environ.get("TEST_SERVER_IMAGE"))
    parser.add_argument("--reuse-cluster", action="store_true",
                        help="Temporarily replace CRDs and reuse an existing empty Edgion runtime")
    parser.add_argument("--restore-reuse-baseline", action="store_true",
                        help="Restore a reused cluster after an interrupted run")
    parser.add_argument("--check-artifacts", action="store_true",
                        help="Verify locked downloads and per-release source inventories without a cluster")
    args = parser.parse_args()
    cases = sorted(HERE.glob("v*/case.json"))
    locked = json.loads(ARTIFACTS.read_text())
    for case in cases:
        metadata = json.loads(case.read_text())
        if metadata.get("release") != case.parent.name or set(metadata.get("expectedSources", {})) != {"standard"}:
            parser.error(f"invalid release metadata: {case}")
        if f"{metadata['release']}/standard" not in locked:
            parser.error(f"release artifact is not locked: {metadata['release']}/standard")
    if args.version:
        cases = [case for case in cases if case.parent.name == args.version]
    if args.from_version:
        if args.from_version not in {case.parent.name for case in cases}:
            parser.error(f"unknown --from-version: {args.from_version}")
        cases = [case for case in cases if case.parent.name >= args.from_version]
    if args.version and args.from_version:
        parser.error("--version and --from-version are mutually exclusive")
    if not cases:
        parser.error("no matching release directory")
    args.cache_dir.mkdir(parents=True, exist_ok=True)
    if args.check_artifacts:
        try:
            from jsonschema import Draft202012Validator
        except ImportError as exc:
            parser.error(f"--check-artifacts requires jsonschema: {exc}")
        validated = 0
        for case in cases:
            metadata = json.loads(case.read_text())
            crds = fetch_locked(case.parent.name, "standard", args.cache_dir)
            versions = served_versions(crds)
            if versions != metadata["expectedSources"]["standard"]:
                raise RuntimeError(f"source inventory drift: {case.parent.name}/standard")
            beta = all(served(crds, kind, "v1beta1") for kind in
                       ("GatewayClass", "Gateway", "HTTPRoute"))
            objects = standard_suite(case.parent.name, crds, versions, beta)[0]
            for obj in objects:
                source = obj["apiVersion"].split("/")[-1]
                schema = next((item["schema"]["openAPIV3Schema"] for crd in crds
                               if crd["spec"]["names"]["kind"] == obj["kind"]
                               for item in crd["spec"]["versions"]
                               if item["name"] == source and item.get("served")), None)
                if schema is None:
                    raise RuntimeError(f"fixture source is unserved: {case.parent.name}/standard "
                                       f"{obj['kind']}/{source}")
                errors = list(Draft202012Validator(schema).iter_errors(obj))
                if errors:
                    raise RuntimeError(f"fixture schema error: {case.parent.name}/standard "
                                       f"{obj['kind']}/{source}: {errors[0].message}")
                validated += 1
            print(f"PASS artifact {case.parent.name}/standard")
        print(f"Validated {validated} resource fixtures across {len(cases)} Standard bundles")
        return 0
    if not args.context:
        parser.error("--context is required for a cluster run")
    if args.restore_reuse_baseline and args.reuse_cluster:
        parser.error("--restore-reuse-baseline and --reuse-cluster are mutually exclusive")
    if not args.restore_reuse_baseline and (not args.controller_image or not args.gateway_image):
        parser.error("--controller-image and --gateway-image are required for a cluster run")
    active = subprocess.check_output(["kubectl", "config", "current-context"], text=True).strip()
    if active != args.context:
        parser.error(f"active context {active!r} differs from --context {args.context!r}")
    call(args.context, "get", "nodes", "-o", "name")
    state_path = args.report_dir / "reuse-baseline-state.json"
    if args.restore_reuse_baseline:
        if not state_path.exists():
            parser.error(f"no interrupted reuse baseline to restore: {state_path}")
        state = json.loads(state_path.read_text())
        restore_reuse(args.context, state, state["created_namespaces"])
        state_path.unlink()
        print(f"Restored reused cluster from {state_path}")
        return 0
    if any(json.loads(case.read_text())["expectedSources"]["standard"].get("GRPCRoute") for case in cases):
        if shutil.which("grpcurl") is None:
            parser.error("grpcurl is required to probe GRPCRoute traffic; install it before the cluster run")
    if args.reuse_cluster and state_path.exists():
        parser.error(f"interrupted reuse baseline remains at {state_path}; "
                     "run --restore-reuse-baseline before another compatibility run")
    created_namespaces = []
    if args.reuse_cluster:
        for namespace in NAMESPACES:
            if call(args.context, "get", "namespace", namespace, "-o", "name", check=False).returncode != 0:
                created_namespaces.append(namespace)
        if "edgion-system" in created_namespaces:
            parser.error("--reuse-cluster requires an existing edgion-system namespace")
        for namespace in TEST_NAMESPACES:
            if namespace not in created_namespaces:
                parser.error(f"namespace/{namespace} already exists; fixture ownership is unclear")
    else:
        for namespace in NAMESPACES:
            if call(args.context, "get", "namespace", namespace, "-o", "name", check=False).returncode == 0:
                parser.error(f"namespace/{namespace} already exists; use an empty dedicated cluster")
        existing = json.loads(call(args.context, "get", "crd", "-o", "json").stdout)
        if any(item["spec"]["group"] in ("gateway.networking.k8s.io", "edgion.io")
               for item in existing["items"]):
            parser.error("Gateway API or Edgion CRDs already exist; use an empty dedicated cluster")
        for path in (K8S / "kubernetes/controller/rbac.yaml",
                     K8S / "kubernetes/controller/rbac-leader-election.yaml"):
            for obj in yaml.safe_load_all(path.read_text()):
                if obj and obj["kind"] in ("ClusterRole", "ClusterRoleBinding"):
                    if call(args.context, "get", obj["kind"].lower(), obj["metadata"]["name"],
                            "-o", "name", check=False).returncode == 0:
                        parser.error(f"{obj['kind']}/{obj['metadata']['name']} already exists; "
                                     "use an empty dedicated cluster")
    args.report_dir.mkdir(parents=True, exist_ok=True)
    images = {"edgion-controller": args.controller_image, "edgion-gateway": args.gateway_image,
              "test-server": args.test_server_image}
    baseline = baseline_reuse(args.context, args.report_dir) if args.reuse_cluster else None
    if baseline:
        save_reuse_baseline(state_path, baseline, created_namespaces)
        try:
            prepare_reuse(args.context, baseline, images)
            return run_cases(args, cases, images, created_namespaces)
        finally:
            restore_reuse(args.context, baseline, created_namespaces)
            state_path.unlink()
    return run_cases(args, cases, images, created_namespaces)


def run_cases(args, cases, images, created_namespaces):
    failures = []
    for case in cases:
        version = case.parent.name
        channel = "standard"
        label = f"{version}/{channel}"
        work = args.report_dir / f"{version}-{channel}"
        work.mkdir(parents=True, exist_ok=True)
        (work / "result.json").write_text(json.dumps({"result": "INCOMPLETE"}) + "\n")
        crds = []
        passed = False
        try:
            print(f"RUN {label}", flush=True)
            crds = fetch_locked(version, channel, args.cache_dir)
            versions = served_versions(crds)
            beta = all(served(crds, kind, "v1beta1") for kind in
                       ("GatewayClass", "Gateway", "HTTPRoute"))
            if versions != json.loads(case.read_text())["expectedSources"][channel]:
                raise RuntimeError(f"source inventory drift: {label}")
            for kind in ("GatewayClass", "Gateway", "HTTPRoute"):
                if not versions.get(kind):
                    raise RuntimeError(f"required {kind}/v1 absent from {label}")
            objects, no_watch_kinds = standard_suite(version, crds, versions, beta)
            deploy(args.context, crds, versions, beta, objects, no_watch_kinds,
                   work, images, args.reuse_cluster)
            traffic(args.context, bool(versions.get("ReferenceGrant")), beta, versions)
            passed = True
        except Exception as exc:
            failures.append(label)
            (work / "result.json").write_text(json.dumps({"result": "FAIL", "error": str(exc)}, indent=2) + "\n")
            print(f"FAIL {label}: {exc}", file=sys.stderr, flush=True)
            for selector, filename in (("app=edgion-controller", "controller.log"),
                                       ("app=edgion-gateway", "gateway.log")):
                result = call(args.context, "logs", "-n", "edgion-system", "-l", selector,
                              "--tail=300", check=False)
                (work / filename).write_text(result.stdout + result.stderr)
        finally:
            if args.reuse_cluster:
                cleanup_reuse(args.context, crds, created_namespaces)
            else:
                cleanup(args.context, crds)
                residual = json.loads(call(args.context, "get", "crd", "-o", "json").stdout)
                if any(item["spec"]["group"] in ("gateway.networking.k8s.io", "edgion.io")
                       for item in residual["items"]):
                    (work / "result.json").write_text(json.dumps({"result": "FAIL", "error": "CRD cleanup residue"}) + "\n")
                    raise RuntimeError(f"cleanup left CRDs after {label}; stop before switching versions")
                if any(call(args.context, "get", "namespace", ns, "-o", "name",
                            check=False).returncode == 0 for ns in NAMESPACES):
                    (work / "result.json").write_text(json.dumps({"result": "FAIL", "error": "namespace cleanup residue"}) + "\n")
                    raise RuntimeError(f"cleanup left namespaces after {label}; stop before switching versions")
        if passed:
            (work / "result.json").write_text(json.dumps({"result": "PASS", "sources": versions,
                                                            "betaCore": beta}, indent=2) + "\n")
            print(f"PASS {label}", flush=True)
    print(f"Compatible tests: {len(cases) - len(failures)} passed, "
          f"{len(failures)} failed; reports: {args.report_dir}")
    return bool(failures)


if __name__ == "__main__":
    sys.exit(main())
