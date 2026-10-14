#!/bin/bash
# Explicitly regenerate src/proto_gen/test.rs from the annotated test schema.
# Usage: ./edgion-tests/integration/scripts/proto/generate_test_proto.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
cd "$PROJECT_ROOT"

cargo run -p edgion-tests --bin build_test_proto

echo "Generation complete: edgion-tests/src/proto_gen/test.rs"
