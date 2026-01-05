#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CONF_DIR="$PROJECT_ROOT/examples/conf"
TEST_DATA_DIR="$SCRIPT_DIR/ctl_test_data"
RUNTIME_DIR="$SCRIPT_DIR/runtime/ctl_test"
API_PORT=5800
CONTROLLER_BIN="$PROJECT_ROOT/target/debug/edgion-controller"
CTL_BIN="$PROJECT_ROOT/target/debug/edgion-ctl"
CONTROLLER_PID=""

# Test counters
TESTS_PASSED=0
TESTS_FAILED=0
TOTAL_TESTS=0

# ============================================================
# Helper Functions
# ============================================================

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_test() {
    echo -e "${YELLOW}[TEST $1/$TOTAL_TESTS]${NC} $2"
}

# Verify file exists
assert_file_exists() {
    local file=$1
    local desc=${2:-"File exists"}
    if [ -f "$file" ]; then
        echo "  ✓ $desc: $file"
        return 0
    else
        echo "  ✗ $desc: $file (NOT FOUND)"
        return 1
    fi
}

# Verify file does not exist
assert_file_not_exists() {
    local file=$1
    local desc=${2:-"File deleted"}
    if [ ! -f "$file" ]; then
        echo "  ✓ $desc: $file"
        return 0
    else
        echo "  ✗ $desc: $file (STILL EXISTS)"
        return 1
    fi
}

# Verify CLI output contains string
assert_output_contains() {
    local output=$1
    local expected=$2
    local desc=${3:-"Output contains"}
    if echo "$output" | grep -q "$expected"; then
        echo "  ✓ $desc: '$expected'"
        return 0
    else
        echo "  ✗ $desc: '$expected' (NOT FOUND)"
        echo "  Actual output: $output"
        return 1
    fi
}

# Verify command succeeded
assert_success() {
    local exit_code=$1
    local desc=${2:-"Command succeeded"}
    if [ $exit_code -eq 0 ]; then
        echo "  ✓ $desc"
        return 0
    else
        echo "  ✗ $desc (EXIT CODE: $exit_code)"
        return 1
    fi
}

# Run test and track results
run_test() {
    local test_name=$1
    shift
    local test_func=$@
    
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    log_test $TOTAL_TESTS "$test_name"
    
    if $test_func; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        return 0
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        return 1
    fi
}

# Wait for API to be ready
wait_for_api() {
    local max_attempts=30
    local attempt=0
    
    log_info "Waiting for Controller API to be ready..."
    
    while [ $attempt -lt $max_attempts ]; do
        if curl -s "http://localhost:$API_PORT/health" > /dev/null 2>&1; then
            log_info "API is ready!"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done
    
    log_error "API failed to start after $max_attempts seconds"
    return 1
}

# Cleanup function
cleanup() {
    log_info "Cleaning up..."
    
    if [ -n "$CONTROLLER_PID" ] && kill -0 "$CONTROLLER_PID" 2>/dev/null; then
        log_info "Stopping controller (PID: $CONTROLLER_PID)..."
        kill "$CONTROLLER_PID" 2>/dev/null || true
        wait "$CONTROLLER_PID" 2>/dev/null || true
    fi
    
    # Clean up runtime directory
    if [ -d "$RUNTIME_DIR" ]; then
        rm -rf "$RUNTIME_DIR"
    fi
}

# Set trap for cleanup
trap cleanup EXIT INT TERM

# ============================================================
# Test Cases
# ============================================================

test_create_resource() {
    log_info "Creating HTTPRoute resource..."
    
    local output
    output=$($CTL_BIN --server "http://localhost:$API_PORT" apply -f "$CONF_DIR/HTTPRoute_edge_test-http.yaml" 2>&1)
    local exit_code=$?
    
    assert_success $exit_code "Apply command" || return 1
    assert_output_contains "$output" "created" "CLI output" || return 1
    assert_file_exists "$RUNTIME_DIR/httproute_edge_test-http.yaml" "Storage file" || return 1
    
    # Verify file content contains the resource (stored as JSON)
    if grep -q '"name":"test-http"' "$RUNTIME_DIR/httproute_edge_test-http.yaml"; then
        echo "  ✓ File content correct"
    else
        echo "  ✗ File content incorrect"
        return 1
    fi
    
    return 0
}

test_list_resources() {
    log_info "Listing HTTPRoute resources..."
    
    local output
    output=$($CTL_BIN --server "http://localhost:$API_PORT" get httproute 2>&1)
    local exit_code=$?
    
    assert_success $exit_code "Get command" || return 1
    assert_output_contains "$output" "test-http" "Resource in list" || return 1
    
    return 0
}

test_get_single_resource() {
    log_info "Getting single HTTPRoute resource..."
    
    local output
    output=$($CTL_BIN --server "http://localhost:$API_PORT" get httproute test-http -n edge -o yaml 2>&1)
    local exit_code=$?
    
    assert_success $exit_code "Get command" || return 1
    assert_output_contains "$output" "name: test-http" "Resource name" || return 1
    assert_output_contains "$output" "/api" "Path prefix" || return 1
    
    return 0
}

test_update_resource() {
    log_info "Updating HTTPRoute resource..."
    
    # Create updated version by modifying the original
    local temp_file="$RUNTIME_DIR/httproute_updated_temp.yaml"
    sed 's|value: /api|value: /updated|g; s|port: 30001|port: 9090|g' \
        "$CONF_DIR/HTTPRoute_edge_test-http.yaml" > "$temp_file"
    
    local output
    output=$($CTL_BIN --server "http://localhost:$API_PORT" apply -f "$temp_file" 2>&1)
    local exit_code=$?
    
    assert_success $exit_code "Apply command" || { rm -f "$temp_file"; return 1; }
    assert_output_contains "$output" "updated" "CLI output" || { rm -f "$temp_file"; return 1; }
    
    # Verify file content updated (stored as JSON)
    if grep -q '/updated' "$RUNTIME_DIR/httproute_edge_test-http.yaml"; then
        echo "  ✓ File content updated"
    else
        echo "  ✗ File content not updated"
        rm -f "$temp_file"
        return 1
    fi
    
    # Verify get returns new content
    local get_output
    get_output=$($CTL_BIN --server "http://localhost:$API_PORT" get httproute test-http -n edge -o yaml 2>&1)
    assert_output_contains "$get_output" "/updated" "Updated content" || { rm -f "$temp_file"; return 1; }
    
    rm -f "$temp_file"
    return 0
}

test_delete_resource() {
    log_info "Deleting HTTPRoute resource..."
    
    local output
    output=$($CTL_BIN --server "http://localhost:$API_PORT" delete httproute test-http -n edge 2>&1)
    local exit_code=$?
    
    assert_success $exit_code "Delete command" || return 1
    assert_output_contains "$output" "deleted" "CLI output" || return 1
    assert_file_not_exists "$RUNTIME_DIR/httproute_edge_test-http.yaml" "Storage file" || return 1
    
    # Verify get returns error
    if $CTL_BIN --server "http://localhost:$API_PORT" get httproute test-http -n edge 2>/dev/null; then
        echo "  ✗ Resource still exists after deletion"
        return 1
    else
        echo "  ✓ Resource not found (as expected)"
    fi
    
    return 0
}

test_batch_apply() {
    log_info "Applying multiple resources from examples/conf..."
    
    # Apply Service first
    local output1
    output1=$($CTL_BIN --server "http://localhost:$API_PORT" apply -f "$CONF_DIR/Service_edge_test-http.yaml" 2>&1)
    assert_success $? "Apply Service" || return 1
    assert_file_exists "$RUNTIME_DIR/service_edge_test-http.yaml" "Service file" || return 1
    
    # Apply EndpointSlice
    local output2
    output2=$($CTL_BIN --server "http://localhost:$API_PORT" apply -f "$CONF_DIR/EndpointSlice_edge_test-http.yaml" 2>&1)
    assert_success $? "Apply EndpointSlice" || return 1
    assert_file_exists "$RUNTIME_DIR/endpointslice_edge_test-http.yaml" "EndpointSlice file" || return 1
    
    # Apply HTTPRoute (already exists from previous test, should update)
    local output3
    output3=$($CTL_BIN --server "http://localhost:$API_PORT" apply -f "$CONF_DIR/HTTPRoute_edge_test-http.yaml" 2>&1)
    assert_success $? "Apply HTTPRoute" || return 1
    
    echo "  ✓ Multiple resources applied successfully"
    
    return 0
}

test_reload() {
    log_info "Testing reload functionality..."
    
    # Manually modify a file in storage
    local file="$RUNTIME_DIR/httproute_edge_test-http.yaml"
    if [ -f "$file" ]; then
        sed -i.bak 's|/api|/api-reloaded|g' "$file"
        echo "  ✓ Modified file in storage"
    else
        echo "  ✗ File not found for modification"
        return 1
    fi
    
    # Call reload
    local output
    output=$($CTL_BIN --server "http://localhost:$API_PORT" reload 2>&1)
    local exit_code=$?
    
    assert_success $exit_code "Reload command" || return 1
    assert_output_contains "$output" "reload" "CLI output" || return 1
    
    # Verify get returns modified content
    sleep 1  # Give a moment for reload to complete
    local get_output
    get_output=$($CTL_BIN --server "http://localhost:$API_PORT" get httproute test-http -n edge -o yaml 2>&1)
    assert_output_contains "$get_output" "/api-reloaded" "Modified content" || return 1
    
    return 0
}

test_cluster_scoped() {
    log_info "Testing cluster-scoped resource (EdgionGatewayConfig)..."
    
    # The base EdgionGatewayConfig already exists, test by getting it
    local list_output
    list_output=$($CTL_BIN --server "http://localhost:$API_PORT" get edgiongwconfig 2>&1)
    local exit_code=$?
    
    assert_success $exit_code "Get EdgionGatewayConfig" || return 1
    assert_output_contains "$list_output" "example-gateway" "Resource in list" || return 1
    
    # Test updating it
    local output
    output=$($CTL_BIN --server "http://localhost:$API_PORT" apply -f "$CONF_DIR/EdgionGatewayConfig__example-gateway.yaml" 2>&1)
    assert_success $? "Apply (update) command" || return 1
    assert_output_contains "$output" "updated" "CLI output contains 'updated'" || return 1
    
    echo "  ✓ Cluster-scoped resource operations successful"
    
    return 0
}

# ============================================================
# Main Test Flow
# ============================================================

main() {
    echo "========================================"
    echo "Edgion-ctl Integration Test"
    echo "========================================"
    echo ""
    
    # Count total tests
    TOTAL_TESTS=8
    
    # Step 1: Build binaries
    log_info "[1/9] Building binaries..."
    cd "$PROJECT_ROOT"
    if ! cargo build --bin edgion-controller --bin edgion-ctl > /dev/null 2>&1; then
        log_error "Failed to build binaries"
        exit 1
    fi
    echo "  ✓ Built edgion-controller"
    echo "  ✓ Built edgion-ctl"
    echo ""
    
    # Step 2: Clean and prepare runtime directory
    log_info "[2/9] Preparing test environment..."
    rm -rf "$RUNTIME_DIR"
    mkdir -p "$RUNTIME_DIR"
    
    # Copy base configuration files from examples/conf
    cp "$CONF_DIR/GatewayClass__public-gateway.yaml" "$RUNTIME_DIR/"
    cp "$CONF_DIR/EdgionGatewayConfig__example-gateway.yaml" "$RUNTIME_DIR/"
    cp "$CONF_DIR/Gateway_edge_example-gateway.yaml" "$RUNTIME_DIR/"
    
    echo "  ✓ Runtime directory prepared"
    echo "  ✓ Base configuration files copied from examples/conf"
    echo ""
    
    # Step 3: Start controller
    log_info "[3/9] Starting controller..."
    cd "$PROJECT_ROOT"
    # Use main config file with CLI overrides for testing
    $CONTROLLER_BIN -c config/edgion-controller.toml \
        --conf-dir "$RUNTIME_DIR" \
        --admin-listen "0.0.0.0:5800" \
        > "$SCRIPT_DIR/logs/ctl_test_controller.log" 2>&1 &
    CONTROLLER_PID=$!
    echo "  ✓ Controller started (PID: $CONTROLLER_PID)"
    
    if ! wait_for_api; then
        log_error "Controller failed to start"
        exit 1
    fi
    echo ""
    
    # Reset test counters
    TESTS_PASSED=0
    TESTS_FAILED=0
    
    # Run tests
    log_info "[4/9] Running tests..."
    echo ""
    
    run_test "Create resource (apply)" test_create_resource
    echo ""
    
    run_test "List resources (get)" test_list_resources
    echo ""
    
    run_test "Get single resource" test_get_single_resource
    echo ""
    
    run_test "Update resource (apply)" test_update_resource
    echo ""
    
    run_test "Delete resource" test_delete_resource
    echo ""
    
    run_test "Batch apply directory" test_batch_apply
    echo ""
    
    run_test "Reload configuration" test_reload
    echo ""
    
    run_test "Cluster-scoped resource" test_cluster_scoped
    echo ""
    
    # Summary
    echo "========================================"
    echo "Test Summary"
    echo "========================================"
    echo "Total tests: $TOTAL_TESTS"
    echo "Passed: $TESTS_PASSED"
    echo "Failed: $TESTS_FAILED"
    echo ""
    
    if [ $TESTS_FAILED -eq 0 ]; then
        echo -e "${GREEN}All tests passed! ✓${NC}"
        echo "========================================"
        return 0
    else
        echo -e "${RED}Some tests failed! ✗${NC}"
        echo "========================================"
        return 1
    fi
}

# Run main function
main
exit $?

