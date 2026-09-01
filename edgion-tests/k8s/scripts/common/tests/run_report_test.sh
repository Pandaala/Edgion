#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNNER="$(cd "${SCRIPT_DIR}/../.." && pwd)/run_k8s_integration.sh"
TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT INT TERM

mkdir -p "${TMP_DIR}/bin"
cat > "${TMP_DIR}/bin/kubectl" <<'EOF_KUBECTL'
#!/usr/bin/env bash
exit 42
EOF_KUBECTL
chmod +x "${TMP_DIR}/bin/kubectl"

K8S_REPORT_ROOT="${TMP_DIR}/help-reports" "${RUNNER}" --help >/dev/null
if [[ -d "${TMP_DIR}/help-reports" ]] \
  && find "${TMP_DIR}/help-reports" -mindepth 1 -print -quit | grep -q .; then
  echo "--help unexpectedly created a run report" >&2
  exit 1
fi

set +e
PATH="${TMP_DIR}/bin:${PATH}" \
  CLUSTER_MODE=local \
  LOCAL_IMAGE_MODE=local-existing \
  K8S_REPORT_ROOT="${TMP_DIR}/failed-reports" \
  "${RUNNER}" --skip-prepare -r HTTPRoute -i Basic > "${TMP_DIR}/failed.log" 2>&1
runner_rc=$?
set -e

if [[ "${runner_rc}" -eq 0 ]]; then
  echo "runner unexpectedly succeeded with an unavailable cluster" >&2
  exit 1
fi

status_file="$(find "${TMP_DIR}/failed-reports" -name status -type f -print -quit)"
if [[ -z "${status_file}" ]]; then
  echo "failed run did not create a durable status file" >&2
  exit 1
fi
if [[ "$(cat "${status_file}")" != "INCOMPLETE" ]]; then
  echo "unexpected failed-run status: $(cat "${status_file}")" >&2
  exit 1
fi

run_dir="$(dirname "${status_file}")"
for artifact in metadata run.log pods.txt events.txt endpointslices.yaml route-status.yaml workloads.txt; do
  if [[ ! -f "${run_dir}/${artifact}" ]]; then
    echo "missing failed-run artifact: ${artifact}" >&2
    exit 1
  fi
done

echo "run_report_test.sh: PASS"
