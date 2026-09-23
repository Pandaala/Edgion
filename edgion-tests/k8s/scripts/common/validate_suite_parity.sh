#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

LOCAL_RUNNER="${PROJECT_ROOT}/edgion-tests/integration/scripts/integration/run_integration.sh"
K8S_RUNNER="${PROJECT_ROOT}/edgion-tests/k8s/scripts/run_k8s_integration.sh"
DYNAMIC_MANIFEST_SOURCE="${PROJECT_ROOT}/edgion-tests/src/client/test_client.rs"
K8S_DYNAMIC_MANIFEST="${PROJECT_ROOT}/edgion-tests/k8s/scripts/common/k8s_dynamic_scenarios.tsv"
SCOPE_EXCEPTIONS="${PROJECT_ROOT}/edgion-tests/k8s/scripts/common/k8s_suite_scope_exceptions.tsv"

usage() {
  cat <<'EOF'
Usage: validate_suite_parity.sh [options]

Compare the canonical integration inventory with the K8s runner inventory.

Options:
  --local-runner PATH             Canonical local integration runner
  --k8s-runner PATH               K8s integration runner
  --dynamic-manifest-source PATH  Rust source containing EXPECTED_DYNAMIC_SCENARIO_IDS
  --k8s-dynamic-manifest PATH     Manifest of helper-backed K8s dynamic scenarios
  --scope-exceptions PATH         Structured deliberate-scope exceptions
  -h, --help                      Show this help

K8s isolated scenarios and final gates may be declared with source-local markers:
  # suite-parity: isolated <scenario-id>
  # suite-parity: final-gate <gate-id-without-Final_>
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --local-runner)
      [ "$#" -ge 2 ] || { echo "ERROR: --local-runner requires a path" >&2; exit 2; }
      LOCAL_RUNNER="$2"
      shift 2
      ;;
    --k8s-runner)
      [ "$#" -ge 2 ] || { echo "ERROR: --k8s-runner requires a path" >&2; exit 2; }
      K8S_RUNNER="$2"
      shift 2
      ;;
    --dynamic-manifest-source)
      [ "$#" -ge 2 ] || { echo "ERROR: --dynamic-manifest-source requires a path" >&2; exit 2; }
      DYNAMIC_MANIFEST_SOURCE="$2"
      shift 2
      ;;
    --k8s-dynamic-manifest)
      [ "$#" -ge 2 ] || { echo "ERROR: --k8s-dynamic-manifest requires a path" >&2; exit 2; }
      K8S_DYNAMIC_MANIFEST="$2"
      shift 2
      ;;
    --scope-exceptions)
      [ "$#" -ge 2 ] || { echo "ERROR: --scope-exceptions requires a path" >&2; exit 2; }
      SCOPE_EXCEPTIONS="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

python3 - "$LOCAL_RUNNER" "$K8S_RUNNER" "$DYNAMIC_MANIFEST_SOURCE" "$K8S_DYNAMIC_MANIFEST" "$SCOPE_EXCEPTIONS" "$PROJECT_ROOT" <<'PY'
import re
import sys
from pathlib import Path


class InventoryError(Exception):
    pass


def read_source(path_text, label):
    path = Path(path_text)
    try:
        return path, path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        raise InventoryError(f"cannot read {label} {path}: {error}") from error


def between(text, start, end, label):
    start_index = text.find(start)
    if start_index < 0:
        raise InventoryError(f"{label}: missing start anchor {start!r}")
    end_index = text.find(end, start_index + len(start))
    if end_index < 0:
        raise InventoryError(f"{label}: missing end anchor {end!r}")
    return text[start_index + len(start):end_index]


def require_unique(values, label):
    seen = set()
    duplicates = set()
    for value in values:
        if value in seen:
            duplicates.add(value)
        seen.add(value)
    if duplicates:
        raise InventoryError(f"{label}: duplicate entries: {', '.join(sorted(duplicates))}")
    return seen


def parse_local_static(text):
    body = between(text, "run_all_tests() {", "\nmain() {", "local static inventory")
    pairs = re.findall(
        r'-r\s+([A-Za-z][A-Za-z0-9_]*)\s+-i\s+([A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*)',
        body,
    )
    if not pairs:
        raise InventoryError("local static inventory: no literal Resource/Item selections found")
    return {f"{resource}/{item}" for resource, item in pairs}


def parse_k8s_static(text):
    function = between(
        text,
        "run_all_selected_suites() {",
        "\n  local local_start_from=",
        "K8s static inventory",
    )
    array_body = between(function, "  local suites=(", "\n  )", "K8s static suites array")
    entries = re.findall(r'^\s*"([^"\n]+)"\s*$', array_body, flags=re.MULTILINE)
    nonblank_lines = [line.strip() for line in array_body.splitlines() if line.strip()]
    if len(entries) != len(nonblank_lines):
        malformed = [line for line in nonblank_lines if not re.fullmatch(r'"[^"\n]+"', line)]
        raise InventoryError(
            "K8s static suites array: malformed or non-literal entries: " + ", ".join(malformed)
        )
    if not entries:
        raise InventoryError("K8s static inventory: suites array is empty")
    suite_keys = []
    for entry in entries:
        fields = entry.split("|")
        if len(fields) != 3 or any(not field for field in fields):
            raise InventoryError(f"K8s static suites array: malformed suite triple: {entry}")
        _suite_dir, resource, item = fields
        suite_keys.append(f"{resource}/{item}")
    return require_unique(suite_keys, "K8s static inventory")


def parse_dynamic_manifest(text):
    match = re.search(
        r'const\s+EXPECTED_DYNAMIC_SCENARIO_IDS\s*:\s*&\[&str\]\s*=\s*&\[(.*?)\];',
        text,
        flags=re.DOTALL,
    )
    if not match:
        raise InventoryError("dynamic manifest: EXPECTED_DYNAMIC_SCENARIO_IDS anchor is missing")
    body = re.sub(r'//[^\n]*', '', match.group(1))
    ids = re.findall(r'"([A-Za-z][A-Za-z0-9_]*)"', body)
    residue = re.sub(r'"[A-Za-z][A-Za-z0-9_]*"', '', body)
    if residue.replace(',', '').strip():
        raise InventoryError("dynamic manifest: unsupported non-literal entry")
    if not ids:
        raise InventoryError("dynamic manifest: expected scenario list is empty")
    return require_unique(ids, "dynamic manifest")


def parse_k8s_dynamic(text):
    return set(re.findall(r'--dynamic-scenario\s+([A-Za-z][A-Za-z0-9_]*)\b', text))


def parse_k8s_dynamic_manifest(text):
    ids = []
    for line_number, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        fields = raw_line.rstrip("\r").split("\t")
        if len(fields) != 7:
            raise InventoryError(
                f"K8s dynamic manifest line {line_number}: expected seven tab-separated fields"
            )
        (
            scenario_id,
            conf_path,
            reset_path,
            kubectl_resource,
            gateway_kind,
            namespace,
            resource_name,
        ) = fields
        if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*", scenario_id):
            raise InventoryError(
                f"K8s dynamic manifest line {line_number}: invalid scenario id {scenario_id!r}"
            )
        for path_label, path_value in (("conf", conf_path), ("reset", reset_path)):
            path_parts = path_value.split("/")
            if (
                not path_value
                or path_value.startswith("/")
                or any(part in {"", ".", ".."} for part in path_parts)
            ):
                raise InventoryError(
                    f"K8s dynamic manifest line {line_number}: "
                    f"invalid relative {path_label} path {path_value!r}"
                )
        identity_values = (kubectl_resource, gateway_kind, resource_name)
        if (
            any(not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:-]*", value) for value in identity_values)
            or not re.fullmatch(r"-|[A-Za-z0-9][A-Za-z0-9._:-]*", namespace)
        ):
            raise InventoryError(
                f"K8s dynamic manifest line {line_number}: invalid resource identity"
            )
        ids.append(scenario_id)
    if not ids:
        raise InventoryError("K8s dynamic manifest contains no scenarios")
    return require_unique(ids, "K8s dynamic manifest")


def parse_local_isolated(text):
    function = between(
        text,
        "run_isolated_scenarios() {",
        "\n# =============================================================================\n# Cleanup function",
        "local isolated inventory",
    )
    initial = between(function, "    local names=(", "\n    )", "local isolated names array")
    names = re.findall(r'^\s*"([A-Za-z][A-Za-z0-9_]*)"\s*$', initial, flags=re.MULTILINE)
    initial_lines = [line for line in initial.splitlines() if line.strip()]
    if len(names) != len(initial_lines):
        raise InventoryError("local isolated names array: unsupported non-literal entry")
    for addition in re.findall(r'\bnames\+=\((.*?)\)', function, flags=re.DOTALL):
        addition_names = re.findall(r'"([A-Za-z][A-Za-z0-9_]*)"', addition)
        residue = re.sub(r'"[A-Za-z][A-Za-z0-9_]*"', '', addition)
        if not addition_names or residue.strip():
            raise InventoryError("local isolated names addition: unsupported non-literal entry")
        names.extend(addition_names)
    if not names:
        raise InventoryError("local isolated inventory: no scenarios found")
    return require_unique(names, "local isolated inventory")


def marker_ids(text, kind):
    pattern = rf'^\s*#\s*suite-parity:\s*{re.escape(kind)}\s+([A-Za-z][A-Za-z0-9_.-]*)\s*$'
    return re.findall(pattern, text, flags=re.MULTILINE)


def parse_local_final(text):
    gates = set(re.findall(r'report_test\s+"Final_([A-Za-z][A-Za-z0-9_]*)"', text))
    if not gates:
        raise InventoryError("local final-gate inventory: no Final_ report entries found")
    return gates


def parse_scope_exceptions(text, project_root, canonical, covered):
    allowed_reasons = {
        "local_certificate_fixture": {
            "path": "edgion-tests/src/client/dynamic/scenarios/encrypted_active_health_checks.rs",
            "symbol": "LocalCertificates",
            "tokens": ("edgion-tests/certs", "expired-client.crt", "30043"),
        },
        "external_processor_fixture": {
            "path": "edgion-tests/src/client/dynamic/scenarios/ext_proc_tls_policy_recovery.rs",
            "symbol": "EDGION_EXT_PROC_EXTERNAL_REQUIRED",
            "tokens": ("EDGION_EXT_PROC_EXTERNAL_ADMIN_URL", "accepted_connections", "observe_cases"),
        },
        "k8s_status_subresource_fixture": {
            "path": "edgion-tests/src/client/dynamic/scenarios/acme_gatewayref_ownership.rs",
            "symbol": "AcmeGatewayRefOwnership",
            "tokens": ("updates/expire", "selected-a", "selected-b"),
        },
        "host_log_topology": {
            "path": "edgion-tests/src/client/suites/l4_duplex.rs",
            "symbol": "LogProbe",
            "tokens": ("access_log_path", "host"),
        },
        "nested_docker_fixture": {
            "path": "edgion-tests/integration/scripts/integration/ldap_egress/scenario.py",
            "symbol": "Scenario",
            "tokens": ("docker", "OpenLDAP"),
        },
        "loopback_only_external_fixture": {
            "path": "edgion-tests/integration/fixtures/ext_proc_external/main.go",
            "symbol": "validateListenAddress",
            "tokens": ('host != "127.0.0.1"', "must use literal 127.0.0.1"),
        },
        "status_subresource_fixture": {
            "path": "config/crd/edgion-crd/edgion_acme_crd.yaml",
            "symbol": "activeChallenges",
            "tokens": ("/status subresource", "subresources:", "status: {}"),
        },
        "single_selected_class_l4": {
            "path": "edgion-tests/k8s/conf/TCPRoute/L4TimeoutHotReload/DynamicTest/initial/02_Gateway_l4-timeout-gw.yaml",
            "symbol": "l4-timeout-class",
            "tokens": ("gatewayClassName: l4-timeout-class",),
        },
        "single_selected_class_plugin_policy": {
            "path": "edgion-tests/src/client/dynamic/scenarios/plugin_policy.rs",
            "symbol": "PluginPolicyDynamic",
            "tokens": ("GatewayClass_pp-a.yaml", "pp-b"),
        },
        "single_selected_class_preflight": {
            "path": "edgion-tests/k8s/conf/Gateway/PreflightPolicyHotReload/DynamicTest/initial/resources.yaml",
            "symbol": "preflight-policy-hot-reload",
            "tokens": ("gatewayClassName: preflight-policy-hot-reload",),
        },
    }
    result = {name: set() for name in canonical}
    seen = set()
    for line_number, raw_line in enumerate(text.splitlines(), start=1):
        if not raw_line.strip() or raw_line.lstrip().startswith("#"):
            continue
        fields = raw_line.rstrip("\r").split("\t")
        if len(fields) != 7:
            raise InventoryError(
                f"scope exceptions line {line_number}: expected seven tab-separated fields"
            )
        category, item_id, reason, evidence_path, symbol, rationale, trigger = fields
        if category not in {"static", "dynamic", "isolated"}:
            raise InventoryError(f"scope exceptions line {line_number}: invalid category {category!r}")
        id_pattern = r"[A-Za-z][A-Za-z0-9_]*(?:/[A-Za-z0-9_.-]+)*" if category == "static" else r"[A-Za-z][A-Za-z0-9_]*"
        if not re.fullmatch(id_pattern, item_id) or any(token in item_id for token in "*?[](){}|\\"):
            raise InventoryError(f"scope exceptions line {line_number}: invalid literal id {item_id!r}")
        key = (category, item_id)
        if key in seen:
            raise InventoryError(f"scope exceptions line {line_number}: duplicate exception {category}/{item_id}")
        seen.add(key)
        if item_id not in canonical[category]:
            raise InventoryError(f"scope exceptions line {line_number}: noncanonical or stale id {category}/{item_id}")
        if item_id in covered[category]:
            raise InventoryError(f"scope exceptions line {line_number}: {category}/{item_id} already has K8s coverage")
        if reason not in allowed_reasons:
            raise InventoryError(f"scope exceptions line {line_number}: unknown reason {reason!r}")
        reason_contract = allowed_reasons[reason]
        path = Path(evidence_path)
        if path.is_absolute() or not evidence_path or any(part in {"", ".", ".."} for part in path.parts):
            raise InventoryError(f"scope exceptions line {line_number}: invalid evidence path {evidence_path!r}")
        evidence = (project_root / path).resolve()
        try:
            evidence.relative_to(project_root.resolve())
        except ValueError as error:
            raise InventoryError(f"scope exceptions line {line_number}: evidence escapes repository") from error
        if not evidence.is_file():
            raise InventoryError(f"scope exceptions line {line_number}: evidence file is missing: {evidence_path}")
        if evidence_path != reason_contract["path"]:
            raise InventoryError(
                f"scope exceptions line {line_number}: evidence path is outside the approved scope for {reason}"
            )
        if not symbol or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_:.-]*", symbol):
            raise InventoryError(f"scope exceptions line {line_number}: invalid evidence symbol")
        if symbol != reason_contract["symbol"]:
            raise InventoryError(
                f"scope exceptions line {line_number}: evidence symbol is outside the approved scope for {reason}"
            )
        evidence_text = evidence.read_text(encoding="utf-8")
        if symbol not in evidence_text or any(token not in evidence_text for token in reason_contract["tokens"]):
            raise InventoryError(f"scope exceptions line {line_number}: evidence no longer supports {reason}")
        if reason.startswith("single_selected_class_"):
            controller_config = project_root / "edgion-tests/k8s/kubernetes/controller/configmap.yaml"
            if "gateway_class: public-gateway" not in controller_config.read_text(encoding="utf-8"):
                raise InventoryError("single selected GatewayClass exception no longer matches the Kubernetes controller configuration")
        if reason == "single_selected_class_plugin_policy":
            fixture_root = project_root / "edgion-tests/k8s/conf/PluginPolicy/DynamicTest/initial"
            for class_name in ("pp-a", "pp-b"):
                class_fixture = (fixture_root / f"GatewayClass_{class_name}.yaml").read_text(encoding="utf-8")
                gateway_fixture = (fixture_root / f"Gateway_{class_name}.yaml").read_text(encoding="utf-8")
                if not re.search(rf"(?m)^metadata:\s*\n\s+name:\s*{class_name}\s*$", class_fixture) \
                    or not re.search(rf"(?m)^\s+gatewayClassName:\s*{class_name}\s*$", gateway_fixture):
                    raise InventoryError(f"PluginPolicyDynamic exception no longer uses private GatewayClass {class_name}")
        placeholders = ("todo", "tbd", "n/a", "later", "same as above")
        if (
            len(rationale.split()) < 8
            or any(marker in rationale.lower() for marker in placeholders)
            or re.search(r"\b(all|every|any)\b.*\b(test|suite|scenario)s?\b", rationale, re.IGNORECASE)
        ):
            raise InventoryError(
                f"scope exceptions line {line_number}: rationale is blank, placeholder, or overbroad"
            )
        if (
            len(trigger.split()) < 8
            or any(marker in trigger.lower() for marker in placeholders)
            or not trigger.startswith(("Re-evaluate when ", "Re-evaluate after "))
        ):
            raise InventoryError(
                f"scope exceptions line {line_number}: re-evaluation trigger is invalid or placeholder"
            )
        result[category].add(item_id)
    return result


def print_diff(label, canonical, k8s):
    missing = sorted(canonical - k8s)
    extra = sorted(k8s - canonical)
    print(f"{label}: canonical={len(canonical)} k8s={len(k8s)} missing={len(missing)} k8s_only={len(extra)}")
    for value in missing:
        print(f"  MISSING {label} {value}")
    for value in extra:
        print(f"  K8S_ONLY {label} {value}")
    return bool(missing)


def main():
    local_path, local_text = read_source(sys.argv[1], "local runner")
    k8s_path, k8s_text = read_source(sys.argv[2], "K8s runner")
    dynamic_path, dynamic_text = read_source(sys.argv[3], "dynamic manifest source")
    k8s_dynamic_path, k8s_dynamic_text = read_source(sys.argv[4], "K8s dynamic manifest")
    exception_path, exception_text = read_source(sys.argv[5], "scope exceptions")
    project_root = Path(sys.argv[6])

    if "--list-dynamic-scenarios" not in local_text or "run_dynamic_scenarios" not in local_text:
        raise InventoryError("local dynamic inventory: runner no longer enumerates the dynamic registry")

    local_static = parse_local_static(local_text)
    k8s_static = parse_k8s_static(k8s_text)
    local_dynamic = parse_dynamic_manifest(dynamic_text)
    if k8s_dynamic_path.name not in k8s_text or "run_k8s_dynamic_scenarios.sh" not in k8s_text:
        raise InventoryError("K8s runner does not reference the manifest-backed dynamic helper")
    if k8s_text.count("run_manifest_dynamic_scenarios") < 2 \
            or "if ! run_manifest_dynamic_scenarios; then" not in k8s_text:
        raise InventoryError("K8s runner does not invoke the manifest-backed dynamic phase fail-closed")
    k8s_dynamic = parse_k8s_dynamic(k8s_text)
    k8s_dynamic.update(parse_k8s_dynamic_manifest(k8s_dynamic_text))
    local_isolated = parse_local_isolated(local_text)
    k8s_isolated = set(marker_ids(k8s_text, "isolated"))
    k8s_isolated.update(re.findall(r'report_test\s+"Isolated_([A-Za-z][A-Za-z0-9_]*)"', k8s_text))
    local_final = parse_local_final(local_text)
    k8s_final = set(marker_ids(k8s_text, "final-gate"))
    k8s_final.update(re.findall(r'report_test\s+"Final_([A-Za-z][A-Za-z0-9_]*)"', k8s_text))
    canonical = {"static": local_static, "dynamic": local_dynamic, "isolated": local_isolated}
    covered = {"static": k8s_static, "dynamic": k8s_dynamic, "isolated": k8s_isolated}
    exceptions = parse_scope_exceptions(exception_text, project_root, canonical, covered)

    print("Suite parity inventory sources:")
    print(f"  local runner: {local_path}")
    print(f"  K8s runner: {k8s_path}")
    print(f"  dynamic manifest: {dynamic_path}::EXPECTED_DYNAMIC_SCENARIO_IDS")
    print(f"  K8s dynamic manifest: {k8s_dynamic_path}")
    print(f"  deliberate scope exceptions: {exception_path}")
    print("K8s gate declaration sources:")
    print("  isolated: literal Isolated_ report_test entries or '# suite-parity: isolated <id>'")
    print("  final: literal Final_ report_test entries or '# suite-parity: final-gate <id>'")

    has_missing = False
    for category, values in exceptions.items():
        for value in sorted(values):
            print(f"  EXCEPTED {category} {value}")
    has_missing |= print_diff("static", local_static - exceptions["static"], k8s_static)
    has_missing |= print_diff("dynamic", local_dynamic - exceptions["dynamic"], k8s_dynamic)
    has_missing |= print_diff("isolated", local_isolated - exceptions["isolated"], k8s_isolated)
    has_missing |= print_diff("final-gate", local_final, k8s_final)
    if has_missing:
        print("FAIL: K8s integration inventory is missing canonical coverage.")
        return 1
    print("PASS: K8s integration inventory covers every canonical entry.")
    return 0


try:
    sys.exit(main())
except InventoryError as error:
    print(f"ERROR: suite parity inventory could not be extracted: {error}", file=sys.stderr)
    sys.exit(2)
PY
