# B0055 — Joint-Generation EGC Request Context

## Scope

- Baseline: `de8abb130be440c7c104dd0934c547d2a3241b03`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Starting file: `edgion-gateway/src/routes/http/proxy_http/pg_early_request_filter.rs`.
- Exact runtime allow-list:
  - `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`
  - `edgion-gateway/src/config/edgion_gateway/mod.rs`
  - `edgion-gateway/src/config/mod.rs`
  - `edgion-gateway/src/routes/http/proxy_http/pg_new_ctx.rs`
  - `edgion-gateway/src/ctx.rs`
- Existing `edgion-tests/**` files, unrelated modules, knowledge pages, and protected user paths remain forbidden.
- Exact commit allow-list: the five runtime files above plus B0054/B0055 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Current-HEAD analysis verdict: `change`; implementation complexity: `moderate`, plan-first.
- The starting early-filter callback is sound, but its `new_ctx` setup chain independently calls the EGC body-policy and RemoteIp-header getters.
- Each getter performs its own `ArcSwap` load and map lookup, so a concurrent hot reload can place values from two EGC generations into one request context while every request also pays redundant snapshot work.
- One resolved per-EGC HTTP-request value should carry both the pre-resolved body policy and validated RemoteIp output header and be fetched through one load and lookup.
- Preserve invalid-header fallback to `x-real-ip`, invalid-body fallback to body defaults, missing/deleted EGC defaults, in-flight request pinning, and cold-path-only parsing/validation/logging.
- Correct the stale `ctx.rs` comment that says request-body policy is parsed at listener construction.

## Plan requirements

1. Keep the publication shape immutable and jointly epoch-consistent without adding request-path parsing, locks, allocations, or new mechanisms.
2. Replace the two internal accessors/re-exports with one combined getter and update all callers.
3. Extend same-file config tests to cover joint values across update, deletion, invalid-header fallback, and default-body fallback.
4. Preserve all external schema, protocol, response streaming, access-log, and observability behavior.

## Verification plan

- Focused EGC config tests and proxy context/early-filter tests.
- Full campaign repository gates and frozen-path verification.

## Approved plan and implementation

- An independent plan reviewer approved the six-step implementation plan and required explicit tests that invalid header and invalid body settings fall back independently.
- Added crate-visible `ResolvedHttpRequestConfig` to carry the resolved request-body policy and validated RemoteIp header in one immutable per-EGC map.
- Replaced the two getters and snapshot maps with one getter that performs one `ArcSwap` load, one lookup, and one shallow value clone; no compatibility shims remain.
- `new_ctx` captures the complete pair once and moves both values into request-owned context state, preserving in-flight generation pinning.
- Missing, deleted, and absent references return the complete default; invalid header retains a valid body policy, and invalid body fields retain a valid custom header.
- Corrected the stale context comment; no external schema, protocol, response-streaming, logging, metric, or access-log behavior changed.

## Independent code review

- A fresh final reviewer approved the implementation with no Must Fix, Should Fix, or deferred finding.
- The review verified the one-load guarantee, complete caller/export removal, crate-private visibility, shallow header clone then move, immutable publication/update/delete behavior, fallback independence, retained old values, and frozen-test containment.

## Verification

- Focused tests passed: 31 EGC ConfHandler, 9 early-request-filter, and 13 upstream-request-filter tests.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only.
- Only the campaign's known Kafka and frozen integration-test clippy warnings plus cargo-deny duplicate/advisory-ignore warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `cb7d2a5323f48d9a96e32751614e2197543f083b`.

## Protected paths

- `task/`
- `tasks/pingora-0.9.0/`
- `tasks/todo/complete-k8s-dynamic-scenario-parity/`
- `waf-architecture-and-multi-pattern-matching.md`
