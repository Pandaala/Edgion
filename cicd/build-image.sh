#!/usr/bin/env bash

# Edgion Multi-Architecture Docker Image Build Script
#
# This script builds multi-architecture Docker images using a unified approach:
#   1. Compile binaries in Docker containers (supports arm64/amd64 from any host)
#   2. Package binaries using Dockerfile.runtime (lightweight)
#   3. Create multi-platform manifests
#
# On Apple Silicon Mac:
#   - arm64 builds run natively (fast)
#   - amd64 builds run via Rosetta (still works)
#
# Prerequisites:
#   - Docker with Buildx support
#
# Usage:
#   cicd/build-image.sh                           # Build arm64 only (default for arm64 host)
#   cicd/build-image.sh --arch amd64,arm64        # Build both architectures
#   cicd/build-image.sh --arch amd64,arm64 --push # Build and push multi-arch images

set -eo pipefail

# =============================================================================
# Configuration  (edit these variables to customise the build)
# =============================================================================

# Fallback version used when HEAD has no git tag.
# Normal release flow: git tag v0.x.y  →  script auto-detects via `git describe --tags --exact-match`
# or pass --from-tag / --version explicitly.
DEFAULT_VERSION="0.3.6"
IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
RUST_VERSION="${RUST_VERSION:-1.99.1}"
FEATURES="${FEATURES:-default}"
BINARIES="gateway controller"
# Test harness binaries. These are [[bin]] targets of the edgion-tests crate
# (formerly under examples/, now moved into edgion-tests). Built only via the
# dedicated --test-images mode, never alongside the standard images.
TEST_BINARIES="test_server test_client"
TEST_SERVER_AUXILIARY_BINARIES="ext_proc_external_processor"
TEST_CLIENT_AUXILIARY_BINARIES="cache_residue"

# =============================================================================
# Internal variables  (no need to change unless you know what you're doing)
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Script lives in cicd/; the build context / project root is one level up.
PROJECT_DIR="$(dirname "${SCRIPT_DIR}")"
BUILDER_IMAGE="edgion-builder"
BUILDER_IMAGES_READY=""

# Docker image name for the standalone controller-gateway bundle.
CONTROLLER_GATEWAY_IMAGE="edgion-controller-gateway"
GATEWAY_BINARY="edgion-gateway"
GATEWAY_FEATURES="default"
GATEWAY_NO_DEFAULT_FEATURES=false

configure_gateway_features() {
    GATEWAY_BINARY="edgion-gateway"
    BINARIES="gateway controller"
    CONTROLLER_GATEWAY_IMAGE="edgion-controller-gateway"
    if [[ "${FEATURES}" == "default" ]]; then
        GATEWAY_FEATURES="default"
        GATEWAY_NO_DEFAULT_FEATURES=false
    else
        GATEWAY_FEATURES="${FEATURES}"
        GATEWAY_NO_DEFAULT_FEATURES=true
        local subsystem
        local has_subsystem=false
        for subsystem in full kafka otlp geoip wasm; do
            if gateway_features_include "${subsystem}"; then
                has_subsystem=true
            fi
        done
        if [[ "${has_subsystem}" == "false" ]]; then
            log_warning "FEATURES='${FEATURES}' names no optional Gateway subsystem; Kafka, OTLP, GeoIP and Wasm are compiled out (add 'full' to keep them)"
        fi
    fi
}

gateway_features_include() {
    local feature=$1
    local feature_list=${GATEWAY_FEATURES//,/ }
    local feature_tokens=()
    local token

    read -r -a feature_tokens <<< "${feature_list}"
    for token in "${feature_tokens[@]}"; do
        if [[ "${token}" == "${feature}" ]]; then
            return 0
        fi
    done
    return 1
}

# Ensure Docker Buildx builder exists and is selected
ensure_buildx() {
    if ! docker buildx inspect edgion-builder &> /dev/null; then
        log_info "Creating Docker Buildx builder..."
        docker buildx create --name edgion-builder --use
    else
        docker buildx use edgion-builder
    fi
}

get_arch_info() {
    local arch=$1
    case "${arch}" in
        arm64)
            echo "linux/arm64:aarch64-unknown-linux-gnu:arm64"
            ;;
        amd64)
            echo "linux/amd64:x86_64-unknown-linux-gnu:amd64"
            ;;
        *)
            echo ""
            ;;
    esac
}

# Auto-detect version from git tag when VERSION is still unset.
if [[ -z "${VERSION:-}" ]]; then
    if git rev-parse --git-dir > /dev/null 2>&1; then
        GIT_TAG=$(git describe --tags --exact-match 2>/dev/null || echo "")
        if [[ -n "${GIT_TAG}" ]]; then
            VERSION="${GIT_TAG}"
        else
            VERSION="${DEFAULT_VERSION}"
        fi
    else
        VERSION="${DEFAULT_VERSION}"
    fi
fi

# =============================================================================
# Colors and Logging
# =============================================================================

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $*"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
log_warning() { echo -e "${YELLOW}[WARNING]${NC} $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }
log_stage() {
    echo -e "\n${CYAN}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${CYAN}  $*${NC}"
    echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}\n"
}

# =============================================================================
# Usage
# =============================================================================

show_usage() {
    cat << EOF
Edgion Multi-Architecture Docker Image Build Script

Usage: $0 [options]

This script builds multi-architecture Docker images using Docker containers:
  1. Compile binaries in platform-specific Docker containers
  2. Package using Dockerfile.runtime (lightweight)
  3. Create multi-platform manifests (with --push)

Options:
    --arch ARCH                  Comma-separated architectures (default: auto-detect)
                                 Available: arm64, amd64
    --push                       Push images and create multi-arch manifest
    --rebuild                    Force rebuild binaries (ignore cache)
    --version TAG                Specify version tag (default: git tag or "${DEFAULT_VERSION}")
    --from-tag                   Use current git tag as version (fails if HEAD has no tag)
    --compile-only               Only compile binaries, don't build images
    --test-images                Build ONLY the test harness images
                                 (test_server + test_client); skips standard images
    --controller-gateway         Build standalone bundle image: controller + gateway
                                 Image: edgion-controller-gateway
                                 (skips standard images)
    -h, --help                   Show this help message

Test-Images Notes:
    --test-images builds only the edgion-tests harness binaries (test_server,
    test_client) as their own images (edgion-test-server, edgion-test-client).
    It runs as a separate mode and does NOT build the controller / gateway
    images. Combine with --push to push them. Mutually exclusive with
    --controller-gateway.

Controller-Gateway Notes:
    --controller-gateway produces a standalone image that bundles controller +
    gateway (+ cli) into a single container for quick evaluation / single-node
    use. The management dashboard is NOT included (deploy it separately). When
    specified, standard images are NOT built.

Environment Variables:
    IMAGE_REGISTRY      Docker registry (default: docker.io)
    IMAGE_NAMESPACE     Image namespace (default: pandaala)
    VERSION             Image version (overrides git tag; default: ${DEFAULT_VERSION})
    RUST_VERSION        Rust version for builder (default: 1.99.1)
    FEATURES            Gateway Cargo features (default: default). An explicit list
                        replaces the defaults; include `full` to keep Kafka, OTLP,
                        GeoIP and Wasm support

Examples:
    # Build for current architecture only
    $0

    # Build for both arm64 and amd64
    $0 --arch arm64,amd64

    # Build and push multi-arch images
    $0 --arch arm64,amd64 --push

    # Build from git tag (interactive confirmation)
    git tag v0.3.0
    $0 --from-tag --arch arm64,amd64 --push

    # Force rebuild and push
    $0 --arch arm64,amd64 --rebuild --push

    # Just compile, don't build images
    $0 --arch arm64 --compile-only

    # Build and push ONLY the test harness images (test_server, test_client)
    $0 --arch arm64,amd64 --push --test-images

    # Build controller-gateway bundle: controller + gateway
    $0 --controller-gateway

Build Flow:
    Stage 1: Build Docker builder image (one-time)
    Stage 2: Compile binaries for each architecture in Docker
    Stage 3: Build Docker images for each architecture
    Stage 4: Merge into multi-platform manifests (only with --push)

EOF
}

# =============================================================================
# Prerequisites Check
# =============================================================================

check_prerequisites() {
    log_info "Checking prerequisites..."
    local has_error=false

    # Check Docker
    if ! command -v docker &> /dev/null; then
        log_error "Docker not found. Please install Docker."
        has_error=true
    else
        if ! docker info &> /dev/null; then
            log_error "Docker daemon not running. Please start Docker."
            has_error=true
        fi
    fi

    # Check Docker Buildx
    if ! docker buildx version &> /dev/null; then
        log_error "Docker Buildx not found."
        log_info "Please install Docker Buildx or upgrade to Docker Desktop."
        has_error=true
    fi

    # Check required files
    if [[ ! -f "${PROJECT_DIR}/cicd/docker/Dockerfile.runtime" ]]; then
        log_error "cicd/docker/Dockerfile.runtime not found"
        has_error=true
    fi

    if [[ ! -f "${PROJECT_DIR}/cicd/docker/Dockerfile.builder" ]]; then
        log_error "cicd/docker/Dockerfile.builder not found"
        has_error=true
    fi

    if [[ "${has_error}" == "true" ]]; then
        exit 1
    fi

    log_success "Prerequisites check passed"
}

# =============================================================================
# Auto-detect host architecture
# =============================================================================

detect_host_arch() {
    local arch
    arch=$(uname -m)
    case "${arch}" in
        x86_64|amd64)
            echo "amd64"
            ;;
        aarch64|arm64)
            echo "arm64"
            ;;
        *)
            log_warning "Unknown architecture: ${arch}, defaulting to amd64"
            echo "amd64"
            ;;
    esac
}

# =============================================================================
# Stage 1: Ensure builder image exists
# =============================================================================

builder_image_tag() {
    local platform=$1
    echo "${BUILDER_IMAGE}:${platform//\//-}"
}

ensure_builder_image() {
    local platform=$1
    local tag
    tag=$(builder_image_tag "${platform}")

    case " ${BUILDER_IMAGES_READY} " in
        *" ${tag} "*) return 0 ;;
    esac

    # Always ask Docker to build once per architecture in this invocation. Its
    # layer cache makes an unchanged builder cheap, while Dockerfile dependency
    # changes (such as the clang/runtime tools required by FIPS verification)
    # cannot be hidden behind a stale local tag.
    log_info "Ensuring builder image for ${platform}..."
    docker build \
        --platform "${platform}" \
        --build-arg "RUST_VERSION=${RUST_VERSION}" \
        -t "${tag}" \
        -f "${PROJECT_DIR}/cicd/docker/Dockerfile.builder" \
        "${PROJECT_DIR}"

    BUILDER_IMAGES_READY="${BUILDER_IMAGES_READY} ${tag}"
    log_success "Builder image ${tag} ready"
}

verify_fips_binary() {
    local arch=$1
    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local builder_tag
    builder_tag=$(builder_image_tag "${platform}")

    log_info "Verifying BoringSSL FIPS mode and the approved TLS profile for ${arch}..."
    docker run --rm \
        --platform "${platform}" \
        -v "${PROJECT_DIR}":/project \
        -v "${HOME}/.cargo/registry":/usr/local/cargo/registry \
        -v "${HOME}/.cargo/git":/usr/local/cargo/git \
        -v "edgion-rustup-${suffix}":/usr/local/rustup \
        -w /project \
        -e "EDGION_GATEWAY_BIN=/project/target/${target}/release/edgion-gateway" \
        -e "EDGION_CONTROLLER_BIN=/project/target/${target}/release/edgion-controller" \
        -e "EDGION_CLI_BIN=/project/target/${target}/release/edgion-cli" \
        -e EDGION_EXPECT_FIPS_MODULE=1 \
        "${builder_tag}" \
        bash edgion-tests/integration/scripts/integration/test_fips_policy.sh

    log_success "BoringSSL FIPS mode and TLS profile verified for ${arch}"
}

# =============================================================================
# Stage 2: Compile binaries using Docker
# =============================================================================

compile_binaries() {
    local arch=$1
    local force_rebuild=$2

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local builder_tag
    builder_tag=$(builder_image_tag "${platform}")

    log_info "Compiling binaries for ${arch} (${target})..."
    log_info "This may take several minutes on first build..."

    # Ensure builder image exists
    ensure_builder_image "${platform}"

    local gateway_feature_args=""
    if [[ "${GATEWAY_NO_DEFAULT_FEATURES}" == "true" ]]; then
        gateway_feature_args="--no-default-features --features \"${GATEWAY_FEATURES}\""
    fi

    local build_cmd="cargo build --locked --release \
            --target \"${target}\" \
            -p edgion-controller \
            --bin edgion-controller \
            -p edgion-cli \
            --bin edgion-cli && \
        cargo build --locked --release \
            --target \"${target}\" \
            -p edgion-gateway \
            --bin \"${GATEWAY_BINARY}\" \
            ${gateway_feature_args}"

    local docker_args=(
        run --rm
        -e CARGO_BUILD_JOBS
        --platform "${platform}"
        -v "${PROJECT_DIR}":/project
        -v "${HOME}/.cargo/registry":/usr/local/cargo/registry
        -v "${HOME}/.cargo/git":/usr/local/cargo/git
        -v "edgion-rustup-${suffix}":/usr/local/rustup
        -w /project
    )

    if [[ "${force_rebuild}" == "true" ]]; then
        log_info "Force rebuild requested for ${arch}: cleaning target ${target} first"
        build_cmd="cargo clean --target \"${target}\" && ${build_cmd}"
    fi

    # Run compilation in Docker
    # Use named volume for rustup to persist toolchain downloads across runs
    docker_args+=("${builder_tag}" bash -c "${build_cmd}")
    docker "${docker_args[@]}"

    if [[ $? -ne 0 ]]; then
        log_error "Compilation failed for ${arch}"
        exit 1
    fi

    log_success "Compiled binaries for ${arch}"

    if gateway_features_include "fips"; then
        verify_fips_binary "${arch}"
    fi
}

# =============================================================================
# Test images: Compile test harness binaries using Docker (only --test-images)
# =============================================================================

compile_test_binaries() {
    local arch=$1
    local force_rebuild=$2

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local bin_path="target/${target}/release"
    local builder_tag
    builder_tag=$(builder_image_tag "${platform}")

    log_info "Compiling test binaries for ${arch} (${target})..."

    # Ensure builder image exists
    ensure_builder_image "${platform}"

    # test_server/test_client are [[bin]] targets of the edgion-tests crate.
    # They use rustls directly and define no boringssl/rustls feature, so they
    # build with `-p edgion-tests --bin <name>` and take no `--features` flag.
    local testbin_args=""
    for testbin in ${TEST_BINARIES}; do
        testbin_args+=" --bin ${testbin}"
    done
    for auxiliary_bin in ${TEST_CLIENT_AUXILIARY_BINARIES}; do
        testbin_args+=" --bin ${auxiliary_bin}"
    done

    local build_cmd="cargo build --locked --release \
            --target \"${target}\" \
            -p edgion-tests${testbin_args}"

    if [[ "${force_rebuild}" == "true" ]]; then
        log_info "Force rebuild requested for test binaries (${arch})"
        build_cmd="cargo clean --target \"${target}\" -p edgion-tests && ${build_cmd}"
    fi

    # Run compilation in Docker
    docker run --rm \
        -e CARGO_BUILD_JOBS \
        --platform "${platform}" \
        -v "${PROJECT_DIR}":/project \
        -v "${HOME}/.cargo/registry":/usr/local/cargo/registry \
        -v "${HOME}/.cargo/git":/usr/local/cargo/git \
        -v "edgion-rustup-${suffix}":/usr/local/rustup \
        -w /project \
        "${builder_tag}" \
        bash -c "${build_cmd}"

    if [[ $? -ne 0 ]]; then
        log_error "Test binaries compilation failed for ${arch}"
        exit 1
    fi

    docker run --rm \
        --platform "${platform}" \
        -v "${PROJECT_DIR}":/project \
        -w /project/edgion-tests/integration/fixtures/ext_proc_external \
        "${builder_tag}" \
        go build -mod=readonly -trimpath -o "/project/${bin_path}/ext_proc_external_processor" .

    log_success "Compiled test binaries for ${arch}"
}

# =============================================================================
# Stage 3: Build Docker images
# =============================================================================

build_images() {
    local arch=$1
    local push=$2
    local version=$3

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local bin_path="target/${target}/release"

    ensure_buildx

    for binary in ${BINARIES}; do
        local image_base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${binary}"
        local image_tag="${image_base}:${version}_${suffix}"

        log_info "Building ${binary} image for ${arch}..."
        log_info "  Tag: ${image_tag}"

        local build_cmd=(
            docker buildx build
            --file "${PROJECT_DIR}/cicd/docker/Dockerfile.runtime"
            --build-arg "BINARY=${binary}"
            --build-arg "BUILD_TYPE=bin"
            --build-arg "BINARY_PATH=${bin_path}"
            --platform "${platform}"
            --tag "${image_tag}"
        )

        if [[ "${push}" == "true" ]]; then
            build_cmd+=(--push)
        else
            build_cmd+=(--load)
        fi

        build_cmd+=("${PROJECT_DIR}")

        "${build_cmd[@]}"

        if [[ $? -ne 0 ]]; then
            log_error "Failed to build ${binary} image for ${arch}"
            exit 1
        fi

        log_success "Built ${binary}:${version}_${suffix}"

        # Store for report
        BUILD_INFO+=("${binary}|${image_tag}|${arch}")
    done
}

# =============================================================================
# Test images: Build test harness Docker images (only --test-images)
# =============================================================================

build_test_images() {
    local arch=$1
    local push=$2
    local version=$3

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local bin_path="target/${target}/release"

    ensure_buildx

    for testbin in ${TEST_BINARIES}; do
        # Convert binary name to image name: test_server -> edgion-test-server
        local image_name="${testbin//_/-}"
        local image_base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}"
        local image_tag="${image_base}:${version}_${suffix}"
        local runtime_base="debian:bookworm-slim"
        local extra_packages=""
        local auxiliary_binaries=""

        # test_client image should include debug/test tools.
        if [[ "${testbin}" == "test_client" ]]; then
            extra_packages="curl bash"
            auxiliary_binaries="${TEST_CLIENT_AUXILIARY_BINARIES}"
        elif [[ "${testbin}" == "test_server" ]]; then
            auxiliary_binaries="${TEST_SERVER_AUXILIARY_BINARIES}"
        fi

        log_info "Building ${testbin} image for ${arch}..."
        log_info "  Tag: ${image_tag}"
        log_info "  Runtime base: ${runtime_base}"

        # BUILD_TYPE=example is the Dockerfile.runtime contract for an
        # unprefixed standalone binary (no edgion- prefix, no cli copy), which
        # is exactly how the edgion-tests harness binaries are laid out.
        local build_cmd=(
            docker buildx build
            --file "${PROJECT_DIR}/cicd/docker/Dockerfile.runtime"
            --build-arg "BINARY=${testbin}"
            --build-arg "BUILD_TYPE=example"
            --build-arg "BINARY_PATH=${bin_path}"
            --build-arg "RUNTIME_BASE=${runtime_base}"
            --build-arg "EXTRA_PACKAGES=${extra_packages}"
            --build-arg "AUXILIARY_BINARIES=${auxiliary_binaries}"
            --platform "${platform}"
            --tag "${image_tag}"
        )

        if [[ "${push}" == "true" ]]; then
            build_cmd+=(--push)
        else
            build_cmd+=(--load)
        fi

        build_cmd+=("${PROJECT_DIR}")

        "${build_cmd[@]}"

        if [[ $? -ne 0 ]]; then
            log_error "Failed to build ${testbin} image for ${arch}"
            exit 1
        fi

        log_success "Built ${image_name}:${version}_${suffix}"

        # Store for report
        BUILD_INFO+=("${testbin}|${image_tag}|${arch}")
    done
}

# =============================================================================
# Stage 4: Merge multi-architecture manifests
# =============================================================================

merge_manifests() {
    local version=$1
    shift
    local archs=("$@")

    log_stage "Stage 4: Creating multi-architecture manifests"

    # Extract semantic version components
    local version_no_v="${version#v}"
    local major="" minor=""

    if [[ "${version_no_v}" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then
        major=$(echo "${version_no_v}" | cut -d. -f1)
        minor=$(echo "${version_no_v}" | cut -d. -f1-2)
    fi

    for binary in ${BINARIES}; do
        local base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${binary}"

        log_info "Creating multi-arch manifest for ${binary}..."

        # Build tag list
        local tags=()
        tags+=(-t "${base}:${version_no_v}")
        tags+=(-t "${base}:latest")

        if [[ -n "${minor}" ]]; then
            tags+=(-t "${base}:${minor}")
        fi
        if [[ -n "${major}" ]]; then
            tags+=(-t "${base}:${major}")
        fi

        # Source images
        local sources=()
        for arch in "${archs[@]}"; do
            local arch_info
            arch_info=$(get_arch_info "${arch}")
            IFS=':' read -r _ _ suffix <<< "${arch_info}"
            sources+=("${base}:${version}_${suffix}")
        done

        log_info "  Tags: ${version_no_v}, latest${minor:+, ${minor}}${major:+, ${major}}"
        log_info "  Sources: ${sources[*]}"

        docker buildx imagetools create \
            "${tags[@]}" \
            "${sources[@]}"

        if [[ $? -ne 0 ]]; then
            log_error "Failed to create manifest for ${binary}"
            exit 1
        fi

        log_success "Created multi-arch manifest for ${binary}"
    done
}

# =============================================================================
# Test images: Merge test harness multi-architecture manifests (--test-images)
# =============================================================================

merge_test_manifests() {
    local version=$1
    shift
    local archs=("$@")

    log_info "Creating multi-architecture manifests for test images..."

    # Extract semantic version components
    local version_no_v="${version#v}"
    local major="" minor=""

    if [[ "${version_no_v}" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then
        major=$(echo "${version_no_v}" | cut -d. -f1)
        minor=$(echo "${version_no_v}" | cut -d. -f1-2)
    fi

    for testbin in ${TEST_BINARIES}; do
        # Convert binary name to image name: test_server -> edgion-test-server
        local image_name="${testbin//_/-}"
        local base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}"

        log_info "Creating multi-arch manifest for ${testbin}..."

        # Build tag list
        local tags=()
        tags+=(-t "${base}:${version_no_v}")
        tags+=(-t "${base}:latest")

        if [[ -n "${minor}" ]]; then
            tags+=(-t "${base}:${minor}")
        fi
        if [[ -n "${major}" ]]; then
            tags+=(-t "${base}:${major}")
        fi

        # Source images
        local sources=()
        for arch in "${archs[@]}"; do
            local arch_info
            arch_info=$(get_arch_info "${arch}")
            IFS=':' read -r _ _ suffix <<< "${arch_info}"
            sources+=("${base}:${version}_${suffix}")
        done

        log_info "  Tags: ${version_no_v}, latest${minor:+, ${minor}}${major:+, ${major}}"
        log_info "  Sources: ${sources[*]}"

        docker buildx imagetools create \
            "${tags[@]}" \
            "${sources[@]}"

        if [[ $? -ne 0 ]]; then
            log_error "Failed to create manifest for ${testbin}"
            exit 1
        fi

        log_success "Created multi-arch manifest for ${testbin}"
    done
}

# =============================================================================
# Controller-Gateway: Compile backend binaries for the bundle
# =============================================================================

compile_controller_gateway_binaries() {
    local arch=$1
    local force_rebuild=$2

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local builder_tag
    builder_tag=$(builder_image_tag "${platform}")

    log_info "Compiling controller-gateway binaries for ${arch} (${target})..."
    log_info "This may take several minutes on first build..."

    ensure_builder_image "${platform}"

    local gateway_feature_args=""
    if [[ "${GATEWAY_NO_DEFAULT_FEATURES}" == "true" ]]; then
        gateway_feature_args="--no-default-features --features \"${GATEWAY_FEATURES}\""
    fi

    local build_cmd="cargo build --locked --release \
            --target \"${target}\" \
            -p edgion-controller \
            --bin edgion-controller \
            -p edgion-cli \
            --bin edgion-cli && \
        cargo build --locked --release \
            --target \"${target}\" \
            -p edgion-gateway \
            --bin \"${GATEWAY_BINARY}\" \
            ${gateway_feature_args}"

    local docker_args=(
        run --rm
        -e CARGO_BUILD_JOBS
        --platform "${platform}"
        -v "${PROJECT_DIR}":/project
        -v "${HOME}/.cargo/registry":/usr/local/cargo/registry
        -v "${HOME}/.cargo/git":/usr/local/cargo/git
        -v "edgion-rustup-${suffix}":/usr/local/rustup
        -w /project
    )

    if [[ "${force_rebuild}" == "true" ]]; then
        log_info "Force rebuild requested for ${arch}: cleaning target ${target} first"
        build_cmd="cargo clean --target \"${target}\" && ${build_cmd}"
    fi

    docker_args+=("${builder_tag}" bash -c "${build_cmd}")
    docker "${docker_args[@]}"

    if [[ $? -ne 0 ]]; then
        log_error "Controller-gateway compilation failed for ${arch}"
        exit 1
    fi

    log_success "Compiled controller-gateway binaries for ${arch}"

    if gateway_features_include "fips"; then
        verify_fips_binary "${arch}"
    fi
}

# =============================================================================
# Controller-Gateway: Build the standalone bundle Docker image
# =============================================================================

build_controller_gateway_image() {
    local arch=$1
    local push=$2
    local version=$3

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local bin_path="target/${target}/release"

    ensure_buildx

    local image_name="${CONTROLLER_GATEWAY_IMAGE}"
    local image_base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/${image_name}"
    local image_tag="${image_base}:${version}_${suffix}"

    log_info "Building controller-gateway image for ${arch}..."
    log_info "  Tag: ${image_tag}"

    local build_cmd=(
        docker buildx build
        --file "${PROJECT_DIR}/cicd/docker/Dockerfile.controller-gateway"
        --build-arg "BINARY_PATH=${bin_path}"
        --build-arg "GATEWAY_BINARY=${GATEWAY_BINARY}"
        --platform "${platform}"
        --tag "${image_tag}"
    )

    if [[ "${push}" == "true" ]]; then
        build_cmd+=(--push)
    else
        build_cmd+=(--load)
    fi

    build_cmd+=("${PROJECT_DIR}")

    "${build_cmd[@]}"

    if [[ $? -ne 0 ]]; then
        log_error "Failed to build controller-gateway image for ${arch}"
        exit 1
    fi

    log_success "Built ${image_name}:${version}_${suffix}"
    BUILD_INFO+=("controller-gateway|${image_tag}|${arch}")
}

# =============================================================================
# Controller-Gateway: Merge multi-architecture manifest for the bundle image
# =============================================================================

merge_controller_gateway_manifest() {
    local version=$1
    shift 1
    local archs=("$@")

    local version_no_v="${version#v}"
    local major="" minor=""

    if [[ "${version_no_v}" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then
        major=$(echo "${version_no_v}" | cut -d. -f1)
        minor=$(echo "${version_no_v}" | cut -d. -f1-2)
    fi

    local image_name="${CONTROLLER_GATEWAY_IMAGE}"
    local base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/${image_name}"

    log_info "Creating multi-arch manifest for ${image_name}..."

    local tags=()
    tags+=(-t "${base}:${version_no_v}")
    tags+=(-t "${base}:latest")

    if [[ -n "${minor}" ]]; then
        tags+=(-t "${base}:${minor}")
    fi
    if [[ -n "${major}" ]]; then
        tags+=(-t "${base}:${major}")
    fi

    local sources=()
    for arch in "${archs[@]}"; do
        local arch_info
        arch_info=$(get_arch_info "${arch}")
        IFS=':' read -r _ _ suffix <<< "${arch_info}"
        sources+=("${base}:${version}_${suffix}")
    done

    log_info "  Tags: ${version_no_v}, latest${minor:+, ${minor}}${major:+, ${major}}"
    log_info "  Sources: ${sources[*]}"

    docker buildx imagetools create \
        "${tags[@]}" \
        "${sources[@]}"

    if [[ $? -ne 0 ]]; then
        log_error "Failed to create manifest for ${image_name}"
        exit 1
    fi

    log_success "Created multi-arch manifest for ${image_name}"
}

# =============================================================================
# Controller-Gateway: Print summary for the bundle image
# =============================================================================

print_controller_gateway_summary() {
    local version_no_v=$1
    local push=$2
    shift 2
    local archs=("$@")

    local image_name="${CONTROLLER_GATEWAY_IMAGE}"

    if [[ "${push}" == "true" ]]; then
        echo "  ${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/${image_name}:${version_no_v}"
    else
        for arch in "${archs[@]}"; do
            local arch_info
            arch_info=$(get_arch_info "${arch}")
            IFS=':' read -r _ _ suffix <<< "${arch_info}"
            echo "  ${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/${image_name}:${version}_${suffix} (local)"
        done
    fi
}

# =============================================================================
# Main
# =============================================================================

main() {
    # Show help if requested
    if [[ $# -gt 0 ]] && [[ "$1" == "-h" || "$1" == "--help" ]]; then
        show_usage
        exit 0
    fi

    # Parse options
    local archs_arg=""
    local push=false
    local rebuild=false
    local compile_only=false
    local test_images=false
    local controller_gateway=false
    local from_tag=false
    local version="${VERSION}"

    while [[ $# -gt 0 ]]; do
        case $1 in
            --arch)
                archs_arg=$2
                shift 2
                ;;
            --push)
                push=true
                shift
                ;;
            --rebuild)
                rebuild=true
                shift
                ;;
            --version)
                version=$2
                shift 2
                ;;
            --from-tag)
                from_tag=true
                shift
                ;;
            --compile-only)
                compile_only=true
                shift
                ;;
            --test-images)
                test_images=true
                shift
                ;;
            --controller-gateway)
                controller_gateway=true
                shift
                ;;
            -h|--help)
                show_usage
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                show_usage
                exit 1
                ;;
        esac
    done

    # Modes are mutually exclusive
    if [[ "${test_images}" == "true" ]] && [[ "${controller_gateway}" == "true" ]]; then
        log_error "--test-images and --controller-gateway are mutually exclusive"
        exit 1
    fi

    if [[ "${test_images}" != "true" ]]; then
        configure_gateway_features
    fi

    # --from-tag: resolve version from current git tag
    if [[ "${from_tag}" == "true" ]]; then
        if ! git rev-parse --git-dir > /dev/null 2>&1; then
            log_error "Not a git repository, cannot use --from-tag"
            exit 1
        fi
        local git_tag
        git_tag=$(git describe --tags --exact-match 2>/dev/null || echo "")
        if [[ -z "${git_tag}" ]]; then
            log_error "HEAD is not on a git tag. Please create a tag first:"
            log_error "  git tag v0.x.0 && cicd/build-image.sh --from-tag --push"
            exit 1
        fi
        local git_commit
        git_commit=$(git rev-parse --short HEAD)
        local git_branch
        git_branch=$(git branch --show-current 2>/dev/null || echo "detached")

        version="${git_tag}"

        echo ""
        log_info "┌─────────────────────────────────────────────┐"
        log_info "│  Build from Git Tag                         │"
        log_info "├─────────────────────────────────────────────┤"
        log_info "│  Tag:      ${git_tag}"
        log_info "│  Commit:   ${git_commit}"
        log_info "│  Branch:   ${git_branch}"
        log_info "│  Push:     ${push}"
        log_info "└─────────────────────────────────────────────┘"
        echo ""
        read -rp "$(echo -e "${YELLOW}Proceed with version ${git_tag}? [y/N]: ${NC}")" confirm
        if [[ "${confirm}" != "y" && "${confirm}" != "Y" ]]; then
            log_info "Aborted."
            exit 0
        fi
        echo ""
    fi

    # Auto-detect architecture if not specified
    if [[ -z "${archs_arg}" ]]; then
        archs_arg=$(detect_host_arch)
        log_info "Auto-detected host architecture: ${archs_arg}"
    fi

    # Parse architectures into array
    IFS=',' read -ra archs <<< "${archs_arg}"

    # Validate architectures
    for arch in "${archs[@]}"; do
        local arch_info
        arch_info=$(get_arch_info "${arch}")
        if [[ -z "${arch_info}" ]]; then
            log_error "Unknown architecture: ${arch}"
            log_info "Available: arm64, amd64"
            exit 1
        fi
    done

    # Header
    echo ""
    log_info "Edgion Multi-Architecture Docker Image Builder"
    log_info "Version: ${version}"
    log_info "Architectures: ${archs[*]}"
    log_info "Registry: ${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}"
    log_info "Push: ${push}"
    if [[ "${controller_gateway}" == "true" ]]; then
        log_info "Mode: Controller-Gateway (standalone bundle image)"
        log_info "Gateway binary: ${GATEWAY_BINARY}"
        log_info "Gateway features: ${GATEWAY_FEATURES}"
        log_info "  → ${CONTROLLER_GATEWAY_IMAGE} (controller + gateway)"
    elif [[ "${test_images}" == "true" ]]; then
        log_info "Mode: Test images (test harness only)"
        log_info "  → edgion-test-server, edgion-test-client"
    else
        log_info "Mode: Standard"
        log_info "Gateway binary: ${GATEWAY_BINARY}"
        log_info "Gateway features: ${GATEWAY_FEATURES}"
        log_info "  → edgion-${BINARIES// /, edgion-}"
    fi
    echo ""

    check_prerequisites

    BUILD_INFO=()

    # =========================================================================
    # Controller-Gateway path — completely separate from standard
    # =========================================================================
    if [[ "${controller_gateway}" == "true" ]]; then

        # --- Compile backend binaries ---
        log_stage "Controller-Gateway: Compiling backend binaries"
        for arch in "${archs[@]}"; do
            compile_controller_gateway_binaries "${arch}" "${rebuild}"
        done

        if [[ "${compile_only}" == "true" ]]; then
            log_success "Controller-gateway compilation completed (--compile-only)"
            exit 0
        fi

        # --- Build Docker image ---
        log_stage "Controller-Gateway: Building Docker images"
        for arch in "${archs[@]}"; do
            build_controller_gateway_image "${arch}" "${push}" "${version}"
        done

        # --- Merge manifests ---
        if [[ "${push}" == "true" ]] && [[ ${#archs[@]} -gt 1 ]]; then
            log_stage "Controller-Gateway: Creating multi-architecture manifests"
            merge_controller_gateway_manifest "${version}" "${archs[@]}"
        elif [[ "${push}" == "true" ]]; then
            log_info "Single architecture, creating simple tags..."
            local version_no_v="${version#v}"
            local arch_info
            arch_info=$(get_arch_info "${archs[0]}")
            IFS=':' read -r _ _ suffix <<< "${arch_info}"

            local base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/${CONTROLLER_GATEWAY_IMAGE}"
            docker buildx imagetools create \
                -t "${base}:${version_no_v}" \
                -t "${base}:latest" \
                "${base}:${version}_${suffix}"
        fi

        # --- Summary ---
        echo ""
        log_success "Controller-gateway build completed successfully!"
        echo ""
        log_info "Built controller-gateway image:"
        local version_no_v="${version#v}"
        print_controller_gateway_summary "${version_no_v}" "${push}" "${archs[@]}"
        echo ""
        return 0
    fi

    # =========================================================================
    # Test-images path — test_server + test_client only (separate from standard)
    # =========================================================================
    if [[ "${test_images}" == "true" ]]; then

        # --- Compile test harness binaries ---
        log_stage "Test images: Compiling test harness binaries"
        for arch in "${archs[@]}"; do
            compile_test_binaries "${arch}" "${rebuild}"
        done

        if [[ "${compile_only}" == "true" ]]; then
            log_success "Test binaries compilation completed (--compile-only)"
            exit 0
        fi

        # --- Build Docker images ---
        log_stage "Test images: Building Docker images"
        for arch in "${archs[@]}"; do
            build_test_images "${arch}" "${push}" "${version}"
        done

        # --- Merge manifests (only when pushing) ---
        if [[ "${push}" == "true" ]] && [[ ${#archs[@]} -gt 1 ]]; then
            log_stage "Test images: Creating multi-architecture manifests"
            merge_test_manifests "${version}" "${archs[@]}"
        elif [[ "${push}" == "true" ]]; then
            log_info "Single architecture, creating simple tags..."
            local version_no_v="${version#v}"
            local arch_info
            arch_info=$(get_arch_info "${archs[0]}")
            IFS=':' read -r _ _ suffix <<< "${arch_info}"

            for testbin in ${TEST_BINARIES}; do
                local image_name="${testbin//_/-}"
                local base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}"
                docker buildx imagetools create \
                    -t "${base}:${version_no_v}" \
                    -t "${base}:latest" \
                    "${base}:${version}_${suffix}"
            done
        else
            log_info "Skipping manifest creation (not pushing)"
            log_info "Use --push to create and push multi-arch manifests"
        fi

        # --- Summary ---
        echo ""
        log_success "Test images build completed successfully!"
        echo ""
        log_info "Built test images:"
        local version_no_v="${version#v}"
        for testbin in ${TEST_BINARIES}; do
            local image_name="${testbin//_/-}"
            if [[ "${push}" == "true" ]]; then
                echo "  ${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}:${version_no_v}"
            else
                for arch in "${archs[@]}"; do
                    local arch_info
                    arch_info=$(get_arch_info "${arch}")
                    IFS=':' read -r _ _ suffix <<< "${arch_info}"
                    echo "  ${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}:${version}_${suffix} (local)"
                done
            fi
        done
        echo ""
        return 0
    fi

    # =========================================================================
    # Standard path — gateway + controller
    # =========================================================================

    # Stage 2: Compile binaries
    log_stage "Stage 2: Compiling binaries"
    for arch in "${archs[@]}"; do
        compile_binaries "${arch}" "${rebuild}"
    done

    if [[ "${compile_only}" == "true" ]]; then
        log_success "Compilation completed (--compile-only)"
        exit 0
    fi

    # Stage 3: Build images
    log_stage "Stage 3: Building Docker images"
    for arch in "${archs[@]}"; do
        build_images "${arch}" "${push}" "${version}"
    done

    # Stage 4: Merge manifests (only when pushing)
    if [[ "${push}" == "true" ]] && [[ ${#archs[@]} -gt 1 ]]; then
        merge_manifests "${version}" "${archs[@]}"
    elif [[ "${push}" == "true" ]]; then
        log_info "Single architecture build, creating simple tags..."
        local version_no_v="${version#v}"
        local arch_info
        arch_info=$(get_arch_info "${archs[0]}")
        IFS=':' read -r _ _ suffix <<< "${arch_info}"

        for binary in ${BINARIES}; do
            local base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${binary}"

            # Tag the arch-specific image as main tags
            docker buildx imagetools create \
                -t "${base}:${version_no_v}" \
                -t "${base}:latest" \
                "${base}:${version}_${suffix}"
        done
    else
        log_info "Skipping manifest creation (not pushing)"
        log_info "Use --push to create and push multi-arch manifests"
    fi

    # Summary
    echo ""
    log_success "Build completed successfully!"
    echo ""
    log_info "Built images:"
    local version_no_v="${version#v}"
    for binary in ${BINARIES}; do
        if [[ "${push}" == "true" ]]; then
            echo "  ${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${binary}:${version_no_v}"
        else
            for arch in "${archs[@]}"; do
                local arch_info
                arch_info=$(get_arch_info "${arch}")
                IFS=':' read -r _ _ suffix <<< "${arch_info}"
                echo "  ${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${binary}:${version}_${suffix} (local)"
            done
        fi
    done
    echo ""
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
