#!/usr/bin/env bash

# Edgion Docker Image Build Script
# 
# This script provides advanced build functionality including:
# - Pre-build validation
# - Multi-architecture builds
# - Post-build testing
# - Build reporting
#
# Usage:
#   ./scripts/build-image.sh gateway
#   ./scripts/build-image.sh controller --platforms linux/amd64,linux/arm64
#   ./scripts/build-image.sh all --push

set -euo pipefail

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="${SCRIPT_DIR}"

# Auto-detect version from git
# Priority: 1. ENV var 2. git tag 3. "unknown"
if [[ -z "${VERSION:-}" ]]; then
    if git rev-parse --git-dir > /dev/null 2>&1; then
        # Try to get tag for current commit
        GIT_TAG=$(git describe --tags --exact-match 2>/dev/null || echo "")
        if [[ -n "${GIT_TAG}" ]]; then
            VERSION="${GIT_TAG#v}"  # Remove 'v' prefix if exists
        else
            VERSION="unknown"
        fi
    else
        VERSION="unknown"
    fi
fi

IMAGE_REGISTRY="${IMAGE_REGISTRY:-docker.io}"
IMAGE_NAMESPACE="${IMAGE_NAMESPACE:-pandaala}"
RUST_VERSION="${RUST_VERSION:-1.92}"
FEATURES="${FEATURES:-default}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $*"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $*"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $*"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $*"
}

show_usage() {
    cat << EOF
Edgion Docker Image Build Script

Usage: $0 <target> [options]

Targets:
    gateway         Build Gateway image
    controller      Build Controller image
    ctl             Build CLI tool image
    test-server     Build test server image (example)
    test-client     Build test client image (example)
    all             Build all images (including test)

Options:
    --rebuild                Force rebuild local binaries
    --platforms <platforms>  Comma-separated list of platforms (default: linux/amd64)
                             Multi-platform builds will use Docker buildx
    --push                   Push image to registry
    --no-cache               Build without cache (Docker build only)
    --test                   Run tests after build
    --report <file>          Generate build report
    -h, --help               Show this help message

Build Mode (automatic):
    Local Build (default)    Fast! Compiles once, uses Dockerfile.runtime
                             Requires: cargo installed
    Docker Build (fallback)  Used when: multi-platform or no cargo available

Environment Variables:
    IMAGE_REGISTRY          Docker registry (default: docker.io)
    IMAGE_NAMESPACE         Image namespace (default: pandaala)
    VERSION                 Image version (auto: git tag > "unknown")
    RUST_VERSION            Rust version (default: 1.92)
    FEATURES                Cargo features (default: default)

Examples:
    # Local development (auto local build - FAST!)
    $0 gateway                        # Build single image
    $0 all --push                     # Build & push all images
    $0 all --rebuild --push           # Force recompile & build all
    
    # Multi-platform build (auto Docker buildx)
    $0 all --platforms linux/amd64,linux/arm64 --push
    
    # Testing
    $0 gateway --test --report build-report.txt

EOF
}

check_prerequisites() {
    log_info "Checking prerequisites..."
    
    # Check Docker
    if ! command -v docker &> /dev/null; then
        log_error "Docker not found. Please install Docker."
        exit 1
    fi
    
    # Check Docker daemon
    if ! docker info &> /dev/null; then
        log_error "Docker daemon not running."
        exit 1
    fi
    
    # Check Dockerfiles
    if [[ ! -f "${PROJECT_DIR}/docker/Dockerfile" ]]; then
        log_error "Dockerfile not found in ${PROJECT_DIR}/docker"
        exit 1
    fi
    
    if [[ ! -f "${PROJECT_DIR}/docker/Dockerfile.runtime" ]]; then
        log_error "Dockerfile.runtime not found in ${PROJECT_DIR}/docker"
        exit 1
    fi
    
    log_success "Prerequisites check passed"
}

determine_build_mode() {
    local platforms=$1
    
    # Check if multi-platform build
    if [[ "${platforms}" == *","* ]]; then
        log_info "Multi-platform build detected, using Docker buildx" >&2
        echo "docker"
        return
    fi
    
    # Check if cargo is available
    if ! command -v cargo &> /dev/null; then
        log_warning "Cargo not found, falling back to Docker build" >&2
        log_warning "For faster builds, install Rust: https://rustup.rs/" >&2
        echo "docker"
        return
    fi
    
    # Use local build (fastest)
    log_info "Using local build mode (fast!)" >&2
    echo "local"
}

compile_local_binaries() {
    local force_rebuild=${1:-false}
    
    log_info "Checking local binaries..."
    
    # Check if binaries exist
    local bins_exist=true
    if [[ ! -f "${PROJECT_DIR}/target/release/edgion-gateway" ]] || \
       [[ ! -f "${PROJECT_DIR}/target/release/edgion-controller" ]] || \
       [[ ! -f "${PROJECT_DIR}/target/release/edgion-ctl" ]] || \
       [[ ! -f "${PROJECT_DIR}/target/release/examples/test_server" ]] || \
       [[ ! -f "${PROJECT_DIR}/target/release/examples/test_client" ]]; then
        bins_exist=false
    fi
    
    if [[ "${bins_exist}" == "false" ]] || [[ "${force_rebuild}" == "true" ]]; then
        log_info "Compiling all binaries locally..."
        log_info "This may take a few minutes on first build..."
        
        cd "${PROJECT_DIR}"
        cargo build --release \
            --bin edgion-gateway \
            --bin edgion-controller \
            --bin edgion-ctl \
            --example test_server \
            --example test_client \
            --features "${FEATURES}"
        
        if [[ $? -ne 0 ]]; then
            log_error "Local compilation failed"
            exit 1
        fi
        
        log_success "Local binaries compiled successfully"
    else
        log_success "Local binaries already exist (use --rebuild to recompile)"
    fi
}

build_image_local() {
    local binary=$1
    local push=$2
    local build_type=$3
    
    local image_name="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${binary}"
    local image_tag="${image_name}:${VERSION}"
    local image_latest="${image_name}:latest"
    
    log_info "Building ${binary} image (local mode, type: ${build_type})..."
    log_info "  Image: ${image_tag}"
    log_info "  Using: Dockerfile.runtime"
    
    # Determine binary name for BUILD_TYPE
    local binary_arg
    if [[ "${build_type}" == "example" ]]; then
        binary_arg="${binary//-/_}"  # test-server -> test_server
    else
        binary_arg="${binary}"
    fi
    
    docker build \
        -f "${PROJECT_DIR}/docker/Dockerfile.runtime" \
        --build-arg "BINARY=${binary_arg}" \
        --build-arg "BUILD_TYPE=${build_type}" \
        --build-arg "BINARY_PATH=target/release" \
        -t "${image_tag}" \
        -t "${image_latest}" \
        "${PROJECT_DIR}"
    
    if [[ $? -ne 0 ]]; then
        log_error "Failed to build ${binary} image"
        return 1
    fi
    
    log_success "${binary} image built successfully"
    
    if [[ "${push}" == "true" ]]; then
        log_info "Pushing ${binary} image..."
        docker push "${image_tag}"
        docker push "${image_latest}"
        log_success "${binary} image pushed"
    fi
    
    # Store build info for report
    BUILD_INFO+=("${binary}|${image_tag}|local")
}

build_image() {
    local binary=$1
    local platforms=${2:-linux/amd64}
    local push=${3:-false}
    local no_cache=${4:-false}
    local build_type=${5:-bin}
    
    local image_name="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${binary}"
    local image_tag="${image_name}:${VERSION}"
    local image_latest="${image_name}:latest"
    
    log_info "Building ${binary} image (type: ${build_type})..."
    log_info "  Image: ${image_tag}"
    log_info "  Platforms: ${platforms}"
    log_info "  Push: ${push}"
    
    # Determine binary name based on build type
    local binary_arg
    if [[ "${build_type}" == "example" ]]; then
        binary_arg="${binary}"
    else
        binary_arg="edgion-${binary}"
    fi
    
    local build_args=(
        --build-arg "BINARY=${binary_arg}"
        --build-arg "BUILD_TYPE=${build_type}"
        --build-arg "RUST_VERSION=${RUST_VERSION}"
        --build-arg "FEATURES=${FEATURES}"
        -t "${image_tag}"
        -t "${image_latest}"
        -f "${PROJECT_DIR}/docker/Dockerfile"
    )
    
    if [[ "${no_cache}" == "true" ]]; then
        build_args+=(--no-cache)
    fi
    
    # Use buildx for multi-platform builds
    if [[ "${platforms}" == *","* ]] || [[ "${push}" == "true" ]]; then
        log_info "Using Docker Buildx for multi-platform build..."
        
        # Create builder if needed
        if ! docker buildx inspect edgion-builder &> /dev/null; then
            docker buildx create --name edgion-builder --use
        else
            docker buildx use edgion-builder
        fi
        
        build_args+=(
            --platform "${platforms}"
        )
        
        if [[ "${push}" == "true" ]]; then
            build_args+=(--push)
        else
            build_args+=(--load)
        fi
        
        docker buildx build "${build_args[@]}" "${PROJECT_DIR}"
    else
        # Standard build
        docker build "${build_args[@]}" "${PROJECT_DIR}"
        
        if [[ "${push}" == "true" ]]; then
            log_info "Pushing image..."
            docker push "${image_tag}"
            docker push "${image_latest}"
        fi
    fi
    
    log_success "${binary} image built successfully"
    
    # Store build info for report
    BUILD_INFO+=("${binary}|${image_tag}|${platforms}")
}

test_image() {
    local binary=$1
    local image_name="${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}/edgion-${binary}:${VERSION}"
    
    log_info "Testing ${binary} image..."
    
    # Test 1: Image exists
    if ! docker images -q "${image_name}" &> /dev/null; then
        log_error "Image not found: ${image_name}"
        return 1
    fi
    
    # Test 2: Binary exists in image
    if ! docker run --rm "${image_name}" sh -c "test -f /usr/local/bin/edgion-${binary}"; then
        log_error "Binary not found in image"
        return 1
    fi
    
    # Test 3: Binary is executable
    if ! docker run --rm "${image_name}" sh -c "test -x /usr/local/bin/edgion-${binary}"; then
        log_error "Binary is not executable"
        return 1
    fi
    
    # Test 4: Try to run --help (may fail for some binaries, so we don't exit on error)
    log_info "Testing binary execution..."
    docker run --rm "${image_name}" /usr/local/bin/edgion-${binary} --help > /dev/null 2>&1 || true
    
    log_success "${binary} image tests passed"
}

generate_report() {
    local report_file=$1
    
    log_info "Generating build report: ${report_file}"
    
    {
        echo "======================================"
        echo "Edgion Docker Image Build Report"
        echo "======================================"
        echo ""
        echo "Build Date: $(date)"
        echo "Version: ${VERSION}"
        echo "Rust Version: ${RUST_VERSION}"
        echo "Features: ${FEATURES}"
        echo "Registry: ${IMAGE_REGISTRY}/${IMAGE_NAMESPACE}"
        echo ""
        echo "Built Images:"
        echo "--------------------------------------"
        
        for info in "${BUILD_INFO[@]}"; do
            IFS='|' read -r binary tag platforms <<< "${info}"
            echo "  - Binary: edgion-${binary}"
            echo "    Tag: ${tag}"
            echo "    Platforms: ${platforms}"
            echo ""
        done
        
        echo "======================================"
    } > "${report_file}"
    
    log_success "Report generated: ${report_file}"
    cat "${report_file}"
}

# Main
main() {
    if [[ $# -eq 0 ]] || [[ "$1" == "-h" ]] || [[ "$1" == "--help" ]]; then
        show_usage
        exit 0
    fi
    
    local target=$1
    shift
    
    local platforms="linux/amd64"
    local push=false
    local no_cache=false
    local run_tests=false
    local report_file=""
    local force_rebuild=false
    
    # Parse options
    while [[ $# -gt 0 ]]; do
        case $1 in
            --rebuild)
                force_rebuild=true
                shift
                ;;
            --platforms)
                platforms=$2
                shift 2
                ;;
            --push)
                push=true
                shift
                ;;
            --no-cache)
                no_cache=true
                shift
                ;;
            --test)
                run_tests=true
                shift
                ;;
            --report)
                report_file=$2
                shift 2
                ;;
            *)
                log_error "Unknown option: $1"
                show_usage
                exit 1
                ;;
        esac
    done
    
    check_prerequisites
    
    # Determine build mode automatically
    local build_mode=$(determine_build_mode "${platforms}")
    
    # If local build mode, compile binaries first
    if [[ "${build_mode}" == "local" ]]; then
        compile_local_binaries "${force_rebuild}"
    fi
    
    # Build info array for report
    BUILD_INFO=()
    
    # Build images - use local or docker build based on auto-detected mode
    if [[ "${build_mode}" == "local" ]]; then
        # Local build mode: use Dockerfile.runtime (fast!)
        log_info "Build mode: Local (Dockerfile.runtime)"
        case ${target} in
            gateway)
                build_image_local "gateway" "${push}" "bin"
                [[ "${run_tests}" == "true" ]] && test_image "gateway"
                ;;
            controller)
                build_image_local "controller" "${push}" "bin"
                [[ "${run_tests}" == "true" ]] && test_image "controller"
                ;;
            ctl)
                build_image_local "ctl" "${push}" "bin"
                [[ "${run_tests}" == "true" ]] && test_image "ctl"
                ;;
            test-server)
                build_image_local "test-server" "${push}" "example"
                [[ "${run_tests}" == "true" ]] && test_image "test-server"
                ;;
            test-client)
                build_image_local "test-client" "${push}" "example"
                [[ "${run_tests}" == "true" ]] && test_image "test-client"
                ;;
            all)
                build_image_local "gateway" "${push}" "bin"
                build_image_local "controller" "${push}" "bin"
                build_image_local "ctl" "${push}" "bin"
                build_image_local "test-server" "${push}" "example"
                build_image_local "test-client" "${push}" "example"
                
                if [[ "${run_tests}" == "true" ]]; then
                    test_image "gateway"
                    test_image "controller"
                    test_image "ctl"
                    test_image "test-server"
                    test_image "test-client"
                fi
                ;;
            *)
                log_error "Unknown target: ${target}"
                show_usage
                exit 1
                ;;
        esac
    else
        # Docker build mode: use full Dockerfile
        log_info "Build mode: Docker (Dockerfile)"
        case ${target} in
            gateway)
                build_image "gateway" "${platforms}" "${push}" "${no_cache}" "bin"
                [[ "${run_tests}" == "true" ]] && test_image "gateway"
                ;;
            controller)
                build_image "controller" "${platforms}" "${push}" "${no_cache}" "bin"
                [[ "${run_tests}" == "true" ]] && test_image "controller"
                ;;
            ctl)
                build_image "ctl" "${platforms}" "${push}" "${no_cache}" "bin"
                [[ "${run_tests}" == "true" ]] && test_image "ctl"
                ;;
            test-server)
                build_image "test-server" "${platforms}" "${push}" "${no_cache}" "example"
                [[ "${run_tests}" == "true" ]] && test_image "test-server"
                ;;
            test-client)
                build_image "test-client" "${platforms}" "${push}" "${no_cache}" "example"
                [[ "${run_tests}" == "true" ]] && test_image "test-client"
                ;;
            all)
                build_image "gateway" "${platforms}" "${push}" "${no_cache}" "bin"
                build_image "controller" "${platforms}" "${push}" "${no_cache}" "bin"
                build_image "ctl" "${platforms}" "${push}" "${no_cache}" "bin"
                build_image "test-server" "${platforms}" "${push}" "${no_cache}" "example"
                build_image "test-client" "${platforms}" "${push}" "${no_cache}" "example"
                
                if [[ "${run_tests}" == "true" ]]; then
                    test_image "gateway"
                    test_image "controller"
                    test_image "ctl"
                    test_image "test-server"
                    test_image "test-client"
                fi
                ;;
            *)
                log_error "Unknown target: ${target}"
                show_usage
                exit 1
                ;;
        esac
    fi
    
    # Generate report if requested
    if [[ -n "${report_file}" ]]; then
        generate_report "${report_file}"
    fi
    
    log_success "Build completed successfully!"
}

main "$@"

