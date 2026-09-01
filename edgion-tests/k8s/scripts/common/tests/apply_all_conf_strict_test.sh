#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APPLIER="${SCRIPT_DIR}/../apply_all_conf_strict.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-apply-all-conf-test.XXXXXX)"
FAKE_BIN="${TEMP_DIR}/bin"
KUBECTL_LOG="${TEMP_DIR}/kubectl.log"

cleanup() {
  rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT INT TERM

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local expected="$2"
  if ! grep -Fq -- "${expected}" "${file}"; then
    echo "ERROR: expected output to contain: ${expected}" >&2
    cat "${file}" >&2
    exit 1
  fi
}

assert_not_contains() {
  local file="$1"
  local unexpected="$2"
  if grep -Fq -- "${unexpected}" "${file}"; then
    echo "ERROR: expected output not to contain: ${unexpected}" >&2
    cat "${file}" >&2
    exit 1
  fi
}

assert_no_kubectl_mutation() {
  if [[ -s "${KUBECTL_LOG}" ]]; then
    echo "ERROR: kubectl was called before fixture ownership validation failed" >&2
    cat "${KUBECTL_LOG}" >&2
    exit 1
  fi
}

write_config_map() {
  local path="$1"
  local name="$2"
  local value="$3"
  mkdir -p "$(dirname "${path}")"
  cat > "${path}" <<EOF
apiVersion: v1
kind: ConfigMap
metadata:
  name: ${name}
  namespace: test
data:
  value: ${value}
EOF
}

mkdir -p "${FAKE_BIN}"
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

target=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -f)
      target="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

printf 'CALL\n' >> "${KUBECTL_LOG}"
if [[ -d "${target}" ]]; then
  while IFS= read -r fixture; do
    [[ -n "${fixture}" ]] || continue
    resource_name="$(awk '$1 == "name:" { print $2; exit }' "${fixture}")"
    printf 'RESOURCE:%s\n' "${resource_name}" >> "${KUBECTL_LOG}"
  done < <(find "${target}" -type f \( -name '*.yaml' -o -name '*.yml' \) | sort)
elif [[ -f "${target}" ]]; then
  resource_name="$(awk '$1 == "name:" { print $2; exit }' "${target}")"
  printf 'RESOURCE:%s\n' "${resource_name}" >> "${KUBECTL_LOG}"
fi

echo "fake kubectl apply succeeded"
EOF
chmod +x "${FAKE_BIN}/kubectl"

CONFLICTING_ROOT="${TEMP_DIR}/conflicting"
write_config_map "${CONFLICTING_ROOT}/a.yaml" "shared-conflict" "first"
write_config_map "${CONFLICTING_ROOT}/nested/b.yaml" "shared-conflict" "second"
: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" KUBECTL_LOG="${KUBECTL_LOG}" \
  "${APPLIER}" "${CONFLICTING_ROOT}" > "${TEMP_DIR}/conflicting.log" 2>&1; then
  fail "conflicting duplicate fixtures were accepted"
fi
assert_contains "${TEMP_DIR}/conflicting.log" "[duplicate-conflicting]"
assert_contains "${TEMP_DIR}/conflicting.log" "no resources were applied"
assert_no_kubectl_mutation

IDENTICAL_ROOT="${TEMP_DIR}/identical"
write_config_map "${IDENTICAL_ROOT}/a.yaml" "shared-identical" "same"
mkdir -p "${IDENTICAL_ROOT}/nested"
cp "${IDENTICAL_ROOT}/a.yaml" "${IDENTICAL_ROOT}/nested/b.yaml"
: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" KUBECTL_LOG="${KUBECTL_LOG}" \
  "${APPLIER}" "${IDENTICAL_ROOT}" > "${TEMP_DIR}/identical.log" 2>&1; then
  fail "identical duplicate fixtures were accepted"
fi
assert_contains "${TEMP_DIR}/identical.log" "[duplicate-identical]"
assert_contains "${TEMP_DIR}/identical.log" "--fail-on-identical is set"
assert_no_kubectl_mutation

DYNAMIC_OPTION_ROOT="${TEMP_DIR}/dynamic-option"
write_config_map "${DYNAMIC_OPTION_ROOT}/Feature/DynamicTest/initial/resource.yaml" "dynamic-shared" "initial"
write_config_map "${DYNAMIC_OPTION_ROOT}/Feature/DynamicTest/updates_grouped/resource.yaml" "dynamic-shared" "update"
: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" KUBECTL_LOG="${KUBECTL_LOG}" \
  "${APPLIER}" --include-dynamic-updates "${DYNAMIC_OPTION_ROOT}" \
  > "${TEMP_DIR}/dynamic-option.log" 2>&1; then
  fail "--include-dynamic-updates was not passed to fixture ownership validation"
fi
assert_contains "${TEMP_DIR}/dynamic-option.log" "[duplicate-conflicting]"
assert_no_kubectl_mutation

BOOTSTRAP_OPTION_ROOT="${TEMP_DIR}/bootstrap-option"
write_config_map "${BOOTSTRAP_OPTION_ROOT}/active/resource.yaml" "bootstrap-shared" "active"
write_config_map "${BOOTSTRAP_OPTION_ROOT}/00-namespace.yaml" "bootstrap-shared" "bootstrap"
: > "${KUBECTL_LOG}"
if PATH="${FAKE_BIN}:${PATH}" KUBECTL_LOG="${KUBECTL_LOG}" \
  "${APPLIER}" --include-bootstrap "${BOOTSTRAP_OPTION_ROOT}" \
  > "${TEMP_DIR}/bootstrap-option.log" 2>&1; then
  fail "--include-bootstrap was not passed to fixture ownership validation"
fi
assert_contains "${TEMP_DIR}/bootstrap-option.log" "[duplicate-conflicting]"
assert_no_kubectl_mutation

UNIQUE_ROOT="${TEMP_DIR}/unique"
write_config_map "${UNIQUE_ROOT}/root.yaml" "root-unique" "root"
write_config_map "${UNIQUE_ROOT}/Suite/nested.yaml" "nested-unique" "nested"
write_config_map "${UNIQUE_ROOT}/EdgionPlugins/BasicAuth/DynamicTest/initial_grouped/initial.yaml" "dynamic-initial" "initial"
write_config_map "${UNIQUE_ROOT}/EdgionPlugins/BasicAuth/DynamicTest/updates_grouped/update.yaml" "dynamic-update" "update"
write_config_map "${UNIQUE_ROOT}/EdgionPlugins/RateLimit/DynamicTest/staged/delete_grouped/delete.yaml" "dynamic-delete" "delete"
write_config_map "${UNIQUE_ROOT}/base/Secret_edgion-test_edge-tls.yaml" "runtime-secret" "runtime"
write_config_map "${UNIQUE_ROOT}/00-namespace.yaml" "bootstrap-resource" "bootstrap"

: > "${KUBECTL_LOG}"
PATH="${FAKE_BIN}:${PATH}" KUBECTL_LOG="${KUBECTL_LOG}" \
  "${APPLIER}" "${UNIQUE_ROOT}" > "${TEMP_DIR}/unique.log" 2>&1 \
  || fail "unique effective fixtures were not applied"
assert_contains "${TEMP_DIR}/unique.log" "Batch apply finished. Applied=3 Excluded=4"
assert_contains "${KUBECTL_LOG}" "RESOURCE:root-unique"
assert_contains "${KUBECTL_LOG}" "RESOURCE:nested-unique"
assert_contains "${KUBECTL_LOG}" "RESOURCE:dynamic-initial"
assert_not_contains "${KUBECTL_LOG}" "RESOURCE:dynamic-update"
assert_not_contains "${KUBECTL_LOG}" "RESOURCE:dynamic-delete"
assert_not_contains "${KUBECTL_LOG}" "RESOURCE:runtime-secret"
assert_not_contains "${KUBECTL_LOG}" "RESOURCE:bootstrap-resource"

: > "${KUBECTL_LOG}"
PATH="${FAKE_BIN}:${PATH}" KUBECTL_LOG="${KUBECTL_LOG}" \
  "${APPLIER}" --include-bootstrap --include-dynamic-updates "${UNIQUE_ROOT}" \
  > "${TEMP_DIR}/unique-included.log" 2>&1 \
  || fail "explicitly included bootstrap and dynamic fixtures were not applied"
assert_contains "${TEMP_DIR}/unique-included.log" "Batch apply finished. Applied=6 Excluded=1"
assert_contains "${KUBECTL_LOG}" "RESOURCE:bootstrap-resource"
assert_contains "${KUBECTL_LOG}" "RESOURCE:dynamic-update"
assert_contains "${KUBECTL_LOG}" "RESOURCE:dynamic-delete"
assert_not_contains "${KUBECTL_LOG}" "RESOURCE:runtime-secret"

echo "OK: strict fixture apply regression tests passed"
