# B0066 — Duplicate-Host Policy Hot Reload

## Scope

- Baseline: `99604942b4387c47dcdd7a787fa6688b8e6b5586`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/routes/http/redirect_http.rs`.
- Approved code files: `config/edgion_gateway/conf_handler_impl.rs`, `ctx.rs`, `routes/http/proxy_http/{pg_new_ctx.rs,pg_request_filter.rs}`, and `runtime/server/listener_builder.rs` under `edgion-gateway/src/`.
- Approved knowledge files: `skills/01-architecture/02-gateway/02-pingora-lifecycle.md` and `skills/04-review/http1/regression-guards/duplicate-host.md`.
- Existing `edgion-tests/**` and protected user paths remain forbidden.

## Context and value gate

- Analysis verdict: `change`; complexity is `complex`, so implementation is plan-first.
- Redirect and ordinary HTTP paths read `rejectDuplicateHost` from a listener-startup EGC clone, while hot updates only publish a new immutable snapshot and do not rebuild listeners.
- Both consumers must capture the canonical policy generation when creating their request context so updates apply to new requests without changing in-flight behavior.

## Required invariants

- Missing, deleted, or unspecified EGC references retain the safe `true` default; the deliberate `false` migration escape hatch remains supported.
- Ordinary HTTP retains one lock-free snapshot lookup for all request-scoped EGC fields; redirect performs one lookup in `new_ctx` and none in `request_filter`.
- Duplicate Host retains the existing 400 response and ordinary HTTP diagnostic.
- Redirect authority validation, IPv6 handling, query preservation, status and `Location`, keepalive ownership, masking, and access-log schema remain unchanged.
- No new tracing, metric, log, cache, parser, or parallel configuration mechanism.

## Verification plan

- Extend EGC snapshot and both request-context test groups for update, deletion, missing-reference, default, and in-flight pinning behavior.
- Preserve and rerun redirect wire tests for duplicate Host and redirect output.
- Run full campaign repository gates and frozen-path verification.

## Deferred findings

- D0012 tracks the broader `securityProtect` EGC hot-reload audit across request, connection, and handshake lifecycles.
- D0013 tracks frozen cross-process update/delete integration coverage for ordinary and redirect listeners.

## Implementation

- Added `reject_duplicate_host` to the canonical resolved HTTP request snapshot with a safe `true` default.
- Ordinary HTTP copies the policy through its existing single snapshot lookup and consumes the request-pinned value in metadata processing.
- Redirect listeners retain only the EGC name; `new_ctx` performs one canonical lookup and pins the value for a lookup-free request filter.
- Preserved duplicate-Host 400 handling and diagnostics, explicit `false` behavior, redirect 301/`Location`, authority parsing, connection ownership, masking, and access-log shape.

## Independent reviews

- The independent plan review approved the six-file code scope, two knowledge files, snapshot/default semantics, consumer-level tests, and frozen integration boundary.
- The independent final reviewer returned LGTM with no Must Fix or Should Fix findings and confirmed both consumers, generation pinning, rejection ordering, redirect wire behavior, observability restraint, and scope discipline.

## Verification

- Focused snapshot, request-context, nine request-filter, and 11 redirect tests passed; independent review reran the relevant consumer regressions.
- Formatting, diff, workspace check/clippy, agent-doc, SSA-force, metrics-inventory, Pingora-source-policy, Gateway API transform-contract and its 5 tests, no-tracing, cargo-deny, English-only, and frozen-path gates passed.
- Clippy retained only the known unrelated Kafka and frozen integration-test warnings. No full test suite was requested or run.

## Handoff

- Implementation and knowledge committed as `478bd5765`.
- D0012 and D0013 remain separate; next campaign anchor is `edgion-gateway/src/routes/tls/mod.rs`.
