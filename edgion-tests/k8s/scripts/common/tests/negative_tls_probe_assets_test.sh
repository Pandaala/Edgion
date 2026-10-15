#!/usr/bin/env bash
# Check generated negative-TLS pins against the actual test-client pod mounts.
# This is an isolated certificate/artifact regression, not a Kubernetes wire run.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
K8S_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TEST_ROOT="$(mktemp -d /tmp/edgion-negative-tls-assets.XXXXXX)"
trap 'rm -rf "${TEST_ROOT}"' EXIT

if ! bash "${SCRIPT_DIR}/../generate_runtime_certs.sh" "${TEST_ROOT}/generated" \
  >"${TEST_ROOT}/generation.log" 2>&1; then
  echo "Negative TLS asset fixture generation failed" >&2
  exit 1
fi

python3 - "${TEST_ROOT}/generated/secrets" \
  "${K8S_ROOT}/kubernetes/test/test-client/deployment.yaml" <<'PY'
import base64
import pathlib
import sys

import yaml


class AssetError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise AssetError(message)


def documents(path):
    try:
        return list(yaml.safe_load_all(path.read_text()))
    except Exception:
        # YAML parser exceptions can contain literal Secret bodies.
        raise AssetError("Cannot parse an asset fixture manifest") from None


def decoded_secret(resource, key):
    try:
        return base64.b64decode(resource["data"][key], validate=True)
    except Exception:
        raise AssetError("Cannot decode a required serving-certificate fixture") from None


def verify_pins(mounted, resources):
    pins = {
        "ca.pem": "edge-tls",
        "mtls/mtls-server.crt": "mtls-server",
        "mtls/fv-server.crt": "fv-server-cert",
    }
    for relative, secret in pins.items():
        target = str(pathlib.PurePosixPath("/usr/local/edgion/edgion-tests/certs") / relative)
        require(target in mounted, "A required negative TLS pin is absent from the pod mounts")
        source = resources.get(("Secret", "edgion-test", secret))
        require(source is not None, "A required serving Secret fixture is absent")
        require(mounted[target] == decoded_secret(source, "tls.crt"),
                "A mounted negative TLS pin differs from its serving Secret")


def main():
    resources = {}
    for path in pathlib.Path(sys.argv[1]).glob("*.yaml"):
        for resource in documents(path):
            require(isinstance(resource, dict), "A generated asset manifest is not an object")
            metadata = resource.get("metadata", {})
            key = (resource.get("kind"), metadata.get("namespace"), metadata.get("name"))
            require(key not in resources, "Generated asset fixture identity is duplicated")
            resources[key] = resource

    deployments = [value for value in documents(pathlib.Path(sys.argv[2]))
                   if isinstance(value, dict) and value.get("kind") == "Deployment"
                   and value.get("metadata", {}).get("name") == "edgion-test-client"]
    require(len(deployments) == 1, "Expected the actual test-client Deployment")
    pod = deployments[0]["spec"]["template"]["spec"]
    containers = [value for value in pod["containers"] if value.get("name") == "test-client"]
    require(len(containers) == 1, "Expected the actual test-client container")
    container = containers[0]
    require(any(value.get("name") == "EDGION_TEST_K8S_MODE" and str(value.get("value")).lower() == "true"
                for value in container.get("env", [])), "Test-client must select Kubernetes asset mode")
    volumes = {value["name"]: value for value in pod["volumes"]}
    mounted = {}
    for mount in container["volumeMounts"]:
        volume = volumes[mount["name"]]
        source = volume.get("secret") or volume.get("configMap")
        if source is None:
            continue
        secret = "secret" in volume
        name = source["secretName" if secret else "name"]
        resource = resources.get(("Secret" if secret else "ConfigMap", "edgion-test", name))
        if resource is None:
            # The unrelated ports ConfigMap is not a certificate artifact.
            continue
        require("subPath" not in mount, "Certificate projection unexpectedly uses subPath")
        data = resource.get("data", {})
        items = source.get("items", [{"key": key, "path": key} for key in data])
        for item in items:
            relative = pathlib.PurePosixPath(item["path"])
            require(not relative.is_absolute() and ".." not in relative.parts,
                    "Certificate projection path is invalid")
            target = str(pathlib.PurePosixPath(mount["mountPath"]) / relative)
            require(item["key"] in data, "A projected certificate key is missing")
            mounted[target] = decoded_secret(resource, item["key"]) if secret else data[item["key"]].encode()

    mtls = resources.get(("ConfigMap", "edgion-test", "edgion-test-client-mtls-certs"))
    require(mtls is not None, "The mtls client ConfigMap is missing")
    require("fv-server.key" not in mtls.get("data", {}),
            "The public frontend pin must not add its private key to the client ConfigMap")
    verify_pins(mounted, resources)
    missing = dict(mounted)
    missing.pop("/usr/local/edgion/edgion-tests/certs/mtls/fv-server.crt")
    try:
        verify_pins(missing, resources)
    except AssetError:
        pass
    else:
        raise AssetError("The asset regression accepted an absent frontend pin")


try:
    main()
except AssetError as error:
    print(str(error), file=sys.stderr)
    sys.exit(1)
except Exception:
    # Avoid exposing generated material through arbitrary parser/value errors.
    print("Negative TLS asset mapping validation failed", file=sys.stderr)
    sys.exit(1)
print("Negative TLS generated assets match the test-client projections and serving Secrets")
PY
