#!/usr/bin/env bash
# Ensure every overlay-consumer plugin emits the full fail-to-base signal set.
#
# Background: the EdgionData overlay design (01-design.md §4 fail matrix)
# defines four signals every consumer plugin must emit via its Prometheus
# overlay-event counter:
#
#   overlay_missing         overrideRef set but EdgionData not in store
#   overlay_type_mismatch   EdgionData present but wrong DataEntry variant
#   overlay_invalid         overlay applied but effective config fails validation
#   overlay_disabled        envelope spec.enable=false (H1 top kill switch)
#
# Round-1 review (H1) caught the missing `overlay_disabled` -- envelope kill
# switch existed in design but no plugin emitted it. This gate prevents the
# next plugin from silently skipping one of the four signals.
#
# Currently registered overlay consumers (extend ENTRIES when adding a new one):
#   record_region_route_overlay_event       (RegionRoute HTTP plugin)
#   record_conn_ip_restriction_event        (GlobalConnIpRestriction stream plugin)
#
# Note: record_conn_ip_restriction_event ALSO emits non-overlay labels (denied,
# fail_closed_*); we only require the four overlay labels to be present.
#
# Policy source:
#   tasks/todo/edgion-data-overlay-refactor/01-design.md  -> §4 fail matrix
#   tasks/todo/edgion-data-overlay-refactor/05-issues.md  -> H1 RESOLVED
#
# Exit 0 on clean; exit 1 with the missing-signal list.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

# (recorder_fn:file_glob)
ENTRIES=(
    "record_region_route_overlay_event:edgion-gateway/src/plugins/http/region_route/plugin.rs"
    "record_conn_ip_restriction_event:edgion-gateway/src/plugins/stream/global_connection_ip_restriction/plugin.rs"
)

REQUIRED_SIGNALS=(overlay_missing overlay_type_mismatch overlay_invalid overlay_disabled)

violations=0

for entry in "${ENTRIES[@]}"; do
    recorder="${entry%%:*}"
    file="${entry#*:}"
    if [ ! -f "$file" ]; then
        echo "ERROR: overlay-consumer file '$file' not found (recorder '$recorder')" >&2
        violations=$((violations + 1))
        continue
    fi
    missing=()
    for sig in "${REQUIRED_SIGNALS[@]}"; do
        # Match e.g. `record_region_route_overlay_event("overlay_missing")` --
        # any whitespace between fn and ( is fine.
        if ! grep -qE "${recorder}\s*\(\s*\"${sig}\"" "$file"; then
            missing+=("$sig")
        fi
    done
    if [ "${#missing[@]}" -gt 0 ]; then
        echo "ERROR: $file emits '$recorder' but is missing signal(s): ${missing[*]}" >&2
        violations=$((violations + 1))
    fi
done

if [ "$violations" -gt 0 ]; then
    echo "Found $violations consumer(s) missing required overlay signals. See above." >&2
    echo "Required: ${REQUIRED_SIGNALS[*]}" >&2
    exit 1
fi
echo "OK: all overlay-consumer plugins emit the full ${#REQUIRED_SIGNALS[@]}-signal set"
