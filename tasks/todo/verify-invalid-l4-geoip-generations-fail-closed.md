# Verify Invalid L4 GeoIP Generations Fail Closed

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | integration testing |
| Priority | P2 |

## Requirements

- Extend the existing `parse_only_is_rejected_at_l4` dynamic scenario only in a
  later integration-test-authorized change.
- After the invalid generation reports `Accepted=False`, make a bounded TCP
  probe and prove traffic does not reach the echo backend.
- Publish a valid replacement generation and prove bounded recovery so the test
  distinguishes fail-close from a permanently broken listener.
- Preserve the scenario's existing status assertions and deterministic cleanup.
- Do not weaken Gateway unit coverage for the invalid-generation gate.
