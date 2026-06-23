---
name: core-runtime-module-split-naming
description: Use when reviewing LinkSys module-organization findings; covers why the common-side core lives in `link_sys_core` while the gateway runtime keeps `link_sys`, and why the gateway re-export is intentional.
status: fixed
finding_id: NAMING-008
closed: 2026-06-21
---

# LinkSys core/runtime module split — `link_sys_core` (common) vs `link_sys` (gateway) — FIXED

## Conclusion

The LinkSys subsystem is intentionally split across two crates with two module names:
`edgion-common/src/link_sys_core/` holds the backend-agnostic **core** (credential
state machine, TLS material, health type, lifecycle trait + the etcd provider the
controller also needs); `edgion-gateway/src/link_sys/` holds the data-plane
**runtime** (sink/connector providers + store/dispatcher) and re-exports the core so
gateway call sites stay stable. The names now encode the core/runtime boundary
(`link_sys_core` was previously the over-generic `link`).

Fix suggestions of the form "the two halves should share one name" or "drop the
gateway re-export so the real definition location is visible" are **not accepted**.

## Core Rationale

**1. The two names mark a real architectural boundary.**

The split is dictated by the crate DAG (`resources → common → {gateway, controller}`):
the controller's etcd-backed config center needs the credential/TLS/health/lifecycle
primitives + etcd provider, so they must live in `edgion-common` where both planes
can depend on them. The data-plane providers and runtime store are gateway-only.
`link_sys_core` (common) vs `link_sys` (gateway) names that core-vs-runtime line —
collapsing them to one name would hide it.

**2. The original `link` name was the actual defect, and it is fixed.**

NAMING-008 was that the common module was named `link` (too generic to signal "LinkSys
core") while the gateway used `link_sys`. The rename `edgion-common/src/link →
link_sys_core` resolves it: the placement rule is now stated in both module docs and
in `skills/01-architecture/02-gateway/09-link-sys.md` — backend-agnostic core logic
(or a provider the controller also needs) goes in common `link_sys_core`; data-plane
runtime providers/stores go in `gateway::link_sys`.

**3. The gateway re-export is deliberate, not obfuscation.**

`pub use edgion_common::link_sys_core::{credential, health, lifecycle, tls,
LinkSysHealth}` keeps gateway crate paths stable so existing call sites are unaffected.
Its doc comment now points at the core's real home and restates the placement rule, so
the re-export no longer "hides" where the definitions live.

## Fix Suggestions Not Accepted

- "Merge `link_sys_core` and `link_sys` into one module/name" — they live in different
  crates by necessity (controller needs the core; cannot depend on gateway). One name
  cannot span the crate boundary.
- "Remove the gateway re-export and make callers import from `edgion_common`" — churns
  every gateway call site for no benefit; the re-export's whole point is path stability.
- "Move the data-plane providers into common too" — would pull gateway-only dependencies
  (fred, reqwest, etc.) into the controller's dependency tree.

## Re-evaluation Triggers

Re-open this decision only if:

- The crate dependency graph changes such that the controller no longer needs any LinkSys
  core primitive (then the core could fold back into the gateway).
- A third consumer of the LinkSys core appears that warrants its own crate.

## Reference Cases

- NAMING-008 (2026-06-21) — established this rule; renamed `edgion-common/src/link` to
  `edgion-common/src/link_sys_core`.
- `edgion-common/src/link_sys_core/mod.rs` — core module doc + placement rule.
- `edgion-gateway/src/link_sys/mod.rs` — runtime module + re-export with boundary note.
- `skills/01-architecture/02-gateway/09-link-sys.md` — architecture doc (directory layout).
