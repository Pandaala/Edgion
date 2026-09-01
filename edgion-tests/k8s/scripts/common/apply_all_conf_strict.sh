#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/edgion-tests/k8s/conf}"

show_help() {
  cat <<EOF
Usage: $0 [options] [conf-root]

Apply all YAML resources under k8s conf root.
Uses batch (directory-level) apply for speed, with per-file fallback on failure.
Fixture ownership is validated before any kubectl mutation.

Options:
  --include-bootstrap   Include 00-namespace.yaml and 01-deployment.yaml
  --include-dynamic-updates Include updates* and delete* directories under any DynamicTest
  -h, --help            Show this help
EOF
}

INCLUDE_BOOTSTRAP=false
INCLUDE_DYNAMIC_UPDATES=false
ARGS=()
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
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      ARGS+=("$1")
      shift
      ;;
  esac
done

if [[ ${#ARGS[@]} -gt 0 ]]; then
  CONF_ROOT="${ARGS[0]}"
fi

if [[ ! -d "${CONF_ROOT}" ]]; then
  echo "k8s conf root not found: ${CONF_ROOT}"
  exit 1
fi

OWNERSHIP_VALIDATOR="${SCRIPT_DIR}/validate_fixture_ownership.sh"
if [[ ! -x "${OWNERSHIP_VALIDATOR}" ]]; then
  echo "fixture ownership validator is not executable: ${OWNERSHIP_VALIDATOR}"
  exit 1
fi

OWNERSHIP_ARGS=(--fail-on-identical)
if [[ "${INCLUDE_BOOTSTRAP}" == "true" ]]; then
  OWNERSHIP_ARGS+=(--include-bootstrap)
fi
if [[ "${INCLUDE_DYNAMIC_UPDATES}" == "true" ]]; then
  OWNERSHIP_ARGS+=(--include-dynamic-updates)
fi

if ! "${OWNERSHIP_VALIDATOR}" "${OWNERSHIP_ARGS[@]}" "${CONF_ROOT}"; then
  echo "fixture ownership validation failed; no resources were applied"
  exit 1
fi

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH"
  exit 1
fi

APPLY_ARGS=(--request-timeout=15s apply --server-side --force-conflicts --field-manager=edgion-k8s-test)
WORK_DIR="$(mktemp -d /tmp/edgion-k8s-apply-batch.XXXXXX)"
cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

build_exclude_patterns() {
  local patterns=()

  if [[ "${INCLUDE_BOOTSTRAP}" != "true" ]]; then
    patterns+=('00-namespace\.ya?ml$' '01-deployment\.ya?ml$')
  fi

  if [[ "${INCLUDE_DYNAMIC_UPDATES}" != "true" ]]; then
    patterns+=('/DynamicTest/([^/]+/)*(updates|delete)[^/]*/')
  fi

  patterns+=('/base/Secret_edgion-test_edge-tls\.ya?ml$')
  patterns+=('/EdgionTls/mTLS/Secret_edge_client-ca\.ya?ml$')
  patterns+=('/EdgionTls/mTLS/Secret_edge_ca-chain\.ya?ml$')
  patterns+=('/EdgionTls/mTLS/Secret_edge_mtls-server\.ya?ml$')
  patterns+=('/HTTPRoute/Backend/BackendTLS/Secret_backend-ca\.ya?ml$')
  patterns+=('/EdgionPlugins/HeaderCertAuth/01_Secret_default_header-cert-ca\.ya?ml$')
  patterns+=('/Gateway/PortConflict/Gateway_internal_conflict\.ya?ml$')

  local combined
  combined="$(IFS='|'; echo "${patterns[*]}")"
  echo "${combined}"
}

EXCLUDE_PATTERN="$(build_exclude_patterns)"

is_excluded() {
  local f="$1"
  echo "${f}" | grep -Eq "${EXCLUDE_PATTERN}"
}

# Copy every effective YAML file while preserving its relative path. Ownership
# validation has already guaranteed that resource-key selection is unnecessary.
copy_effective_files() {
  local src_dir="$1"
  local dest_dir="$2"
  local f rel dest

  COPY_FILE_COUNT=0
  COPY_EXCLUDED_COUNT=0
  while IFS= read -r f; do
    [[ -n "${f}" ]] || continue
    if is_excluded "${f}"; then
      COPY_EXCLUDED_COUNT=$((COPY_EXCLUDED_COUNT + 1))
      continue
    fi

    rel="${f#"${src_dir}/"}"
    dest="${dest_dir}/${rel}"
    mkdir -p "$(dirname "${dest}")"
    cp -p "${f}" "${dest}"
    COPY_FILE_COUNT=$((COPY_FILE_COUNT + 1))
  done < <(find "${src_dir}" -type f \( -name "*.yaml" -o -name "*.yml" \) | sort)
}

apply_single_file_with_retry() {
  local f="$1"
  local out rc retry_delay
  local ok=false
  for attempt in 1 2 3; do
    out="$(kubectl "${APPLY_ARGS[@]}" -f "${f}" 2>&1)" && rc=0 || rc=$?
    if [[ "${rc}" -eq 0 ]]; then
      echo "  ${out}"
      ok=true
      break
    fi

    if echo "${out}" | grep -qE 'Operation cannot be fulfilled|the object has been modified'; then
      if [[ "${attempt}" -lt 3 ]]; then
        if [[ "${attempt}" -eq 1 ]]; then
          retry_delay=1
        else
          retry_delay=2
        fi
        echo "  Conflict (attempt ${attempt}/3), retrying in ${retry_delay}s: $(basename "${f}")"
        sleep "${retry_delay}"
        continue
      fi
      echo "  ${out}"
      break
    fi

    echo "  ${out}"
    return "${rc}"
  done

  if [[ "${ok}" != "true" ]]; then
    echo "  Failed to apply $(basename "${f}") after retries"
    return 1
  fi
}

apply_batch_dir() {
  local batch_dir="$1"
  local dir_name="$2"

  local yaml_count
  yaml_count="$(find "${batch_dir}" -type f \( -name "*.yaml" -o -name "*.yml" \) | wc -l | tr -d ' ')"
  if [[ "${yaml_count}" -eq 0 ]]; then
    return 0
  fi

  echo "Applying ${dir_name}/ (${yaml_count} resources) ..."

  out="$(kubectl "${APPLY_ARGS[@]}" -R -f "${batch_dir}" 2>&1)" && rc=0 || rc=$?
  if [[ "${rc}" -eq 0 ]]; then
    echo "${out}" | head -n 50
    local total_lines
    total_lines="$(echo "${out}" | wc -l | tr -d ' ')"
    if [[ "${total_lines}" -gt 50 ]]; then
      echo "  ... (${total_lines} total lines, truncated)"
    fi
    return 0
  fi

  if echo "${out}" | grep -qE 'Operation cannot be fulfilled|the object has been modified'; then
    echo "Batch apply had conflicts, falling back to per-file apply for ${dir_name}/ ..."
  else
    echo "Batch apply failed for ${dir_name}/, falling back to per-file apply ..."
    echo "${out}" | tail -n 5
  fi

  while IFS= read -r f; do
    [[ -n "${f}" ]] || continue
    echo "  Applying $(basename "${f}")"
    if ! apply_single_file_with_retry "${f}"; then
      return 1
    fi
  done < <(find "${batch_dir}" -type f \( -name "*.yaml" -o -name "*.yml" \) | sort)
}

# --- Main ---

total_applied=0
total_excluded=0

# Apply root-level YAML files (not in subdirectories).
ROOT_BATCH="${WORK_DIR}/_root"
mkdir -p "${ROOT_BATCH}"
root_has_files=false
while IFS= read -r f; do
  [[ -n "${f}" ]] || continue
  if ! is_excluded "${f}"; then
    cp -p "${f}" "${ROOT_BATCH}/"
    root_has_files=true
    total_applied=$((total_applied + 1))
  else
    total_excluded=$((total_excluded + 1))
  fi
done < <(find "${CONF_ROOT}" -maxdepth 1 -type f \( -name "*.yaml" -o -name "*.yml" \) | sort)

if [[ "${root_has_files}" == "true" ]]; then
  apply_batch_dir "${ROOT_BATCH}" "conf(root)"
fi

# Apply each top-level subdirectory as a batch.
while IFS= read -r sub_dir; do
  [[ -d "${sub_dir}" ]] || continue
  dir_name="$(basename "${sub_dir}")"
  batch_dir="${WORK_DIR}/${dir_name}"
  mkdir -p "${batch_dir}"

  copy_effective_files "${sub_dir}" "${batch_dir}"
  file_count="${COPY_FILE_COUNT}"
  total_applied=$((total_applied + COPY_FILE_COUNT))
  total_excluded=$((total_excluded + COPY_EXCLUDED_COUNT))

  if [[ "${file_count}" -eq 0 ]]; then
    continue
  fi

  if ! apply_batch_dir "${batch_dir}" "${dir_name}"; then
    exit 1
  fi
done < <(find "${CONF_ROOT}" -mindepth 1 -maxdepth 1 -type d | sort)

echo "Batch apply finished. Applied=${total_applied} Excluded=${total_excluded}"
