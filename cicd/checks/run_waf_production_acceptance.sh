#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repository_root"

report_dir="${EDGION_WAF_ACCEPTANCE_REPORT_DIR:-target/waf-production-acceptance}"
mkdir -p "$report_dir"
report_dir="$(cd "$report_dir" && pwd)"
rm -f "$report_dir/acceptance.json" "$report_dir/integration-report.log" "$report_dir/coverage.json"

cargo run --locked -q -p edgion-resources --example waf_production_acceptance -- \
    edgion-waf/acceptance/production-v1.json "$report_dir/coverage.json"

cargo test --locked -p edgion-gateway --lib waf_store_publishes_atomically_and_pins_in_flight_generations

if [[ "${1:-}" == "--offline-only" ]]; then
    exit 0
fi
if [[ $# -ne 0 ]]; then
    echo "usage: $0 [--offline-only]" >&2
    exit 2
fi

./edgion-tests/integration/scripts/integration/run_integration.sh -r Gateway -i WafDynamic

current_file="tmp/integration_testing/.current"
if [[ ! -s "$current_file" ]]; then
    echo "integration work directory pointer is missing" >&2
    exit 1
fi
integration_report="$(cat "$current_file")/report.log"
if [[ ! -s "$integration_report" ]]; then
    echo "integration report is missing" >&2
    exit 1
fi
cp "$integration_report" "$report_dir/integration-report.log"
if ! grep -Eq '^\[PASS\] Dyn_WafDynamic \(' "$integration_report"; then
    echo "WafDynamic did not pass the Gateway integration run" >&2
    exit 1
fi

python3 - "$report_dir/coverage.json" "$report_dir/integration-report.log" "$report_dir/acceptance.json" <<'PY'
import json
import pathlib
import sys

coverage_path, integration_path, acceptance_path = map(pathlib.Path, sys.argv[1:])
coverage = json.loads(coverage_path.read_text(encoding="utf-8"))
integration = integration_path.read_text(encoding="utf-8")
if "Status: PASS" not in integration or "[PASS] Dyn_WafDynamic (" not in integration:
    raise SystemExit("Gateway integration report is not a complete passing run")
acceptance = {
    "schema": "edgion-waf-production-acceptance/v1",
    "coverage": coverage,
    "gateway": {"scenario": "WafDynamic", "passed": True, "report": "integration-report.log"},
}
acceptance_path.write_text(json.dumps(acceptance, indent=2) + "\n", encoding="utf-8")
PY
