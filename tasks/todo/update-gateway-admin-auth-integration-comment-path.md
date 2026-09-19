# Update the Gateway Admin Auth Integration Comment Path

## Meta

- Status: pending
- Type: docs
- Priority: P3
- Discovered in: Gateway quality campaign B0003

## Evidence

`edgion-tests/integration/scripts/integration/test_gateway_admin_auth.sh` still names the removed source path `src/core/gateway/api/admin_auth.rs` in a comment. The implementation now lives at `edgion-gateway/src/api/admin_auth.rs`.

## Why deferred

Existing `edgion-tests/**` files are frozen during the quality campaign, and the comment does not affect test behavior.

## Candidate next step

Update only the stale comment in a separate documentation/test-maintenance task and verify the script remains byte-for-byte unchanged otherwise.
