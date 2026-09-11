#!/bin/bash
# Verify the production OTLP record selector with synthetic collector batches.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
RUNNER="${PROJECT_ROOT}/edgion-tests/integration/scripts/integration/link_sys/run_otlp_test.sh"
test_tmp=$(mktemp -d)
trap 'rm -rf "$test_tmp"' EXIT
selector=$(sed -n '/^count_shutdown_records()/,/^}/p' "$RUNNER")
[ -n "$selector" ] || { echo "FAIL: record selector missing" >&2; exit 1; }
eval "$selector"

write_batch() {
    jq -n --argjson ids "$1" '{resourceLogs: [{scopeLogs: [{logRecords:
        [$ids[] | {body: {stringValue: ({request_info: {trace_id: {"x-trace-id": .}}} | tojson)}}]
    }]}]}' > "${test_tmp}/records.json"
}
write_batch '["warmup", "4bf92f3577b34da6a3ce929d0e0e4731", "4bf92f3577b34da6a3ce929d0e0e4731", "4bf92f3577b34da6a3ce929d0e0e4736"]'
[ "$(count_shutdown_records "${test_tmp}/records.json")" -eq 1 ]
write_batch '["4bf92f3577b34da6a3ce929d0e0e4731", "4bf92f3577b34da6a3ce929d0e0e4732", "4bf92f3577b34da6a3ce929d0e0e4733", "4bf92f3577b34da6a3ce929d0e0e4734", "4bf92f3577b34da6a3ce929d0e0e4735"]'
[ "$(count_shutdown_records "${test_tmp}/records.json")" -eq 5 ]
[ "$(count_shutdown_records "${test_tmp}/missing")" -eq 0 ]
echo "PASS: unique expected markers, warmup exclusion, and incomplete delivery"
