#!/usr/bin/env bash
# Fast incremental cross-compile + image build for Orbstack K8s testing.
#
# Uses the existing Docker builder container for reliable cross-compilation
# (boringssl/protobuf/bindgen all work), but NEVER cleans the target directory
# so subsequent builds are incremental (~10-30s for small code changes).
#
# First run:  full compile (~3-5 min) + image build (~30s)
# Later runs: incremental compile (~10-60s) + image build (~30s)
#
# Usage:
#   ./fast_build.sh                  # Incremental build (default)
#   ./fast_build.sh --rebuild        # Force cargo clean + full rebuild
#   ./fast_build.sh --compile-only   # Only compile, don't build images
#   ./fast_build.sh --images-only    # Only build images (binaries must exist)
#   ./fast_build.sh --bins gateway controller  # Only build specific product binaries
#
# After build, run tests with:
#   ../run_k8s_integration.sh --skip-deploy -r GRPCRoute

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONFIG_DIR="${SCRIPT_DIR}/../config"
WRITE_CONFIG="${CONFIG_DIR}/images.env"

# Image settings
IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
VERSION="${VERSION:-dev}"
BUILDER_IMAGE="edgion-builder"

# Target arch: on Apple Silicon Mac -> arm64
ARCH="${ARCH:-auto}"

# What to build
REBUILD=false
COMPILE_ONLY=false
IMAGES_ONLY=false
BUILD_BINS=()  # empty = all
BUILD_EXAMPLES=()  # empty = all
BUILT_ISOLATED_IMAGE=false

# Binaries and examples to build
ALL_BINS=(gateway controller)
ALL_EXAMPLES=(test_server test_client)

# Runtime base images per component (functions to avoid bash 3.x associative array issues)
get_runtime_base() {
  case "$1" in
    gateway|controller|ctl|test_client) echo "ubuntu:24.04" ;;
    test_server) echo "debian:bookworm-slim" ;;
    *) echo "ubuntu:24.04" ;;
  esac
}
get_extra_pkgs() {
  case "$1" in
    test_client) echo "curl bash" ;;
    *) echo "" ;;
  esac
}

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
log_ok()      { echo -e "${GREEN}[OK]${NC} $*"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $*"; }

show_help() {
  cat <<EOF
Usage: $(basename "$0") [options]

Fast incremental cross-compile + Docker image build for Orbstack.

Options:
  --rebuild           Force cargo clean before compile
  --compile-only      Only cross-compile, skip Docker image build
  --images-only       Only build Docker images (binaries must already exist)
  --bins <names...>   Only build specified product bins (gateway, controller)
  --examples <names...> Only build specified examples (test_server, test_client)
  --arch <arm64|amd64>  Target arch (default: auto-detect from host)
  --version <tag>     Image version tag (default: dev)
  -h, --help          Show this help

Environment:
  IMAGE_REGISTRY      Docker registry (default: docker.io)
  IMAGE_NAMESPACE     Image namespace (default: pandaala)
  VERSION             Image version tag (default: dev)
  ARCH                Target architecture (default: auto)
EOF
}

# Parse args
while [[ $# -gt 0 ]]; do
  case "$1" in
    --rebuild)        REBUILD=true; shift ;;
    --compile-only)   COMPILE_ONLY=true; shift ;;
    --images-only)    IMAGES_ONLY=true; shift ;;
    --arch)           ARCH="${2:?missing --arch value}"; shift 2 ;;
    --version)        VERSION="${2:?missing --version value}"; shift 2 ;;
    --bins)
      shift
      while [[ $# -gt 0 && ! "$1" =~ ^-- ]]; do
        BUILD_BINS+=("$1"); shift
      done
      ;;
    --examples)
      shift
      while [[ $# -gt 0 && ! "$1" =~ ^-- ]]; do
        BUILD_EXAMPLES+=("$1"); shift
      done
      ;;
    -h|--help) show_help; exit 0 ;;
    *) log_error "unknown argument: $1"; show_help; exit 1 ;;
  esac
done

# Resolve arch
if [[ "${ARCH}" == "auto" ]]; then
  case "$(uname -m)" in
    arm64|aarch64) ARCH="arm64" ;;
    x86_64|amd64)  ARCH="amd64" ;;
    *) log_error "unknown host arch: $(uname -m)"; exit 1 ;;
  esac
fi

case "${ARCH}" in
  arm64) TARGET="aarch64-unknown-linux-gnu"; PLATFORM="linux/arm64"; SUFFIX="arm64" ;;
  amd64) TARGET="x86_64-unknown-linux-gnu";  PLATFORM="linux/amd64"; SUFFIX="amd64" ;;
  *) log_error "invalid arch: ${ARCH}"; exit 1 ;;
esac

# Default: build everything
[[ ${#BUILD_BINS[@]} -eq 0 ]]     && BUILD_BINS=("${ALL_BINS[@]}")
[[ ${#BUILD_EXAMPLES[@]} -eq 0 ]] && BUILD_EXAMPLES=("${ALL_EXAMPLES[@]}")

BIN_PATH="target/${TARGET}/release"

log_info "Fast build: arch=${ARCH} target=${TARGET} version=${VERSION}"
log_info "  bins: ${BUILD_BINS[*]}"
log_info "  examples: ${BUILD_EXAMPLES[*]}"
echo

# ─────────────────────────────────────────────
# Step 0: Ensure builder image
# ─────────────────────────────────────────────
ensure_builder() {
  local tag="${BUILDER_IMAGE}:${PLATFORM//\//-}"
  if docker image inspect "${tag}" &>/dev/null; then
    log_info "Builder image ${tag} exists"
    return 0
  fi
  log_info "Building builder image ${tag} (one-time)..."
  docker build \
    --platform "${PLATFORM}" \
    --build-arg "RUST_VERSION=${RUST_VERSION:-1.92}" \
    -t "${tag}" \
    -f "${PROJECT_ROOT}/cicd/docker/Dockerfile.builder" \
    "${PROJECT_ROOT}"
  log_ok "Builder image ${tag} created"
}

# ─────────────────────────────────────────────
# Step 1: Cross-compile in Docker (incremental)
# ─────────────────────────────────────────────
cross_compile() {
  local builder_tag="${BUILDER_IMAGE}:${PLATFORM//\//-}"

  # Every product runtime image includes edgion-cli for diagnostics. Build it
  # once as an auxiliary binary, but do not invent a separate `ctl` image.
  local cargo_bin_args=" --bin edgion-cli"
  for b in "${BUILD_BINS[@]}"; do
    cargo_bin_args+=" --bin edgion-${b}"
  done

  # The harness binaries (test_server/test_client) are [[bin]] targets of the
  # edgion-tests crate, built with `-p edgion-tests --bin <name>`. They use
  # rustls directly and define no boringssl/rustls feature, so they are compiled
  # in a SEPARATE invocation with no `--features` flag.
  local cargo_testbin_args=""
  for e in "${BUILD_EXAMPLES[@]}"; do
    cargo_testbin_args+=" --bin ${e}"
  done
  if [[ " ${BUILD_EXAMPLES[*]} " == *" test_client "* ]]; then
    cargo_testbin_args+=" --bin cache_residue"
  fi

  local features="${FEATURES:-default}"
  local cmd=""

  if [[ "${REBUILD}" == "true" ]]; then
    log_warn "Force rebuild: cleaning target ${TARGET}"
    cmd+="cargo clean --target \"${TARGET}\" && "
  fi

  # Product bins (gateway/controller/cli) take the TLS backend feature; the
  # edgion-tests harness bins are a separate feature-less invocation.
  cmd+="cargo build --release --target \"${TARGET}\" --features \"${features}\"${cargo_bin_args}"
  if [[ -n "${cargo_testbin_args}" ]]; then
    cmd+=" && cargo build --release --target \"${TARGET}\" -p edgion-tests${cargo_testbin_args}"
  fi

  log_info "Cross-compiling (incremental)..."
  log_info "  cmd: ${cmd}"

  local start_time=$SECONDS

  docker run --rm \
    --platform "${PLATFORM}" \
    -v "${PROJECT_ROOT}":/project \
    -v "${HOME}/.cargo/registry":/usr/local/cargo/registry \
    -v "${HOME}/.cargo/git":/usr/local/cargo/git \
    -w /project \
    "${builder_tag}" \
    bash -c "${cmd}"

  # The test-server image also carries the Go ext_proc external processor,
  # built the same way as cicd/build-image.sh does.
  if [[ " ${BUILD_EXAMPLES[*]} " == *" test_server "* ]]; then
    docker run --rm \
      --platform "${PLATFORM}" \
      -v "${PROJECT_ROOT}":/project \
      -w /project/edgion-tests/integration/fixtures/ext_proc_external \
      "${builder_tag}" \
      go build -mod=readonly -trimpath -o "/project/target/${TARGET}/release/ext_proc_external_processor" .
  fi

  local elapsed=$(( SECONDS - start_time ))
  log_ok "Compilation done in ${elapsed}s"
}

# ─────────────────────────────────────────────
# Step 2: Ensure base images are cached locally
# ─────────────────────────────────────────────
ensure_base_images() {
  local needed_bases=()
  local seen=()

  # Collect unique base images we need
  for b in "${BUILD_BINS[@]}"; do
    local base; base="$(get_runtime_base "${b}")"
    if [[ ! " ${seen[*]:-} " =~ " ${base} " ]]; then
      needed_bases+=("${base}")
      seen+=("${base}")
    fi
  done
  for e in "${BUILD_EXAMPLES[@]}"; do
    local base; base="$(get_runtime_base "${e}")"
    if [[ ! " ${seen[*]:-} " =~ " ${base} " ]]; then
      needed_bases+=("${base}")
      seen+=("${base}")
    fi
  done

  for base in "${needed_bases[@]}"; do
    if docker image inspect "${base}" &>/dev/null; then
      log_info "Base image ${base} cached locally"
    else
      log_warn "Base image ${base} not found locally, pulling..."
      if ! docker pull --platform "${PLATFORM}" "${base}"; then
        log_error "Failed to pull ${base}. Try manually: docker pull ${base}"
        log_error "Or use a mirror/proxy if Docker Hub is unreachable."
        exit 1
      fi
    fi
  done
}

# ─────────────────────────────────────────────
# Step 3: Build Docker images (plain docker build, no buildx)
# ─────────────────────────────────────────────
build_one_image() {
  local name="$1"       # e.g. gateway, test_server
  local build_type="$2" # bin or example
  local image_tag="$3"
  local runtime_base="${4:-ubuntu:24.04}"
  local extra_packages="${5:-}"
  local auxiliary_binaries="${6:-}"
  local include_test_wasm="false"

  if [[ "${name}" == "gateway" ]]; then
    include_test_wasm="true"
  fi

  # For bin type, binary name is edgion-<name>, for example it's just <name>
  local binary_name="${name}"
  if [[ "${build_type}" == "bin" ]]; then
    binary_name="${name}"
  fi

  log_info "Building image: ${image_tag}"

  DOCKER_BUILDKIT=1 docker build \
    --file "${PROJECT_ROOT}/cicd/docker/Dockerfile.runtime" \
    --build-arg "BINARY=${binary_name}" \
    --build-arg "BUILD_TYPE=${build_type}" \
    --build-arg "BINARY_PATH=${BIN_PATH}" \
    --build-arg "RUNTIME_BASE=${runtime_base}" \
    --build-arg "EXTRA_PACKAGES=${extra_packages}" \
    --build-arg "AUXILIARY_BINARIES=${auxiliary_binaries}" \
    --build-arg "INCLUDE_TEST_WASM=${include_test_wasm}" \
    --platform "${PLATFORM}" \
    -t "${image_tag}" \
    "${PROJECT_ROOT}" \
    --quiet

  log_ok "  ${image_tag}"
}

selection_contains() {
  local wanted="$1"
  shift
  local selected
  for selected in "$@"; do
    if [[ "${selected}" == "${wanted}" ]]; then
      return 0
    fi
  done
  return 1
}

build_isolated_image_if_complete() {
  local image_tag binary
  if ! selection_contains gateway "${BUILD_BINS[@]}" \
    || ! selection_contains controller "${BUILD_BINS[@]}" \
    || ! selection_contains test_server "${BUILD_EXAMPLES[@]}"; then
    log_warn "Skipping the isolated-test image because this is a partial component build"
    return 0
  fi

  for binary in edgion-controller edgion-gateway edgion-cli test_server; do
    if [[ ! -f "${PROJECT_ROOT}/${BIN_PATH}/${binary}" ]]; then
      log_error "Isolated-test binary not found: ${PROJECT_ROOT}/${BIN_PATH}/${binary}"
      exit 1
    fi
  done

  image_tag="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-isolated-test:${VERSION}_${SUFFIX}"
  log_info "Building image: ${image_tag}"
  DOCKER_BUILDKIT=1 docker build \
    --file "${PROJECT_ROOT}/edgion-tests/k8s/docker/Dockerfile.isolated" \
    --build-arg "BINARY_PATH=${BIN_PATH}" \
    --platform "${PLATFORM}" \
    --tag "${image_tag}" \
    "${PROJECT_ROOT}" \
    --quiet
  BUILT_ISOLATED_IMAGE=true
  log_ok "  ${image_tag}"
}

build_images() {
  log_info "Building Docker images..."
  local start_time=$SECONDS

  # Verify binaries exist
  if [[ ! -f "${PROJECT_ROOT}/${BIN_PATH}/edgion-cli" ]]; then
    log_error "Auxiliary binary not found: ${PROJECT_ROOT}/${BIN_PATH}/edgion-cli"
    exit 1
  fi
  for b in "${BUILD_BINS[@]}"; do
    local bin_file="${PROJECT_ROOT}/${BIN_PATH}/edgion-${b}"
    if [[ ! -f "${bin_file}" ]]; then
      log_error "Binary not found: ${bin_file}"
      log_error "Run without --images-only first, or check compilation."
      exit 1
    fi
  done
  for e in "${BUILD_EXAMPLES[@]}"; do
    local bin_file="${PROJECT_ROOT}/${BIN_PATH}/${e}"
    if [[ ! -f "${bin_file}" ]]; then
      log_error "Test harness binary not found: ${bin_file}"
      exit 1
    fi
  done

  # Build bin images
  for b in "${BUILD_BINS[@]}"; do
    local image_tag="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${b}:${VERSION}_${SUFFIX}"
    local runtime_base; runtime_base="$(get_runtime_base "${b}")"
    build_one_image "${b}" "bin" "${image_tag}" "${runtime_base}" ""
  done

  # Build example images
  for e in "${BUILD_EXAMPLES[@]}"; do
    local image_name="${e//_/-}"
    local image_tag="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}:${VERSION}_${SUFFIX}"
    local runtime_base; runtime_base="$(get_runtime_base "${e}")"
    local extra; extra="$(get_extra_pkgs "${e}")"
    local auxiliary_binaries=""
    if [[ "${e}" == "test_client" ]]; then
      auxiliary_binaries="cache_residue"
    elif [[ "${e}" == "test_server" ]]; then
      auxiliary_binaries="ext_proc_external_processor"
    fi
    for auxiliary in ${auxiliary_binaries}; do
      if [[ ! -f "${PROJECT_ROOT}/${BIN_PATH}/${auxiliary}" ]]; then
        log_error "${e} auxiliary binary not found: ${PROJECT_ROOT}/${BIN_PATH}/${auxiliary}"
        exit 1
      fi
    done
    build_one_image "${e}" "example" "${image_tag}" "${runtime_base}" "${extra}" "${auxiliary_binaries}"
  done

  build_isolated_image_if_complete

  local elapsed=$(( SECONDS - start_time ))
  log_ok "All images built in ${elapsed}s"
}

# ─────────────────────────────────────────────
# Step 4: Write config for run_k8s_integration.sh
# ─────────────────────────────────────────────
write_config() {
  mkdir -p "${CONFIG_DIR}"

  local ctrl_image="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-controller:${VERSION}_${SUFFIX}"
  local gw_image="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-gateway:${VERSION}_${SUFFIX}"
  local client_image="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-test-client:${VERSION}_${SUFFIX}"
  local server_image="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-test-server:${VERSION}_${SUFFIX}"
  local isolated_image="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-isolated-test:${VERSION}_${SUFFIX}"

  cat > "${WRITE_CONFIG}" <<EOF
IMAGE_REGISTRY=${IMAGE_REGISTRY}
IMAGE_NAMESPACE=${IMAGE_NAMESPACE}
IMAGE_VERSION=${VERSION}
IMAGE_ARCH=${SUFFIX}
CONTROLLER_IMAGE=${ctrl_image}
GATEWAY_IMAGE=${gw_image}
TEST_CLIENT_IMAGE=${client_image}
TEST_SERVER_IMAGE=${server_image}
EOF

  if [[ "${BUILT_ISOLATED_IMAGE}" == "true" ]]; then
    printf 'ISOLATED_JOB_IMAGE=%s\n' "${isolated_image}" >> "${WRITE_CONFIG}"
  fi

  log_ok "Config written: ${WRITE_CONFIG}"
  echo
  echo "Run tests with:"
  echo "  ${SCRIPT_DIR}/../run_k8s_integration.sh --prepare-local-images -r GRPCRoute"
  echo
  echo "Or skip deploy if already deployed:"
  echo "  ${SCRIPT_DIR}/../run_k8s_integration.sh --prepare-local-images --skip-deploy -r GRPCRoute"
}

# ─────────────────────────────────────────────
# Main
# ─────────────────────────────────────────────
main() {
  local total_start=$SECONDS

  if [[ "${IMAGES_ONLY}" != "true" ]]; then
    ensure_builder
    cross_compile
  fi

  if [[ "${COMPILE_ONLY}" != "true" ]]; then
    ensure_base_images
    build_images
    write_config
  fi

  local total_elapsed=$(( SECONDS - total_start ))
  echo
  log_ok "Total time: ${total_elapsed}s"
}

main
