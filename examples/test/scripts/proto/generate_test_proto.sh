#!/bin/bash
# Generate test_service proto code for examples
#
# This script runs the build_test_proto binary to generate Rust code
# from test_service.proto and places it in proto_gen/
#
# Usage:
#   ./examples/test/scripts/proto/generate_test_proto.sh

set -e

# Determine script directory and project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

cd "$PROJECT_ROOT"

echo "=========================================="
echo "Generating test_service proto code"
echo "=========================================="
echo ""

# Run the build_test_proto example
cargo run --example build_test_proto

echo ""
echo "=========================================="
echo "Generation complete!"
echo "=========================================="
echo ""
echo "Generated file: examples/code/proto_gen/test.rs"
echo ""
