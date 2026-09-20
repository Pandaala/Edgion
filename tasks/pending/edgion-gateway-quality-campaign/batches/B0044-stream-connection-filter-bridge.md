# B0044 — Stream Connection Filter Bridge

## Scope

- Baseline: `b4ebea75087d3554c864b67a5a4e590bdd0935b2`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Anchor file: `edgion-gateway/src/plugins/stream/connection_filter_bridge.rs`.
- Approved knowledge support: `skills/01-architecture/02-gateway/00-overview.md`, `02-pingora-lifecycle.md`, `13-stream-plugin-dev.md`, `skills/04-review/listeners/intentional-design/bounded-serial-connection-admission.md`, and `skills/04-review/plugin-runtime/intentional-design/stream-plugin-store-key-normalization.md`.
- Forbidden files: every other source/knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Exact commit allow-list: the anchor, five knowledge files, and B0043/B0044 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis verdict: `change`; simple and not plan-first.
- A dual-stack listener can expose an IPv4 peer as IPv4-mapped IPv6. Stage-1 currently stores that spelling unchanged, while later HTTP/TCP/UDP paths canonicalize it.
- Matchers and GeoIP normalize internally, but `ConnectionRateLimit.perSourceIp` directly composes a Redis key from `direct_peer_ip`; native and mapped spellings can therefore split one physical peer across two fleet-wide buckets.
- Canonicalize the physical peer with the standard library before building `StreamContext`; keep it pre-PROXY `DirectPeerIp`, preserve callback order, and add focused native/mapped IPv4 and native IPv6 coverage.
- Five architecture/review pages still describe retired direct `StreamPluginStore` lookup. Correct them to the atomic `PluginPublication` snapshot, physical-listener `ConnectionView`, effective-view comparison, fail-close unavailable state, and pinned compiled runtime model.
- Preserve silent bridge denial because Pingora's connection-filter callback only returns a boolean; do not add request-level tracing or a parallel access log.

## Implementation

- Added an allocation-free peer-IP canonicalization helper and applied it before constructing the Stage-1 `StreamContext`.
- Added focused coverage proving native IPv4/IPv6 remain unchanged and IPv4-mapped IPv6 folds to native IPv4.
- Updated the five approved pages to describe atomic publication capture, physical-listener views, serving-Gateway equivalence, unavailable fail-close, and pinned runtime ownership.
- Preserved the fatal PROXY guard -> stream bridge -> PROXY parse order and pre-PROXY physical `DirectPeerIp` identity.

## Independent review

- The independent reviewer approved the runtime fix and found one residual direct-ArcSwap-store phrase in the changed developer guide.
- The implementer corrected that phrase within the existing allow-list; final re-review approved the complete six-file diff with no remaining findings.

## Verification

- Focused verification: 5 bridge tests and 88 stream tests passed; one existing measurement test remained ignored. Gateway all-target check and Clippy passed with only known Kafka warnings.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only. Only the campaign's known lint and duplicate-dependency warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `d3960e9c68d01b24e9bb3cff48a955b48d83bc3b`.
