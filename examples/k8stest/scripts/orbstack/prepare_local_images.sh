#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
BUILD_SCRIPT="${PROJECT_ROOT}/build-image.sh"

IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
VERSION="${VERSION:-dev}"
ARCH="${ARCH:-auto}" # auto|arm64|amd64
WRITE_CONFIG="${WRITE_CONFIG:-$PROJECT_ROOT/examples/k8stest/scripts/config/images.env}"
REBUILD=false

show_help() {
  cat <<EOF
Usage: $0 [options]

Build local k8s images (controller/gateway/test-server/test-client) and print env vars
for run_k8s_integration.sh.

Options:
  --arch <auto|arm64|amd64>   Target architecture for local image (default: auto)
  --version <tag>             Image version tag prefix (default: ${VERSION})
  --rebuild                   Force rebuild binaries/images
  --write-config <path>       Write resolved image config to file
                              (default: ${WRITE_CONFIG})
  -h, --help                  Show this help

Env:
  IMAGE_REGISTRY              default: ${IMAGE_REGISTRY}
  IMAGE_NAMESPACE             default: ${IMAGE_NAMESPACE}
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch)
      ARCH="${2:-}"
      [[ -n "${ARCH}" ]] || { echo "missing value for --arch"; exit 1; }
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      [[ -n "${VERSION}" ]] || { echo "missing value for --version"; exit 1; }
      shift 2
      ;;
    --rebuild)
      REBUILD=true
      shift
      ;;
    --write-config)
      WRITE_CONFIG="${2:-}"
      [[ -n "${WRITE_CONFIG}" ]] || { echo "missing value for --write-config"; exit 1; }
      shift 2
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      echo "unknown argument: $1"
      show_help
      exit 1
      ;;
  esac
done

if [[ ! -x "${BUILD_SCRIPT}" ]]; then
  echo "build script not found or not executable: ${BUILD_SCRIPT}"
  exit 1
fi

if [[ "${ARCH}" == "auto" ]]; then
  case "$(uname -m)" in
    arm64|aarch64) ARCH="arm64" ;;
    x86_64|amd64) ARCH="amd64" ;;
    *) echo "unknown host arch: $(uname -m), fallback arch=amd64"; ARCH="amd64" ;;
  esac
fi

if [[ "${ARCH}" != "arm64" && "${ARCH}" != "amd64" ]]; then
  echo "invalid arch: ${ARCH} (expected arm64|amd64|auto)"
  exit 1
fi

BUILD_ARGS=(--arch "${ARCH}" --with-examples --version "${VERSION}")
if [[ "${REBUILD}" == "true" ]]; then
  BUILD_ARGS+=(--rebuild)
fi

echo "[INFO] building local images via ${BUILD_SCRIPT} ${BUILD_ARGS[*]}"
IMAGE_REGISTRY="${IMAGE_REGISTRY}" IMAGE_NAMESPACE="${IMAGE_NAMESPACE}" "${BUILD_SCRIPT}" "${BUILD_ARGS[@]}"

CONTROLLER_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-controller:${VERSION}_${ARCH}"
GATEWAY_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-gateway:${VERSION}_${ARCH}"
CLIENT_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-test-client:${VERSION}_${ARCH}"
SERVER_IMAGE="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-test-server:${VERSION}_${ARCH}"

if ! docker image inspect "${CONTROLLER_IMAGE}" >/dev/null 2>&1; then
  echo "[ERROR] built image not found locally: ${CONTROLLER_IMAGE}"
  exit 1
fi
if ! docker image inspect "${GATEWAY_IMAGE}" >/dev/null 2>&1; then
  echo "[ERROR] built image not found locally: ${GATEWAY_IMAGE}"
  exit 1
fi
if ! docker image inspect "${CLIENT_IMAGE}" >/dev/null 2>&1; then
  echo "[ERROR] built image not found locally: ${CLIENT_IMAGE}"
  exit 1
fi
if ! docker image inspect "${SERVER_IMAGE}" >/dev/null 2>&1; then
  echo "[ERROR] built image not found locally: ${SERVER_IMAGE}"
  exit 1
fi

echo
echo "[OK] local images prepared:"
echo "  CONTROLLER_IMAGE=${CONTROLLER_IMAGE}"
echo "  GATEWAY_IMAGE=${GATEWAY_IMAGE}"
echo "  TEST_CLIENT_IMAGE=${CLIENT_IMAGE}"
echo "  TEST_SERVER_IMAGE=${SERVER_IMAGE}"

mkdir -p "$(dirname "${WRITE_CONFIG}")"
cat > "${WRITE_CONFIG}" <<EOF
IMAGE_REGISTRY=${IMAGE_REGISTRY}
IMAGE_NAMESPACE=${IMAGE_NAMESPACE}
IMAGE_VERSION=${VERSION}
IMAGE_ARCH=${ARCH}
CONTROLLER_IMAGE=${CONTROLLER_IMAGE}
GATEWAY_IMAGE=${GATEWAY_IMAGE}
TEST_CLIENT_IMAGE=${CLIENT_IMAGE}
TEST_SERVER_IMAGE=${SERVER_IMAGE}
EOF
echo "[OK] image config written: ${WRITE_CONFIG}"
echo
echo "Run with:"
echo "  CONTROLLER_IMAGE=${CONTROLLER_IMAGE} \\"
echo "  GATEWAY_IMAGE=${GATEWAY_IMAGE} \\"
echo "  TEST_CLIENT_IMAGE=${CLIENT_IMAGE} \\"
echo "  TEST_SERVER_IMAGE=${SERVER_IMAGE} \\"
echo "  ${PROJECT_ROOT}/examples/k8stest/scripts/run_k8s_integration.sh"
