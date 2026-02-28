#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONF_ROOT="${CONF_ROOT:-$PROJECT_ROOT/examples/k8stest/conf}"

show_help() {
  cat <<EOF
Usage: $0 [options] [conf-root]

Apply all YAML resources under k8s conf root.
Uses batch (directory-level) apply for speed, with per-file fallback on failure.
Automatically deduplicates resources that appear in multiple suite directories.

Options:
  --include-bootstrap   Include 00-namespace.yaml and 01-deployment.yaml
  --include-dynamic-updates Include Gateway/DynamicTest/updates and delete resources
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

if ! command -v kubectl >/dev/null 2>&1; then
  echo "kubectl not found in PATH"
  exit 1
fi

if [[ ! -d "${CONF_ROOT}" ]]; then
  echo "k8s conf root not found: ${CONF_ROOT}"
  exit 1
fi

APPLY_ARGS=(apply --server-side --force-conflicts --field-manager=edgion-k8s-test)
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
    patterns+=('/Gateway/DynamicTest/(updates|delete)/')
  fi

  patterns+=('/base/Secret_edgion-test_edge-tls\.ya?ml$')
  patterns+=('/EdgionTls/mTLS/Secret_edge_client-ca\.ya?ml$')
  patterns+=('/EdgionTls/mTLS/Secret_edge_ca-chain\.ya?ml$')
  patterns+=('/EdgionTls/mTLS/Secret_edge_mtls-server\.ya?ml$')
  patterns+=('/HTTPRoute/Backend/BackendTLS/Secret_backend-ca\.ya?ml$')
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

# Deduplicate YAML files within a directory tree: when multiple files define the
# same K8s resource (kind/namespace/name), keep only the first one (sorted order).
# Copies unique files into dest_dir preserving relative paths.
dedup_and_copy() {
  local src_dir="$1"
  local dest_dir="$2"

  python3 - "${src_dir}" "${dest_dir}" "${EXCLUDE_PATTERN}" <<'PYEOF'
import yaml, os, sys, re, shutil

src_dir = sys.argv[1]
dest_dir = sys.argv[2]
exclude_re = re.compile(sys.argv[3])

seen = {}  # resource_key -> first file path
copied = 0
deduped = 0
excluded = 0

yaml_files = []
for dirpath, _, filenames in os.walk(src_dir):
    for fn in sorted(filenames):
        if fn.endswith(('.yaml', '.yml')):
            yaml_files.append(os.path.join(dirpath, fn))
yaml_files.sort()

for fpath in yaml_files:
    if exclude_re.search(fpath):
        excluded += 1
        continue

    resource_keys = []
    try:
        with open(fpath) as f:
            for doc in yaml.safe_load_all(f):
                if not doc or 'kind' not in doc or 'metadata' not in doc:
                    continue
                kind = doc['kind']
                ns = doc['metadata'].get('namespace', '_cluster_')
                name = doc['metadata'].get('name', '_unknown_')
                resource_keys.append(f'{kind}/{ns}/{name}')
    except Exception:
        resource_keys = []

    is_dup = False
    for key in resource_keys:
        if key in seen:
            is_dup = True
            break

    if is_dup:
        deduped += 1
        continue

    for key in resource_keys:
        seen[key] = fpath

    rel = os.path.relpath(fpath, src_dir)
    dest = os.path.join(dest_dir, rel)
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    shutil.copy2(fpath, dest)
    copied += 1

print(f"{copied}:{deduped}:{excluded}")
PYEOF
}

apply_single_file_with_retry() {
  local f="$1"

  local ok=false saw_conflict=false
  for attempt in 1 2 3; do
    out="$(kubectl "${APPLY_ARGS[@]}" -f "${f}" 2>&1)" && rc=0 || rc=$?
    if [[ "${rc}" -eq 0 ]]; then
      echo "  ${out}"
      ok=true
      break
    fi

    if echo "${out}" | grep -qE 'Operation cannot be fulfilled|the object has been modified'; then
      saw_conflict=true
      echo "  Conflict (attempt ${attempt}/3), retrying: $(basename "${f}")"
      sleep 1
      continue
    fi

    echo "  ${out}"
    return "${rc}"
  done

  if [[ "${ok}" != "true" && "${saw_conflict}" == "true" ]]; then
    echo "  Conflict persists, fallback to replace --force: $(basename "${f}")"
    if kubectl replace --force -f "${f}"; then
      ok=true
    fi
  fi

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
total_deduped=0
total_excluded=0

# Apply root-level YAML files (not in subdirectories).
ROOT_BATCH="${WORK_DIR}/_root"
mkdir -p "${ROOT_BATCH}"
root_has_files=false
while IFS= read -r f; do
  [[ -n "${f}" ]] || continue
  if ! is_excluded "${f}"; then
    cp "${f}" "${ROOT_BATCH}/"
    root_has_files=true
  fi
done < <(find "${CONF_ROOT}" -maxdepth 1 -type f \( -name "*.yaml" -o -name "*.yml" \) | sort)

if [[ "${root_has_files}" == "true" ]]; then
  apply_batch_dir "${ROOT_BATCH}" "conf(root)"
fi

# Apply each top-level subdirectory as a batch, with resource-level dedup.
while IFS= read -r sub_dir; do
  [[ -d "${sub_dir}" ]] || continue
  dir_name="$(basename "${sub_dir}")"
  batch_dir="${WORK_DIR}/${dir_name}"
  mkdir -p "${batch_dir}"

  stats="$(dedup_and_copy "${sub_dir}" "${batch_dir}")"
  file_count="${stats%%:*}"
  rest="${stats#*:}"
  dedup_count="${rest%%:*}"
  excl_count="${rest##*:}"

  total_applied=$((total_applied + file_count))
  total_deduped=$((total_deduped + dedup_count))
  total_excluded=$((total_excluded + excl_count))

  if [[ "${dedup_count}" -gt 0 ]]; then
    echo "(deduped ${dedup_count} duplicate resources in ${dir_name}/)"
  fi

  if [[ "${file_count}" -eq 0 ]]; then
    continue
  fi

  if ! apply_batch_dir "${batch_dir}" "${dir_name}"; then
    exit 1
  fi
done < <(find "${CONF_ROOT}" -mindepth 1 -maxdepth 1 -type d | sort)

echo "Batch apply finished. Applied=${total_applied} Deduped=${total_deduped} Excluded=${total_excluded}"
