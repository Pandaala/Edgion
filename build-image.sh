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
    --platforms <platforms>  Comma-separated list of platforms (default: linux/amd64)
    --push                   Push image to registry
    --no-cache               Build without cache
    --test                   Run tests after build
    --report <file>          Generate build report
    -h, --help               Show this help message

Environment Variables:
    IMAGE_REGISTRY          Docker registry (default: docker.io)
    IMAGE_NAMESPACE         Image namespace (default: pandaala)
    VERSION                 Image version (auto: git tag > "unknown")
    RUST_VERSION            Rust version (default: 1.92)
    FEATURES                Cargo features (default: default)

Examples:
    $0 gateway
    $0 controller --push
    $0 all --platforms linux/amd64,linux/arm64 --push
    $0 test-server --push
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
    
    # Check Dockerfile
    if [[ ! -f "${PROJECT_DIR}/docker/Dockerfile" ]]; then
        log_error "Dockerfile not found in ${PROJECT_DIR}/docker"
        exit 1
    fi
    
    log_success "Prerequisites check passed"
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
    
    # Parse options
    while [[ $# -gt 0 ]]; do
        case $1 in
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
    
    # Build info array for report
    BUILD_INFO=()
    
    # Build images
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
    
    # Generate report if requested
    if [[ -n "${report_file}" ]]; then
        generate_report "${report_file}"
    fi
    
    log_success "Build completed successfully!"
}

main "$@"

