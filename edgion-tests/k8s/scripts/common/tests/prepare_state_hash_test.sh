#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MAIN_RUNNER="${SCRIPT_DIR}/../../run_k8s_integration.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-k8s-prepare-hash-test.XXXXXX)"
FUNCTION_FILE="${TEMP_DIR}/prepare-hash-functions.sh"
IMAGE_FUNCTION_FILE="${TEMP_DIR}/image-config-functions.sh"

cleanup() {
  rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT INT TERM

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

awk '
  /^hash_file\(\) \{/ { emit = 1 }
  /^write_prepare_state_marker\(\) \{/ { emit = 0 }
  emit { print }
' "${MAIN_RUNNER}" > "${FUNCTION_FILE}"
grep -Fq 'compute_prepare_conf_hash()' "${FUNCTION_FILE}" \
  || fail "could not extract prepare hash functions"

awk '
  /^load_image_config_file\(\) \{/ { emit = 1 }
  /^prepare_local_images_if_needed\(\) \{/ { emit = 0 }
  emit { print }
' "${MAIN_RUNNER}" > "${IMAGE_FUNCTION_FILE}"
grep -Fq 'load_image_config_file()' "${IMAGE_FUNCTION_FILE}" \
  || fail "could not extract image config loader"

CONF_ROOT="${TEMP_DIR}/conf"
K8S_DEPLOY_ROOT="${TEMP_DIR}/deploy"
LOCAL_TEST_BIN_DIR="${TEMP_DIR}/test-bin"
LOCAL_CORE_BIN_DIR=""
mkdir -p "${CONF_ROOT}" "${K8S_DEPLOY_ROOT}" "${LOCAL_TEST_BIN_DIR}"

PORTS_CONFIG="${TEMP_DIR}/ports.json"
IMAGE_CONFIG_FILE="${TEMP_DIR}/images.env"
DEPLOY_SCRIPT="${TEMP_DIR}/deploy.sh"
APPLY_ALL_SCRIPT="${TEMP_DIR}/apply.sh"
GENERATE_CERTS_SCRIPT="${TEMP_DIR}/generate-certs.sh"
TEST_CLIENT_PEER_IDENTITY="${TEMP_DIR}/test-client-peer-identity.sh"
SCRIPT_DIR="${TEMP_DIR}/scripts"
mkdir -p "${SCRIPT_DIR}"

printf 'kind: ConfigMap\n' > "${CONF_ROOT}/resource.yaml"
printf 'kind: Deployment\n' > "${K8S_DEPLOY_ROOT}/deployment.yaml"
printf '{}\n' > "${PORTS_CONFIG}"
printf 'TEST_CLIENT_IMAGE=test-client:v1\n' > "${IMAGE_CONFIG_FILE}"
printf '#!/usr/bin/env bash\n' > "${DEPLOY_SCRIPT}"
printf '#!/usr/bin/env bash\n' > "${APPLY_ALL_SCRIPT}"
printf '#!/usr/bin/env bash\n' > "${GENERATE_CERTS_SCRIPT}"
printf '#!/usr/bin/env bash\n' > "${TEST_CLIENT_PEER_IDENTITY}"
printf '#!/usr/bin/env bash\n' > "${SCRIPT_DIR}/cluster-patch.sh"
printf 'test-client-v1\n' > "${LOCAL_TEST_BIN_DIR}/test_client"
printf 'test-server-v1\n' > "${LOCAL_TEST_BIN_DIR}/test_server"

TEST_SERVER_REPLICAS=3
BACKEND_TEST_NAMESPACE=edgion-backend
LOCAL_IMAGE_MODE_RESOLVED=remote-pull
PULL_POLICY_OVERRIDE=Always
CONTROLLER_IMAGE=controller:v1
GATEWAY_IMAGE=gateway:v1
TEST_CLIENT_IMAGE=test-client:v1
TEST_SERVER_IMAGE=test-server:v1
LOCAL_TEST_RUNTIME_IMAGE=ubuntu:24.04

local_test_binary_mode() { [[ -n "${LOCAL_TEST_BIN_DIR}" ]]; }
local_core_binary_mode() { [[ -n "${LOCAL_CORE_BIN_DIR}" ]]; }

# shellcheck source=/dev/null
source "${FUNCTION_FILE}"

assert_hash_changes() {
  local description="$1"
  local before="$2"
  local after
  after="$(compute_prepare_conf_hash)"
  [[ "${before}" != "${after}" ]] || fail "prepare hash ignored ${description}"
  printf '%s' "${after}"
}

hash="$(compute_prepare_conf_hash)"
printf 'kind: Secret\n' >> "${CONF_ROOT}/resource.yaml"
hash="$(assert_hash_changes 'K8s conf YAML' "${hash}")"
printf 'spec: {}\n' >> "${K8S_DEPLOY_ROOT}/deployment.yaml"
hash="$(assert_hash_changes 'deployment manifest' "${hash}")"
printf '# certificate change\n' >> "${GENERATE_CERTS_SCRIPT}"
hash="$(assert_hash_changes 'certificate generator' "${hash}")"
printf '# peer identity helper change\n' >> "${TEST_CLIENT_PEER_IDENTITY}"
hash="$(assert_hash_changes 'test-client peer identity helper' "${hash}")"
printf '{"changed":true}\n' > "${PORTS_CONFIG}"
hash="$(assert_hash_changes 'ports registry' "${hash}")"
GATEWAY_IMAGE=gateway:v2
hash="$(assert_hash_changes 'image reference' "${hash}")"
printf 'test-server-v2\n' > "${LOCAL_TEST_BIN_DIR}/test_server"
assert_hash_changes 'local test binary' "${hash}" >/dev/null

LOCAL_TEST_BIN_DIR=""
hash="$(compute_prepare_conf_hash)"
LOCAL_TEST_RUNTIME_IMAGE=ubuntu:changed
[[ "${hash}" == "$(compute_prepare_conf_hash)" ]] \
  || fail "unused local test runtime image changed the prepare hash"

saved_deploy_root="${K8S_DEPLOY_ROOT}"
K8S_DEPLOY_ROOT="${TEMP_DIR}/missing-deploy-root"
if compute_prepare_conf_hash >/dev/null 2>&1; then
  fail "missing prepare input root produced a partial hash"
fi
K8S_DEPLOY_ROOT="${saved_deploy_root}"

mktemp() {
  case "$1" in
    *prepare-files*) printf '%s\n' "${TEMP_DIR}/mock-file-list" ;;
    *prepare-input*) printf '%s\n' "${TEMP_DIR}/unwritable-directory" ;;
    *) command mktemp "$@" ;;
  esac
}
mkdir -p "${TEMP_DIR}/unwritable-directory"
if compute_prepare_conf_hash >/dev/null 2>&1; then
  fail "prepare hash accepted a failed hash-input write"
fi
unset -f mktemp

LOCAL_IMAGE_MODE_RESOLVED=remote-pull
CONTROLLER_IMAGE=controller:mutable
GATEWAY_IMAGE=gateway:mutable
TEST_CLIENT_IMAGE=test-client:mutable
TEST_SERVER_IMAGE=test-server:mutable
if prepare_images_cacheable; then
  fail "mutable remote image tags were treated as cacheable"
fi
digest="sha256:$(printf 'a%.0s' {1..64})"
CONTROLLER_IMAGE="controller@${digest}"
GATEWAY_IMAGE="gateway@${digest}"
TEST_CLIENT_IMAGE="test-client@${digest}"
TEST_SERVER_IMAGE="test-server@${digest}"
prepare_images_cacheable || fail "immutable remote image digests were not cacheable"

# shellcheck source=/dev/null
source "${IMAGE_FUNCTION_FILE}"
IMAGE_ARCH=arm64
USER_TEST_CLIENT_IMAGE=""
USER_TEST_SERVER_IMAGE=""
USER_CONTROLLER_IMAGE=""
USER_GATEWAY_IMAGE=""
USER_ISOLATED_JOB_IMAGE=""
TEST_CLIENT_IMAGE=""
TEST_SERVER_IMAGE=""
CONTROLLER_IMAGE=""
GATEWAY_IMAGE=""
ISOLATED_JOB_IMAGE=""
printf '%s\n' \
  'IMAGE_REGISTRY=docker.io' \
  'IMAGE_NAMESPACE=example' \
  'IMAGE_VERSION=old' > "${IMAGE_CONFIG_FILE}"
load_image_config_file "${IMAGE_CONFIG_FILE}"
[[ "${TEST_CLIENT_IMAGE}" == "docker.io/example/edgion-test-client:old_arm64" ]] \
  || fail "initial image config did not resolve the old image"
printf '%s\n' \
  'IMAGE_REGISTRY=docker.io' \
  'IMAGE_NAMESPACE=example' \
  'IMAGE_VERSION=new' \
  'TEST_CLIENT_IMAGE=docker.io/example/edgion-test-client:new_arm64' > "${IMAGE_CONFIG_FILE}"
load_image_config_file "${IMAGE_CONFIG_FILE}"
[[ "${TEST_CLIENT_IMAGE}" == "docker.io/example/edgion-test-client:new_arm64" ]] \
  || fail "rewritten image config was masked by a previously derived image"

echo "OK: prepare state hash covers cluster preparation inputs"
