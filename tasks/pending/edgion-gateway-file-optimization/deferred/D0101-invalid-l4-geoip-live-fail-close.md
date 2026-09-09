# D0101 — Invalid L4 GeoIP Live Fail-close Verification

- Status: closed (2026-09-09)
- Severity: medium (integration regression coverage)
- Discovered in: B0152 stream GeoIP plugin analysis
- Knowledge: [Invalid stream GeoIP generation denial and recovery](../../../../skills/04-review/plugin-runtime/regression-guards/invalid-stream-geoip-generation-fail-closed.md)

The separately authorized D0101 change extends `parse_only_is_rejected_at_l4`
with bounded live TCP denial, a valid baseline and recovery, and checked cleanup.
The owning knowledge page records the evidence and verification limits.
