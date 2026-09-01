# B0098 — RealIp

## Scope
- Baseline: `5b84915b` (promoted on `d9bd728c`; rebased after an external committed Proxy Protocol merge without target-blob drift)
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/real_ip/{mod.rs,plugin.rs}`
- Proposed support allow-list: `edgion-gateway/src/util/real_ip_extractor.rs`, revisited without new campaign coverage credit
- Forbidden files: `mod.rs`, resources, Controller, PluginRuntime/session/context, routing, documentation, Cargo, every existing `edgion-tests/**` path, and protected user paths

## Analysis boundary
- Trace resource validation, Controller/Gateway preparse, PluginRuntime, downstream/direct/current peer state, PROXY and trusted-proxy layering, the canonical extractor, context mutation and GeoIP invalidation, security consumers, access logs, tests, and frozen integration coverage.
- Preserve immutable `DirectPeerIp`, PROXY-effective downstream peer ownership, current-layer `RemoteIp`, extractor trust semantics, recursive and trusted-hop rules, unspecified fallback on invalid peer text, GoodNext failure behavior, sensitive PluginLog arguments, and canonical access-log field names.
- Existing integration tests remain frozen. Record a separate issue only if a genuine future integration gap cannot be covered deterministically in colocated tests.

## Handoff
- Analyst: independent read-only analysis completed against `7f60da30`; both anchor blobs and the support blob were unchanged when promoted on `d9bd728c`
- Decision: medium complexity; plan-first required before freezing the two-file implementation allow-list
- Candidate: make one canonical extraction pass return both resolved RemoteIp and downstream-trust state, precompute static header log classification, avoid copying the current RemoteIp, and remove the owned plugin name
- Implementer plan: keep the exact two-file allow-list; add a crate-local non-Debug resolution carrying `remote_ip` and `downstream_trusted`, delegate both existing extractor entries to it, preclassify static header logs, compare RemoteIp in a short borrow, and cover trust/fallback/log/mutation parity with colocated tests
- Plan review: independently approved; `Option<&str>` is passed by value, initial peer trust is evaluated once, legacy entries delegate, static classification keeps exact ASCII-lowercase behavior, and DirectPeer/GeoIP ownership remains untouched
- External drift revalidation: HEAD advanced to `5b84915b`; both frozen source blobs remained exact and the integration manifest was re-snapshotted to the newly committed external `edgion-tests/**` state without editing integration content
- Implementation: exact two-file allow-list honored; twenty-eight RealIp tests, one GeoIP invalidation test, Gateway lib check, targeted Clippy, and local guards passed
- Final review: independently approved after adding non-UTF-8 legacy fallback, empty-peer unspecified fallback, and isolated sensitive-log redaction coverage; no findings remain
- Verification: `cargo fmt --all` and check mode, locked workspace check and Clippy, Gateway binary build, agent-doc/SSA/metrics/Pingora/no-tracing/English-only guards, diff check, and frozen paths all passed on `5b84915b`; Clippy emitted only pre-existing warnings
- Coverage: `mod.rs` examined-no-change; `plugin.rs` changed; `util/real_ip_extractor.rs` revisited as changed support without duplicate coverage credit
- Integration: the manifest was re-snapshotted after the user's committed external integration changes, then remained frozen; existing RealIp/PROXY/canonical access-log coverage is sufficient and no integration issue was created
- Deferred: D0064 owns canonical RealIp header-name admission; no plugin-local runtime validation is allowed here
- Commit: `e391354b`
