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
        [$ids[] | {body: {stringValue: ({request_info: {"x-trace-id": .}} | tojson)}}]
    }]}]}' > "${test_tmp}/records.json"
}
write_batch '["warmup", "otlp-shutdown-1", "otlp-shutdown-1", "otlp-shutdown-6"]'
[ "$(count_shutdown_records "${test_tmp}/records.json")" -eq 1 ]
write_batch '["otlp-shutdown-1", "otlp-shutdown-2", "otlp-shutdown-3", "otlp-shutdown-4", "otlp-shutdown-5"]'
[ "$(count_shutdown_records "${test_tmp}/records.json")" -eq 5 ]
[ "$(count_shutdown_records "${test_tmp}/missing")" -eq 0 ]
echo "PASS: unique expected markers, warmup exclusion, and incomplete delivery"
