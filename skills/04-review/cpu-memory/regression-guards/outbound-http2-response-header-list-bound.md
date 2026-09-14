---
name: outbound-http2-response-header-list-bound
description: Use when reviewing decoded HTTP/2 response-header memory bounds or configuration for canonical reqwest outbound clients.
knowledge_type: regression-guards
---

# Canonical outbound clients bound decoded HTTP/2 response headers

## Conclusion and scope

Every reqwest client built through
`edgion_common::link_sys_core::http_client::{build_outbound_client, apply_outbound_policy}`
sets an explicit decoded HTTP/2 response-header-list limit. The process default is
16 KiB. A Gateway process may configure `gateway.outbound_http2_max_header_list_size`
from 16 KiB through 64 KiB; the setting is restart-only and applies before any
canonical Gateway reqwest client is constructed. Controller and CLI processes keep
the 16 KiB default.

This protects only Gateway-initiated reqwest exchanges such as LinkSys/webhook,
OIDC, DSL HTTP calls, Wasm callouts and artifact pulls, GeoIP updates, and active
health checks. It does not govern downstream client requests, Pingora route
upstreams, HTTP/1 response headers, or tonic channels.

## Rationale and evidence

The original `bound-outbound-http2-response-header-lists` task found that the
canonical factory did not call `ClientBuilder::http2_max_header_list_size` and
therefore inherited reqwest/Hyper's current 16 KiB default. That runtime was already
bounded, but reqwest documents the default as mutable. A dependency upgrade could
therefore change Edgion's memory and interoperability boundary without a deliberate
project decision.

`apply_outbound_policy` now always passes the effective process value to reqwest,
which forwards it through Hyper to h2 before a response header map reaches a
consumer. h2 accounts for decoded header names, values, and the HTTP/2 per-field
overhead. Existing consumer-level error mapping remains unchanged, so dependency
details stay in operational diagnostics rather than public fail-close bodies.

The canonical construction invariant is owned by
`skills/01-architecture/00-common/05-unification-points.md`. The Gateway installs
its validated override during startup before outbound egress defaults, Wasm OCI,
GeoIP, resource watches, and plugin publication can construct reqwest clients.

## Alternatives and limits

Leaving the value unset was rejected because it would continue to inherit a
dependency default and would not close the regression. A fixed 16 KiB constant was
insufficient for deployments whose external providers return large legitimate
headers, so the Gateway process accepts a bounded restart-only override. A hot-reload
resource field was rejected because existing reqwest clients and their connection
pools would retain the old decoder configuration until rebuilt.

The 64 KiB ceiling is a decoded-list policy, not a precise process-RSS budget. h2 may
temporarily consume additional bounded protocol-decoding state, and each concurrent
connection or stream has independent state. Raising the setting therefore trades
memory headroom for provider interoperability.

## Verification and re-evaluation

Unit coverage in `edgion-common::link_sys_core::http_client::tests` verifies the
exact configuration boundaries: 16 KiB and 64 KiB are accepted, while one byte
below and above that range is rejected. Gateway configuration tests verify the
16 KiB default and a 64 KiB YAML value. The crate-level integration test
`edgion-common/tests/outbound_http2_header_limit.rs` uses the canonical public
policy builder with an h2 peer: a decoded response-header list one byte below
the configured limit succeeds, while one exactly at h2's rejection boundary
fails. Network setup stays outside `#[cfg(test)]` unit tests as required by the
project testing rules.

Fresh verification on 2026-09-14 passed:

- `cargo test -p edgion-common --test outbound_http2_header_limit -- --nocapture`
- `cargo test -p edgion-common link_sys_core::http_client::tests -- --nocapture`
- the related Gateway configuration unit tests
- `cargo fmt --all`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets`
- agent-document, SSA-force, metrics-inventory, and Pingora-source-policy guards

Re-evaluate this rule if reqwest removes the builder API, Hyper/h2 changes decoded
header accounting or boundary semantics, a supported provider requires more than
64 KiB, or Gateway outbound clients become safely rebuildable during hot reload.

## Resolution references

- `bound-outbound-http2-response-header-lists` — fixed on 2026-09-14; source task was `tasks/todo/bound-outbound-http2-response-header-lists.md`.
