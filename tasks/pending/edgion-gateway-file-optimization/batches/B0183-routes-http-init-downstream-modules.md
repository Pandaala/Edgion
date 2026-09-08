# B0183 — HTTP Downstream Module Initialization

## Scope

- Candidate analysis and freshness-revalidation HEAD: `d2da7f4dd1332f56d1564d22f1e8cd398cb3d96c`
- Target blob: `c9c590526a15f4a7a30f89f5721205f957002a72`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned code: `edgion-gateway/src/routes/http/proxy_http/pg_init_downstream_modules.rs`
- Frozen support allow-list:
  - `edgion-gateway/src/routes/http/proxy_http/pg_early_request_filter.rs`
  - `skills/04-review/h2-grpc/regression-guards/grpc-web-independent-of-frontend-http2.md`
  - `skills/04-review/h2-grpc/SKILL.md`

## Handoff

- Analysis: `b0170_final_review` read the complete initialization and per-request module lifecycle, listener/config ownership, early request and response filters, error rendering, gRPC-Web integration, upstream protocol selection, pinned Pingora implementation, and review precedent.
- Finding: `enable-http2` owns frontend h2c/ALPN, but it currently also controls registration of Pingora's HTTP/1.1 gRPC-Web-to-H2 bridge. Disabling frontend H2 therefore removes a bridge needed precisely by an HTTP/1.1 gRPC-Web client, despite independent H2 upstream selection.
- Intended fix: always register exactly one inert-by-default `GrpcWeb` builder; keep initialization restricted to recognized gRPC-Web content types before Pingora's module header filter. Preserve frontend protocol enablement, native gRPC rejection, compression, upstream selection, trailer encoding, response streaming, and observability.
- Integration: existing files remain frozen; defer an HTTP/1-only Gateway gRPC-Web wire scenario to `tasks/todo/add-grpc-web-http1-only-gateway-integration.md`.
- Implementation: the registration seam has no frontend-H2 input and always installs one compression context and one inert gRPC-Web context. Early filtering now runs the single canonical protocol detector on raw headers and activates the bridge only for the frozen `grpc-web` result; the later detector becomes a no-op after Pingora's header rewrite. A production predicate retains the native-gRPC H2 gate, while WebSocket precedence leaves the bridge disabled.
- Review: initial plan review removed an ignored frontend-H2 parameter. Fresh final review then found that Pingora's header rewrite would still misclassify gRPC-Web as native gRPC; the corrected, expanded plan froze raw protocol identity and added the WebSocket combination boundary. Correction re-review returned LGTM with no remaining Must Fix or Should Fix finding.
- Verification: registration tests passed 2/2, early/filter-order tests 7/7, protocol detector tests 3/3, and the complete proxy-http suite 242/242. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed.
- Commit: pending; never push.
