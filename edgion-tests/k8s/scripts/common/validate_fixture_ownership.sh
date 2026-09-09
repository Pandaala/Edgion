#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
CONF_ROOT="${PROJECT_ROOT}/edgion-tests/k8s/conf"
INCLUDE_BOOTSTRAP=false
INCLUDE_DYNAMIC_UPDATES=false
FAIL_ON_IDENTICAL=false

show_help() {
  cat <<EOF
Usage: $0 [options] [conf-root]

Validate that each effective initial Kubernetes object has one fixture owner.
Object identity is (apiVersion, kind, namespace, metadata.name). Conflicting
duplicate definitions fail. Identical normalized definitions are reported and
allowed by default.

Options:
  --include-bootstrap       Include 00-namespace.yaml and 01-deployment.yaml
  --include-dynamic-updates Include updates* and delete* directories under any DynamicTest
  --fail-on-identical       Fail when duplicate-identical definitions are found
  -h, --help                Show this help
EOF
}

POSITIONAL_COUNT=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --include-bootstrap)
      INCLUDE_BOOTSTRAP=true
      shift
      ;;
    --include-dynamic-updates)
      INCLUDE_DYNAMIC_UPDATES=true
      shift
      ;;
    --fail-on-identical)
      FAIL_ON_IDENTICAL=true
      shift
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    --*)
      echo "[ERROR] unknown option: $1" >&2
      show_help >&2
      exit 2
      ;;
    *)
      POSITIONAL_COUNT=$((POSITIONAL_COUNT + 1))
      if [[ "${POSITIONAL_COUNT}" -gt 1 ]]; then
        echo "[ERROR] expected at most one conf-root argument" >&2
        show_help >&2
        exit 2
      fi
      CONF_ROOT="$1"
      shift
      ;;
  esac
done

if [[ ! -d "${CONF_ROOT}" ]]; then
  echo "[ERROR] k8s conf root not found: ${CONF_ROOT}" >&2
  exit 1
fi

if ! python3 -c 'import yaml' >/dev/null 2>&1; then
  echo "[ERROR] Python package PyYAML is required to validate fixture ownership" >&2
  exit 1
fi

echo "[INFO] validating effective initial fixture ownership in: ${CONF_ROOT}"

python3 - "${CONF_ROOT}" "${INCLUDE_BOOTSTRAP}" "${INCLUDE_DYNAMIC_UPDATES}" "${FAIL_ON_IDENTICAL}" "${PROJECT_ROOT}/config/crd/edgion-crd" <<'PYEOF'
import hashlib
import os
import re
import sys
from collections import defaultdict

import yaml
from yaml.constructor import ConstructorError
from yaml.nodes import ScalarNode


conf_root = os.path.abspath(sys.argv[1])
include_bootstrap = sys.argv[2] == "true"
include_dynamic_updates = sys.argv[3] == "true"
fail_on_identical = sys.argv[4] == "true"
crd_root = os.path.abspath(sys.argv[5])


class UniqueKeyLoader(yaml.SafeLoader):
    """Reject duplicate literal mapping keys instead of silently keeping the last value."""


def construct_unique_mapping(loader, node, deep=False):
    seen_keys = set()
    for key_node, _ in node.value:
        if not isinstance(key_node, ScalarNode):
            continue
        key = (key_node.tag, key_node.value)
        if key in seen_keys:
            raise ConstructorError(
                "while constructing a mapping",
                node.start_mark,
                f"found duplicate mapping key {key_node.value!r}",
                key_node.start_mark,
            )
        seen_keys.add(key)
    return yaml.SafeLoader.construct_mapping(loader, node, deep=deep)


UniqueKeyLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG,
    construct_unique_mapping,
)

# Keep these exclusions aligned with apply_all_conf_strict.sh. They describe
# files that are not part of the effective initial fixture set.
runtime_secret_patterns = (
    re.compile(r"/base/Secret_edgion-test_edge-tls\.ya?ml$"),
    re.compile(r"/EdgionTls/mTLS/Secret_edge_client-ca\.ya?ml$"),
    re.compile(r"/EdgionTls/mTLS/Secret_edge_ca-chain\.ya?ml$"),
    re.compile(r"/EdgionTls/mTLS/Secret_edge_mtls-server\.ya?ml$"),
    re.compile(r"/HTTPRoute/Backend/BackendTLS/Secret_backend-ca\.ya?ml$"),
    re.compile(r"/EdgionPlugins/HeaderCertAuth/01_Secret_default_header-cert-ca\.ya?ml$"),
)
invalid_admission_pattern = re.compile(r"/Gateway/PortConflict/Gateway_internal_conflict\.ya?ml$")
dynamic_updates_pattern = re.compile(r"/DynamicTest/([^/]+/)*(updates|delete)[^/]*/")


def is_excluded(relative_path):
    basename = os.path.basename(relative_path)
    if not include_bootstrap and basename in {"00-namespace.yaml", "00-namespace.yml", "01-deployment.yaml", "01-deployment.yml"}:
        return True
    match_path = "/" + relative_path
    if not include_dynamic_updates and dynamic_updates_pattern.search(match_path):
        return True
    if any(pattern.search(match_path) for pattern in runtime_secret_patterns):
        return True
    if invalid_admission_pattern.search(match_path):
        return True
    return False


def resource_label(key):
    api_version, kind, namespace, name = key
    namespace_display = namespace or "<unspecified>"
    return f"apiVersion={api_version} kind={kind} namespace={namespace_display} name={name}"


yaml_files = []
for directory, _, filenames in os.walk(conf_root):
    for filename in filenames:
        if filename.endswith((".yaml", ".yml")):
            yaml_files.append(os.path.join(directory, filename))
yaml_files.sort()

served_edgion_versions = {}
for filename in sorted(os.listdir(crd_root)):
    if not filename.endswith((".yaml", ".yml")):
        continue
    with open(os.path.join(crd_root, filename), encoding="utf-8") as stream:
        crd = yaml.safe_load(stream)
    if not isinstance(crd, dict) or crd.get("kind") != "CustomResourceDefinition":
        continue
    spec = crd.get("spec", {})
    if spec.get("group") != "edgion.io":
        continue
    kind = spec.get("names", {}).get("kind")
    versions = {
        f"edgion.io/{version.get('name')}"
        for version in spec.get("versions", [])
        if version.get("served") is True and version.get("name")
    }
    if kind and versions:
        served_edgion_versions[kind] = versions

owners = defaultdict(list)
excluded_count = 0
parse_errors = []

for path in yaml_files:
    relative_path = os.path.relpath(path, conf_root).replace(os.sep, "/")
    excluded = is_excluded(relative_path)
    if excluded:
        excluded_count += 1

    try:
        with open(path, encoding="utf-8") as stream:
            documents = list(yaml.load_all(stream, Loader=UniqueKeyLoader))
    except (OSError, yaml.YAMLError) as error:
        parse_errors.append(f"{relative_path}: {error}")
        continue

    for document_index, document in enumerate(documents, start=1):
        if document is None:
            continue
        if not isinstance(document, dict):
            parse_errors.append(
                f"{relative_path}#doc={document_index}: expected a YAML mapping, got {type(document).__name__}"
            )
            continue

        metadata = document.get("metadata")
        api_version = document.get("apiVersion")
        kind = document.get("kind")
        if not isinstance(metadata, dict) or not api_version or not kind or not metadata.get("name"):
            parse_errors.append(
                f"{relative_path}#doc={document_index}: resource must define apiVersion, kind, metadata, and metadata.name"
            )
            continue

        if str(api_version).startswith("edgion.io/"):
            served_versions = served_edgion_versions.get(str(kind))
            if served_versions is None or str(api_version) not in served_versions:
                expected = ", ".join(sorted(served_versions or [])) or "a known Edgion CRD kind"
                parse_errors.append(
                    f"{relative_path}#doc={document_index}: apiVersion {api_version} is not served for "
                    f"{kind}; expected {expected}"
                )
                continue

        if excluded:
            continue

        namespace = metadata.get("namespace") or ""
        name = metadata["name"]
        key = (str(api_version), str(kind), str(namespace), str(name))
        normalized = yaml.safe_dump(document, sort_keys=True, default_flow_style=False)
        digest = hashlib.sha256(normalized.encode("utf-8")).hexdigest()
        owners[key].append((relative_path, document_index, digest))

if parse_errors:
    print(f"[ERROR] failed to parse {len(parse_errors)} effective fixture document(s):", file=sys.stderr)
    for error in parse_errors:
        print(f"  - {error}", file=sys.stderr)
    sys.exit(1)

identical = []
conflicting = []
for key in sorted(owners):
    occurrences = owners[key]
    if len(occurrences) < 2:
        continue
    if len({occurrence[2] for occurrence in occurrences}) == 1:
        identical.append((key, occurrences))
    else:
        conflicting.append((key, occurrences))

for key, occurrences in identical:
    print(f"[duplicate-identical] {resource_label(key)}")
    for relative_path, document_index, _ in occurrences:
        print(f"  - {relative_path}#doc={document_index}")

for key, occurrences in conflicting:
    print(f"[duplicate-conflicting] {resource_label(key)}", file=sys.stderr)
    for relative_path, document_index, digest in occurrences:
        print(f"  - {relative_path}#doc={document_index} content={digest[:12]}", file=sys.stderr)

print(
    f"[INFO] scanned {len(yaml_files) - excluded_count} effective YAML file(s); excluded {excluded_count} file(s)"
)

if conflicting:
    print(
        f"[ERROR] conflicting fixture ownership detected for {len(conflicting)} object key(s)",
        file=sys.stderr,
    )
    sys.exit(1)

if identical and fail_on_identical:
    print(
        f"[ERROR] duplicate-identical fixture ownership detected for {len(identical)} object key(s)"
        " and --fail-on-identical is set",
        file=sys.stderr,
    )
    sys.exit(1)

if identical:
    print(f"[INFO] validation passed with {len(identical)} duplicate-identical object key(s) allowed")
else:
    print("[INFO] validation passed: every effective initial object has one fixture owner")
PYEOF
