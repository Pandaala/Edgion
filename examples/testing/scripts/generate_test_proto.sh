#!/bin/bash
# Generate test_service proto code for examples
#
# This script runs the build_test_proto binary to generate Rust code
# from test_service.proto and places it in proto_gen/
#
# Usage:
#   ./examples/testing/scripts/generate_test_proto.sh
#
# Or from project root:
#   cd examples/testing && ./scripts/generate_test_proto.sh

set -e

# Determine script directory and project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

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
echo "Generated file: examples/testing/proto_gen/test.rs"
echo ""
echo "Next steps:"
echo "  1. Review the generated code"
echo "  2. Commit it to git: git add examples/testing/proto_gen/test.rs"
echo "  3. Proto code is now ready for use in examples"
echo ""

