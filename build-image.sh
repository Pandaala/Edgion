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
# Configuration
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="${SCRIPT_DIR}"
CONF_ENV_FILE="${CONF_ENV_FILE:-${PROJECT_DIR}/examples/k8stest/scripts/conf.env}"

# Binaries to build
BINARIES="gateway controller"
# Examples to build (only with --with-examples flag)
EXAMPLES="test_server test_client"

# Get architecture info: returns "platform:target:suffix"
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

# Load defaults from conf.env if present.
# Explicit env vars passed by caller still take precedence.
if [[ -f "${CONF_ENV_FILE}" ]]; then
    # shellcheck disable=SC1090
    source "${CONF_ENV_FILE}"
fi

# Respect IMAGE_VERSION from conf/env as VERSION default.
if [[ -z "${VERSION:-}" ]] && [[ -n "${IMAGE_VERSION:-}" ]]; then
    VERSION="${IMAGE_VERSION}"
fi

# Auto-detect version from git tag when VERSION is still unset.
if [[ -z "${VERSION:-}" ]]; then
    if git rev-parse --git-dir > /dev/null 2>&1; then
        GIT_TAG=$(git describe --tags --exact-match 2>/dev/null || echo "")
        if [[ -n "${GIT_TAG}" ]]; then
            VERSION="${GIT_TAG}"
        else
            VERSION="dev1"
        fi
    else
        VERSION="dev1"
    fi
fi

IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
RUST_VERSION="${RUST_VERSION:-1.87}"
FEATURES="${FEATURES:-default}"
BUILDER_IMAGE="edgion-builder"

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
    --arch ARCH         Comma-separated architectures (default: auto-detect)
                        Available: arm64, amd64
    --push              Push images and create multi-arch manifest
    --rebuild           Force rebuild binaries (ignore cache)
    --version TAG       Specify version tag (default: git tag or "dev1")
    --compile-only      Only compile binaries, don't build images
    --with-examples     Also build test_server and test_client images
    --with-example      Alias of --with-examples
    -h, --help          Show this help message

Environment Variables:
    CONF_ENV_FILE       Path to env config file (default: examples/k8stest/scripts/conf.env)
    IMAGE_REGISTRY      Docker registry (default: docker.io)
    IMAGE_NAMESPACE     Image namespace (default: pandaala)
    VERSION             Image version (overrides conf.env and git tag detection)
    RUST_VERSION        Rust version for builder (default: 1.87)
    FEATURES            Cargo features (default: default)

Examples:
    # Build for current architecture only
    $0

    # Build for both arm64 and amd64
    $0 --arch arm64,amd64

    # Build and push multi-arch images
    $0 --arch arm64,amd64 --push

    # Force rebuild and push
    $0 --arch arm64,amd64 --rebuild --push

    # Just compile, don't build images
    $0 --arch arm64 --compile-only

    # Build and push with test_server and test_client
    $0 --arch arm64,amd64 --push --with-examples

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
            --bin edgion-ctl \
            --features \"${FEATURES}\""

    if [[ "${force_rebuild}" == "true" ]]; then
        log_info "Force rebuild requested for ${arch}: cleaning target ${target} first"
        build_cmd="cargo clean --target \"${target}\" && ${build_cmd}"
    fi

    # Run compilation in Docker
    docker run --rm \
        --platform "${platform}" \
        -v "${PROJECT_DIR}":/project \
        -v "${HOME}/.cargo/registry":/usr/local/cargo/registry \
        -v "${HOME}/.cargo/git":/usr/local/cargo/git \
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
        log_info "Force rebuild requested for examples (${arch}): cleaning target ${target} first"
        build_cmd="cargo clean --target \"${target}\" && ${build_cmd}"
    fi

    # Run compilation in Docker
    docker run --rm \
        --platform "${platform}" \
        -v "${PROJECT_DIR}":/project \
        -v "${HOME}/.cargo/registry":/usr/local/cargo/registry \
        -v "${HOME}/.cargo/git":/usr/local/cargo/git \
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

    # Ensure buildx builder exists
        if ! docker buildx inspect edgion-builder &> /dev/null; then
        log_info "Creating Docker Buildx builder..."
            docker buildx create --name edgion-builder --use
        else
            docker buildx use edgion-builder
        fi
        
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

    # Ensure buildx builder exists
    if ! docker buildx inspect edgion-builder &> /dev/null; then
        log_info "Creating Docker Buildx builder..."
        docker buildx create --name edgion-builder --use
    else
        docker buildx use edgion-builder
    fi

    for example in ${EXAMPLES}; do
        # Convert example name to image name: test_server -> edgion-test-server
        local image_name="${example//_/-}"
        local image_base="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${image_name}"
        local image_tag="${image_base}:${version}_${suffix}"
        local runtime_base="debian:bookworm-slim"
        local extra_packages=""

        # test_client image should include debug/test tools.
        if [[ "${example}" == "test_client" ]]; then
            runtime_base="ubuntu:24.04"
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
            --compile-only)
                compile_only=true
                shift
                ;;
            --with-examples|--with-example)
                with_examples=true
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
    log_info "With Examples: ${with_examples}"
    echo ""
    
    check_prerequisites
    
    BUILD_INFO=()
    
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
        # Stage 4b: Merge example manifests (only with --with-examples)
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
