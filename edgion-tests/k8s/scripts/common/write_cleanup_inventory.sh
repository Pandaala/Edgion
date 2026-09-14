#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -eq 0 ]]; then
  echo "usage: $0 <fixture-root>..." >&2
  exit 1
fi

python3 - "$@" <<'PY'
import pathlib
import sys

import yaml

cluster_kinds = {"EdgionGatewayConfig", "GatewayClass"}
installation_namespaces = {"dragonfly-system", "edgion-system"}
keys = set()

for raw_root in sys.argv[1:]:
    root = pathlib.Path(raw_root)
    if not root.exists():
        continue
    for path in sorted(set(root.rglob("*.yaml")) | set(root.rglob("*.yml"))):
        if path.name.endswith(".template.yaml"):
            continue
        try:
            documents = list(yaml.safe_load_all(path.read_text()))
        except yaml.YAMLError as error:
            raise SystemExit(f"invalid ownership-inventory YAML {path}: {error}") from error
        for document in documents:
            if not isinstance(document, dict):
                continue
            api_version = document.get("apiVersion")
            kind = document.get("kind")
            metadata = document.get("metadata") or {}
            name = metadata.get("name")
            namespace = metadata.get("namespace")
            group = (
                api_version.split("/", 1)[0]
                if isinstance(api_version, str) and "/" in api_version
                else ""
            )
            locator = f"{kind}.{group}" if group else kind
            if namespace and namespace not in installation_namespaces:
                keys.add(("namespace", "Namespace", "Namespace", namespace))
            if kind == "Namespace" and name and name not in installation_namespaces:
                keys.add(("namespace", "Namespace", "Namespace", name))
            elif kind in cluster_kinds and name:
                keys.add(("cluster", kind, locator, name))

for key in sorted(keys):
    print("\t".join(key))
PY
