# Align conf_sync operational limits overview

## Meta

- Status: pending
- Type: docs
- Priority: P2
- Baseline: `f0ec920dcaf97f3c21c3f5c5ae5e7be5a6d06b09`

## Evidence

The Operational limits section in `skills/01-architecture/03-controller-gateway-link/00-overview.md` still describes a 64 MiB List limit and a queue capacity of 100. Current `edgion-common/src/conf_sync/payload.rs` declares a 512 MiB limit; `edgion-controller/src/conf_sync/conf_server/grpc_server.rs` Watch forwarding uses capacity 1. The bounded-payload regression guard already records the newer contract.

## Required work

Verify the current constants, exact queue ownership and compressed/decompressed limit semantics against `skills/04-review/conf-sync/regression-guards/bounded-payload-compression.md` and the implementation. Correct only stale operational statements in the owning overview. Preserve any separately scoped limits; do not replace unrelated queues by textual coincidence. Run agent-doc validation.

## Why separate

This pre-existing documentation discrepancy is outside the build-script error-handling batch. No runtime change is proposed.
