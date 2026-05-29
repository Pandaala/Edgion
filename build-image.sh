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
#   ./build-image.sh                           # Build arm64 only (default for arm64 host)
#   ./build-image.sh --arch amd64,arm64        # Build both architectures
#   ./build-image.sh --arch amd64,arm64 --push # Build and push multi-arch images

set -eo pipefail

# =============================================================================
# Configuration  (edit these variables to customise the build)
# =============================================================================

# Fallback version used when HEAD has no git tag.
# Normal release flow: git tag v0.x.y  →  script auto-detects via `git describe --tags --exact-match`
# or pass --from-tag / --version explicitly.
DEFAULT_VERSION="0.1.33"
IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
RUST_VERSION="${RUST_VERSION:-1.92}"
FEATURES="${FEATURES:-default}"
BINARIES="gateway controller"
EXAMPLES="test_server test_client"
DASHBOARD_REPO="https://github.com/Pandaala/edgion-dashboard.git"

# =============================================================================
# Internal variables  (no need to change unless you know what you're doing)
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="${SCRIPT_DIR}"
BUILDER_IMAGE="edgion-builder"
# Prefer local sibling edgion-dashboard if present; fall back to GitHub clone
DASHBOARD_LOCAL="$(dirname "${PROJECT_DIR}")/edgion-dashboard"
DASHBOARD_CLONE="${PROJECT_DIR}/docker/.all-in-one-dashboard"

# Map all-in-one mode to Docker image name
all_in_one_image_name() {
    local mode=$1
    if [[ "${mode}" == "controller-gateway" ]]; then
        echo "edgion-all-in-one"
    else
        echo "edgion-all-in-one-${mode}"
    fi
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
    --with-examples              Also build test_server and test_client images
    --with-example               Alias of --with-examples
    --all-in-one-controller-gateway     Build all-in-one image: dashboard + controller + gateway
                                 Image: edgion-all-in-one (standalone, skips standard images)
    --all-in-one-center                 Build all-in-one image: dashboard + center
                                 Image: edgion-all-in-one-center (standalone, skips standard images)
    -h, --help                   Show this help message

All-in-One Notes:
    --all-in-one-* options produce standalone images that bundle the frontend dashboard
    with backend binaries and nginx. Dashboard source priority:
      1. Local sibling directory ../edgion-dashboard (if present)
      2. Previous GitHub clone (auto-updated)
      3. Fresh clone from ${DASHBOARD_REPO}
    When any --all-in-one-* option is specified, standard images are NOT built.
    Both --all-in-one-* options can be combined to build both all-in-one images in one run.

Environment Variables:
    IMAGE_REGISTRY      Docker registry (default: docker.io)
    IMAGE_NAMESPACE     Image namespace (default: pandaala)
    VERSION             Image version (overrides git tag; default: ${DEFAULT_VERSION})
    RUST_VERSION        Rust version for builder (default: 1.92)
    FEATURES            Cargo features (default: default)

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

    # Build and push with test_server and test_client
    $0 --arch arm64,amd64 --push --with-examples

    # Build all-in-one: dashboard + controller + gateway
    $0 --all-in-one-controller-gateway

    # Build all-in-one: dashboard + center
    $0 --all-in-one-center --push

    # Build both all-in-one images at once
    $0 --all-in-one-controller-gateway --all-in-one-center --arch arm64,amd64 --push

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
    if [[ ! -f "${PROJECT_DIR}/docker/Dockerfile.runtime" ]]; then
        log_error "docker/Dockerfile.runtime not found"
        has_error=true
    fi

    if [[ ! -f "${PROJECT_DIR}/docker/Dockerfile.builder" ]]; then
        log_error "docker/Dockerfile.builder not found"
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

ensure_builder_image() {
    local platform=$1
    local tag="${BUILDER_IMAGE}:${platform//\//-}"

    if docker image inspect "${tag}" &> /dev/null; then
        log_info "Builder image ${tag} already exists"
        return 0
    fi

    log_info "Building builder image for ${platform}..."
    docker build \
        --platform "${platform}" \
        --build-arg "RUST_VERSION=${RUST_VERSION}" \
        -t "${tag}" \
        -f "${PROJECT_DIR}/docker/Dockerfile.builder" \
        "${PROJECT_DIR}"

    log_success "Builder image ${tag} created"
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
    local builder_tag="${BUILDER_IMAGE}:${platform//\//-}"

    log_info "Compiling binaries for ${arch} (${target})..."
    log_info "This may take several minutes on first build..."

    # Ensure builder image exists
    ensure_builder_image "${platform}"

    local build_cmd="cargo build --release \
            --target \"${target}\" \
            --bin edgion-gateway \
            --bin edgion-controller \
            --bin edgion-cli \
            --features \"${FEATURES}\""

    if [[ "${force_rebuild}" == "true" ]]; then
        log_info "Force rebuild requested for ${arch}: cleaning target ${target} first"
        build_cmd="cargo clean --target \"${target}\" && ${build_cmd}"
    fi

    # Run compilation in Docker
    # Use named volume for rustup to persist toolchain downloads across runs
    docker run --rm \
        --platform "${platform}" \
        -v "${PROJECT_DIR}":/project \
        -v "${HOME}/.cargo/registry":/usr/local/cargo/registry \
        -v "${HOME}/.cargo/git":/usr/local/cargo/git \
        -v "edgion-rustup-${suffix}":/usr/local/rustup \
        -w /project \
        "${builder_tag}" \
        bash -c "${build_cmd}"

        if [[ $? -ne 0 ]]; then
        log_error "Compilation failed for ${arch}"
            exit 1
        fi

    log_success "Compiled binaries for ${arch}"
}

# =============================================================================
# Stage 2b: Compile examples using Docker (only with --with-examples)
# =============================================================================

compile_examples() {
    local arch=$1
    local force_rebuild=$2

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local builder_tag="${BUILDER_IMAGE}:${platform//\//-}"

    log_info "Compiling examples for ${arch} (${target})..."

    # Ensure builder image exists
    ensure_builder_image "${platform}"

    local build_cmd="cargo build --release \
            --target \"${target}\" \
            --examples \
            --features \"${FEATURES}\""

    if [[ "${force_rebuild}" == "true" ]]; then
        # Do not clean here: binaries compiled in Stage 2 are needed by Stage 3 image packaging.
        log_info "Force rebuild requested for examples (${arch}): rebuilding examples without target clean"
    fi

    # Run compilation in Docker
    docker run --rm \
        --platform "${platform}" \
        -v "${PROJECT_DIR}":/project \
        -v "${HOME}/.cargo/registry":/usr/local/cargo/registry \
        -v "${HOME}/.cargo/git":/usr/local/cargo/git \
        -v "edgion-rustup-${suffix}":/usr/local/rustup \
        -w /project \
        "${builder_tag}" \
        bash -c "${build_cmd}"

    if [[ $? -ne 0 ]]; then
        log_error "Examples compilation failed for ${arch}"
        exit 1
    fi

    log_success "Compiled examples for ${arch}"
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
            --file docker/Dockerfile.runtime
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
# Stage 3b: Build example Docker images (only with --with-examples)
# =============================================================================

build_example_images() {
    local arch=$1
    local push=$2
    local version=$3

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local bin_path="target/${target}/release"

    ensure_buildx

    for example in ${EXAMPLES}; do
        # Convert example name to image name: test_server -> edgion-test-server
        local image_name="${example//_/-}"
        local image_base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}"
        local image_tag="${image_base}:${version}_${suffix}"
        local runtime_base="debian:bookworm-slim"
        local extra_packages=""

        # test_client image should include debug/test tools.
        if [[ "${example}" == "test_client" ]]; then
            extra_packages="curl bash"
        fi

        log_info "Building ${example} image for ${arch}..."
        log_info "  Tag: ${image_tag}"
        log_info "  Runtime base: ${runtime_base}"

        local build_cmd=(
            docker buildx build
            --file docker/Dockerfile.runtime
            --build-arg "BINARY=${example}"
            --build-arg "BUILD_TYPE=example"
            --build-arg "BINARY_PATH=${bin_path}"
            --build-arg "RUNTIME_BASE=${runtime_base}"
            --build-arg "EXTRA_PACKAGES=${extra_packages}"
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
            log_error "Failed to build ${example} image for ${arch}"
            exit 1
        fi

        log_success "Built ${image_name}:${version}_${suffix}"

        # Store for report
        BUILD_INFO+=("${example}|${image_tag}|${arch}")
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
# Stage 4b: Merge example multi-architecture manifests (only with --with-examples)
# =============================================================================

merge_example_manifests() {
    local version=$1
    shift
    local archs=("$@")

    log_info "Creating multi-architecture manifests for examples..."

    # Extract semantic version components
    local version_no_v="${version#v}"
    local major="" minor=""

    if [[ "${version_no_v}" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then
        major=$(echo "${version_no_v}" | cut -d. -f1)
        minor=$(echo "${version_no_v}" | cut -d. -f1-2)
    fi

    for example in ${EXAMPLES}; do
        # Convert example name to image name: test_server -> edgion-test-server
        local image_name="${example//_/-}"
        local base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}"

        log_info "Creating multi-arch manifest for ${example}..."

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
            log_error "Failed to create manifest for ${example}"
            exit 1
        fi

        log_success "Created multi-arch manifest for ${example}"
    done
}

# =============================================================================
# All-in-One: Clone / update frontend dashboard from GitHub
# =============================================================================

prepare_dashboard() {
    # 1. Use local sibling directory if it exists (typical dev setup)
    if [[ -f "${DASHBOARD_LOCAL}/package.json" ]]; then
        DASHBOARD_DIR="${DASHBOARD_LOCAL}"
        log_info "Using local dashboard at ${DASHBOARD_DIR}"
    # 2. Reuse previous clone if it exists
    elif [[ -d "${DASHBOARD_CLONE}/.git" ]]; then
        DASHBOARD_DIR="${DASHBOARD_CLONE}"
        log_info "Updating dashboard clone at ${DASHBOARD_DIR}..."
        git -C "${DASHBOARD_DIR}" pull --ff-only || {
            log_warning "git pull failed, using existing checkout"
        }
    # 3. Clone from GitHub as fallback
    else
        DASHBOARD_DIR="${DASHBOARD_CLONE}"
        log_info "Cloning dashboard from ${DASHBOARD_REPO}..."
        rm -rf "${DASHBOARD_DIR}"
        git clone --depth 1 "${DASHBOARD_REPO}" "${DASHBOARD_DIR}"
    fi

    if [[ ! -f "${DASHBOARD_DIR}/package.json" ]]; then
        log_error "Dashboard not found — package.json missing"
        exit 1
    fi

    log_success "Dashboard source ready at ${DASHBOARD_DIR}"
}

# =============================================================================
# All-in-One: Build frontend dashboard
# =============================================================================

compile_frontend() {
    log_stage "All-in-One: Building frontend dashboard"

    prepare_dashboard

    # Build frontend in Docker for consistency (no local Node.js required)
    log_info "Running npm ci + npm run build inside Docker (node:20-slim)..."
    local ver="${VERSION#v}"   # strip leading 'v' so package.json gets e.g. 0.1.22
    local pkg_json="${DASHBOARD_DIR}/package.json"
    local pkg_json_orig
    pkg_json_orig=$(cat "${pkg_json}")   # backup before docker modifies it

    docker run --rm \
        -v "${DASHBOARD_DIR}":/app \
        -w /app \
        node:20-slim \
        bash -c "npm pkg set version='${ver}' && npm ci --prefer-offline && npm run build"
    local rc=$?

    echo "${pkg_json_orig}" > "${pkg_json}"   # restore to avoid dirty working tree

    if [[ ${rc} -ne 0 ]]; then
        log_error "Frontend build failed"
        exit 1
    fi

    # Copy dist into Docker build context
    rm -rf "${PROJECT_DIR}/docker/.all-in-one-dist"
    cp -r "${DASHBOARD_DIR}/dist" "${PROJECT_DIR}/docker/.all-in-one-dist"

    log_success "Frontend dashboard built successfully"
}

# =============================================================================
# All-in-One: Compile backend binaries needed for a specific all-in-one mode
# =============================================================================

compile_all_in_one_binaries() {
    local arch=$1
    local force_rebuild=$2
    local all_in_one_mode=$3   # "controller-gateway" or "center"

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local builder_tag="${BUILDER_IMAGE}:${platform//\//-}"

    # Determine which binaries to compile
    local bins="--bin edgion-cli"
    case "${all_in_one_mode}" in
        controller-gateway)
            bins="${bins} --bin edgion-gateway --bin edgion-controller"
            ;;
        center)
            bins="${bins} --bin edgion-center"
            ;;
        both)
            bins="${bins} --bin edgion-gateway --bin edgion-controller --bin edgion-center"
            ;;
    esac

    log_info "Compiling all-in-one [${all_in_one_mode}] binaries for ${arch} (${target})..."
    log_info "This may take several minutes on first build..."

    ensure_builder_image "${platform}"

    local build_cmd="cargo build --release \
            --target \"${target}\" \
            ${bins} \
            --features \"${FEATURES}\""

    if [[ "${force_rebuild}" == "true" ]]; then
        log_info "Force rebuild requested for ${arch}: cleaning target ${target} first"
        build_cmd="cargo clean --target \"${target}\" && ${build_cmd}"
    fi

    docker run --rm \
        --platform "${platform}" \
        -v "${PROJECT_DIR}":/project \
        -v "${HOME}/.cargo/registry":/usr/local/cargo/registry \
        -v "${HOME}/.cargo/git":/usr/local/cargo/git \
        -v "edgion-rustup-${suffix}":/usr/local/rustup \
        -w /project \
        "${builder_tag}" \
        bash -c "${build_cmd}"

    if [[ $? -ne 0 ]]; then
        log_error "All-in-one compilation failed for ${arch} (${all_in_one_mode})"
        exit 1
    fi

    log_success "Compiled all-in-one [${all_in_one_mode}] binaries for ${arch}"
}

# =============================================================================
# All-in-One: Build a single all-in-one Docker image
# =============================================================================

build_all_in_one_image() {
    local arch=$1
    local push=$2
    local version=$3
    local all_in_one_mode=$4   # "controller-gateway" or "center"

    local arch_info
    arch_info=$(get_arch_info "${arch}")
    IFS=':' read -r platform target suffix <<< "${arch_info}"
    local bin_path="target/${target}/release"

    ensure_buildx

    local image_name
    image_name=$(all_in_one_image_name "${all_in_one_mode}")

    local image_base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/${image_name}"
    local image_tag="${image_base}:${version}_${suffix}"

    log_info "Building all-in-one [${all_in_one_mode}] image for ${arch}..."
    log_info "  Tag: ${image_tag}"

    local build_cmd=(
        docker buildx build
        --file docker/Dockerfile.all-in-one
        --build-arg "ALL_IN_ONE_MODE=${all_in_one_mode}"
        --build-arg "BINARY_PATH=${bin_path}"
        --build-arg "DASHBOARD_DIST_PATH=docker/.all-in-one-dist"
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
        log_error "Failed to build all-in-one [${all_in_one_mode}] image for ${arch}"
        exit 1
    fi

    log_success "Built ${image_name}:${version}_${suffix}"
    BUILD_INFO+=("all-in-one-${all_in_one_mode}|${image_tag}|${arch}")
}

# =============================================================================
# All-in-One: Merge multi-architecture manifest for a single all-in-one image
# =============================================================================

merge_all_in_one_manifest() {
    local version=$1
    local all_in_one_mode=$2
    shift 2
    local archs=("$@")

    local version_no_v="${version#v}"
    local major="" minor=""

    if [[ "${version_no_v}" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then
        major=$(echo "${version_no_v}" | cut -d. -f1)
        minor=$(echo "${version_no_v}" | cut -d. -f1-2)
    fi

    local image_name
    image_name=$(all_in_one_image_name "${all_in_one_mode}")

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
# All-in-One: Print summary for one all-in-one image
# =============================================================================

print_all_in_one_summary() {
    local version_no_v=$1
    local push=$2
    local all_in_one_mode=$3
    shift 3
    local archs=("$@")

    local image_name
    image_name=$(all_in_one_image_name "${all_in_one_mode}")

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
    local with_examples=false
    local all_in_one_controller_gateway=false
    local all_in_one_center=false
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
            --with-examples|--with-example)
                with_examples=true
                shift
                ;;
            --all-in-one-controller-gateway)
                all_in_one_controller_gateway=true
                shift
                ;;
            --all-in-one-center)
                all_in_one_center=true
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

    # Derived flag: any all-in-one mode requested?
    local any_all_in_one=false
    if [[ "${all_in_one_controller_gateway}" == "true" ]] || [[ "${all_in_one_center}" == "true" ]]; then
        any_all_in_one=true
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
            log_error "  git tag v0.x.0 && ./build-image.sh --from-tag --push"
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
    if [[ "${any_all_in_one}" == "true" ]]; then
        log_info "Mode: All-in-One (standalone images)"
        [[ "${all_in_one_controller_gateway}" == "true" ]] && log_info "  → edgion-all-in-one (dashboard + controller + gateway)"
        [[ "${all_in_one_center}" == "true" ]]             && log_info "  → edgion-all-in-one-center (dashboard + center)"
    else
        log_info "Mode: Standard"
        [[ "${with_examples}" == "true" ]] && log_info "  → With examples"
    fi
    echo ""

    check_prerequisites

    BUILD_INFO=()

    # =========================================================================
    # All-in-One path — completely separate from standard
    # =========================================================================
    if [[ "${any_all_in_one}" == "true" ]]; then

        # --- Compile backend binaries ---
        # "both" compiles all four binaries in a single cargo invocation;
        # single mode compiles only what's needed.
        local compile_mode=""
        if [[ "${all_in_one_controller_gateway}" == "true" ]] && [[ "${all_in_one_center}" == "true" ]]; then
            compile_mode="both"
        elif [[ "${all_in_one_controller_gateway}" == "true" ]]; then
            compile_mode="controller-gateway"
        else
            compile_mode="center"
        fi

        log_stage "All-in-One: Compiling backend binaries (${compile_mode})"
        for arch in "${archs[@]}"; do
            compile_all_in_one_binaries "${arch}" "${rebuild}" "${compile_mode}"
        done

        # --- Build frontend ---
        compile_frontend

        if [[ "${compile_only}" == "true" ]]; then
            log_success "All-in-one compilation completed (--compile-only)"
            rm -rf "${PROJECT_DIR}/docker/.all-in-one-dist"
            exit 0
        fi

        # --- Build all-in-one Docker images ---
        log_stage "All-in-One: Building Docker images"
        for arch in "${archs[@]}"; do
            [[ "${all_in_one_controller_gateway}" == "true" ]] && build_all_in_one_image "${arch}" "${push}" "${version}" "controller-gateway"
            [[ "${all_in_one_center}" == "true" ]]             && build_all_in_one_image "${arch}" "${push}" "${version}" "center"
        done

        # --- Merge manifests ---
        if [[ "${push}" == "true" ]] && [[ ${#archs[@]} -gt 1 ]]; then
            log_stage "All-in-One: Creating multi-architecture manifests"
            [[ "${all_in_one_controller_gateway}" == "true" ]] && merge_all_in_one_manifest "${version}" "controller-gateway" "${archs[@]}"
            [[ "${all_in_one_center}" == "true" ]]             && merge_all_in_one_manifest "${version}" "center" "${archs[@]}"
        elif [[ "${push}" == "true" ]]; then
            log_info "Single architecture, creating simple tags..."
            local version_no_v="${version#v}"
            local arch_info
            arch_info=$(get_arch_info "${archs[0]}")
            IFS=':' read -r _ _ suffix <<< "${arch_info}"

            for all_in_one_mode in controller-gateway center; do
                local flag_var="all_in_one_${all_in_one_mode//-/_}"
                [[ "${!flag_var}" != "true" ]] && continue

                local image_name
                image_name=$(all_in_one_image_name "${all_in_one_mode}")
                local base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/${image_name}"
                docker buildx imagetools create \
                    -t "${base}:${version_no_v}" \
                    -t "${base}:latest" \
                    "${base}:${version}_${suffix}"
            done
        fi

        # --- Summary ---
        echo ""
        log_success "All-in-one build completed successfully!"
        echo ""
        log_info "Built all-in-one images:"
        local version_no_v="${version#v}"
        [[ "${all_in_one_controller_gateway}" == "true" ]] && print_all_in_one_summary "${version_no_v}" "${push}" "controller-gateway" "${archs[@]}"
        [[ "${all_in_one_center}" == "true" ]]             && print_all_in_one_summary "${version_no_v}" "${push}" "center" "${archs[@]}"

        # Cleanup temporary frontend dist
        rm -rf "${PROJECT_DIR}/docker/.all-in-one-dist"
        echo ""
        return 0
    fi

    # =========================================================================
    # Standard path — gateway + controller (+ optional examples)
    # =========================================================================

    # Stage 2: Compile binaries
    log_stage "Stage 2: Compiling binaries"
    for arch in "${archs[@]}"; do
        compile_binaries "${arch}" "${rebuild}"
    done

    # Stage 2b: Compile examples (only with --with-examples)
    if [[ "${with_examples}" == "true" ]]; then
        log_stage "Stage 2b: Compiling examples"
        for arch in "${archs[@]}"; do
            compile_examples "${arch}" "${rebuild}"
        done
    fi

    if [[ "${compile_only}" == "true" ]]; then
        log_success "Compilation completed (--compile-only)"
        exit 0
    fi

    # Stage 3: Build images
    log_stage "Stage 3: Building Docker images"
    for arch in "${archs[@]}"; do
        build_images "${arch}" "${push}" "${version}"
    done

    # Stage 3b: Build example images (only with --with-examples)
    if [[ "${with_examples}" == "true" ]]; then
        log_stage "Stage 3b: Building example Docker images"
        for arch in "${archs[@]}"; do
            build_example_images "${arch}" "${push}" "${version}"
        done
    fi

    # Stage 4: Merge manifests (only when pushing)
    if [[ "${push}" == "true" ]] && [[ ${#archs[@]} -gt 1 ]]; then
        merge_manifests "${version}" "${archs[@]}"
        if [[ "${with_examples}" == "true" ]]; then
            merge_example_manifests "${version}" "${archs[@]}"
        fi
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

        # Handle examples for single architecture push
        if [[ "${with_examples}" == "true" ]]; then
            for example in ${EXAMPLES}; do
                local image_name="${example//_/-}"
                local base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}"

                docker buildx imagetools create \
                    -t "${base}:${version_no_v}" \
                    -t "${base}:latest" \
                    "${base}:${version}_${suffix}"
            done
        fi
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

    # Show example images if built
    if [[ "${with_examples}" == "true" ]]; then
        echo ""
        log_info "Built example images:"
        for example in ${EXAMPLES}; do
            local image_name="${example//_/-}"
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
    fi
    echo ""
}

main "$@"
