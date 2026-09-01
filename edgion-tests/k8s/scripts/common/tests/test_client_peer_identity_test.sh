#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../test_client_peer_identity.sh"
TEST_ROOT="$(mktemp -d /tmp/edgion-test-client-peer-test.XXXXXX)"
FAKE_BIN="${TEST_ROOT}/bin"
STORE_DIR="${TEST_ROOT}/store"

cleanup() {
  rm -rf "${TEST_ROOT}"
}
trap cleanup EXIT INT TERM

mkdir -p "${FAKE_BIN}" "${STORE_DIR}"
cat > "${FAKE_BIN}/kubectl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
  get)
    if [ "${2:-}" = "pods" ]; then
      case "${FAKE_POD_CASE:-ipv4}" in
        ipv4)
          echo '{"items":[{"metadata":{"uid":"uid-v4"},"status":{"phase":"Running","podIP":"10.20.30.40","conditions":[{"type":"Ready","status":"True"}]}}]}'
          ;;
        ipv6)
          echo '{"items":[{"metadata":{"uid":"uid-v6"},"status":{"phase":"Running","podIP":"2001:db8::20","conditions":[{"type":"Ready","status":"True"}]}}]}'
          ;;
        stale)
          echo '{"items":[{"metadata":{"uid":"uid-new"},"status":{"phase":"Running","podIP":"10.20.30.41","conditions":[{"type":"Ready","status":"True"}]}}]}'
          ;;
        zero)
          echo '{"items":[]}'
          ;;
        multiple)
          echo '{"items":[{"metadata":{"uid":"uid-a"},"status":{"phase":"Running","podIP":"10.0.0.1","conditions":[{"type":"Ready","status":"True"}]}},{"metadata":{"uid":"uid-b"},"status":{"phase":"Running","podIP":"10.0.0.2","conditions":[{"type":"Ready","status":"True"}]}}]}'
          ;;
        invalid)
          echo '{"items":[{"metadata":{"uid":"uid-bad"},"status":{"phase":"Running","podIP":"not-an-ip","conditions":[{"type":"Ready","status":"True"}]}}]}'
          ;;
        terminating)
          echo '{"items":[{"metadata":{"uid":"uid-old","deletionTimestamp":"2026-01-01T00:00:00Z"},"status":{"phase":"Running","podIP":"10.0.0.8","conditions":[{"type":"Ready","status":"True"}]}},{"metadata":{"uid":"uid-live"},"status":{"phase":"Running","podIP":"10.0.0.9","conditions":[{"type":"Ready","status":"True"}]}}]}'
          ;;
      esac
      exit 0
    fi
    if [ "${2:-}" = "edgionconfigdata" ]; then
      namespace=""
      while [ "$#" -gt 0 ]; do
        if [ "$1" = "--namespace" ]; then
          namespace="$2"
          break
        fi
        shift
      done
      python3 - "${FAKE_STORE_DIR}/${namespace}.yaml" <<'PY'
import json
import sys
import yaml

with open(sys.argv[1], encoding="utf-8") as stream:
    print(json.dumps(yaml.safe_load(stream)))
PY
      exit 0
    fi
    ;;
  apply)
    input="$(mktemp)"
    trap 'rm -f "${input}"' EXIT
    sed -n '1,$p' > "${input}"
    namespace="$(python3 - "${input}" <<'PY'
import sys
import yaml

with open(sys.argv[1], encoding="utf-8") as stream:
    print(yaml.safe_load(stream)["metadata"]["namespace"])
PY
)"
    cp "${input}" "${FAKE_STORE_DIR}/${namespace}.yaml"
    echo "edgionconfigdata.edgion.io/k8s-test-client-peer serverside-applied"
    exit 0
    ;;
esac

echo "unexpected kubectl command: $*" >&2
exit 9
EOF
chmod 755 "${FAKE_BIN}/kubectl"

export PATH="${FAKE_BIN}:${PATH}"
export FAKE_STORE_DIR="${STORE_DIR}"

export FAKE_POD_CASE=ipv4
current="$(${HELPER} current)"
[ "${current}" = $'uid-v4\t10.20.30.40' ]
"${HELPER}" publish >/dev/null
for namespace in edgion-test edgion-default; do
  grep -q 'edgion.io/test-client-pod-uid: "uid-v4"' "${STORE_DIR}/${namespace}.yaml"
  grep -q 'edgion.io/test-client-pod-ip: "10.20.30.40"' "${STORE_DIR}/${namespace}.yaml"
  grep -q '"10.20.30.40/32"' "${STORE_DIR}/${namespace}.yaml"
done
"${HELPER}" assert >/dev/null

export FAKE_POD_CASE=stale
if "${HELPER}" assert >/dev/null 2>&1; then
  echo "stale identity unexpectedly passed" >&2
  exit 1
fi

rm -f "${STORE_DIR}"/*.yaml
export FAKE_POD_CASE=ipv6
"${HELPER}" publish >/dev/null
grep -q '"2001:db8::20/128"' "${STORE_DIR}/edgion-test.yaml"

for pod_case in zero multiple invalid; do
  rm -f "${STORE_DIR}"/*.yaml
  export FAKE_POD_CASE="${pod_case}"
  if "${HELPER}" publish >/dev/null 2>&1; then
    echo "${pod_case} Pod inventory unexpectedly published an identity" >&2
    exit 1
  fi
  if find "${STORE_DIR}" -type f | grep -q .; then
    echo "${pod_case} Pod inventory mutated the identity store" >&2
    exit 1
  fi
done

export FAKE_POD_CASE=terminating
current="$(${HELPER} current)"
[ "${current}" = $'uid-live\t10.0.0.9' ]

echo "test_client_peer_identity_test.sh: PASS"
