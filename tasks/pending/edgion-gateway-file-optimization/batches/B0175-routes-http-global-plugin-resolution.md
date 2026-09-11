# B0175 — HTTP Global Plugin Resolution

## Scope

- Analysis and promotion HEAD: `efcc445f`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned files:
  - `edgion-gateway/src/routes/http/proxy_http/global_plugins_resolve.rs`
  - `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`
- Exact technical implementation allow-list: the two owned files only

## Handoff

- Analysis: `b0168_final_review` read the complete target and request-filter flow, pinned plugin execution state, PluginStore publication and constructors, ExtensionRef and backend consumers, access-log resolution, tests, frozen integrations, and relevant review precedent.
- Decision: complex only because the small ownership change touches the request hot path. Replace two singleton-returning Arc clones with the canonical static PluginStore borrow, and make resolver tests use local stores rather than mutating process-global state behind a module-only lock.
- Value: HTTP requests avoid up to two global Arc atomic increment/decrement pairs and gRPC requests avoid one, while target tests become parallel-safe across modules.
- Preserved behavior: the preflight boundary, global/gRPC/Gateway/route lookup order, dangling-reference policy, runtime Arc identity, WAF selection, hot reload, context pinning, failure behavior, and all observability remain unchanged.
- Plan review: a fresh reviewer verified the stable singleton lifetime, exact outer Arc atomic cost, unchanged ArcSwap lookup timing and generation semantics, local-store test feasibility, scope, and stop conditions; verdict Approved with no Must Fix.
- Implementation: both request-filter sites now borrow the static PluginStore at their original pre/post-preflight positions. Resolver tests use local stores without a global lock or clearing, and exact Arc identity assertions cover ordering, default namespaces, positional dangling entries, and duplicates.
- Final review: a fresh independent reviewer traced publication, lookup timing, runtime context pinning, plugin order/failure behavior, WAF selection, and all later consumers; verdict LGTM with no Must Fix or Should Fix. The wider coherent-generation task remains correctly separate.
- Testing: resolver tests passed 8/8, request-filter tests 13/13, and the complete HTTP route suite 345/345. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed. Existing integrations remain frozen and sufficient.
- Request-wide PluginStore generation capture was initially deferred, then declined by the user on 2026-09-09. See `skills/04-review/plugin-runtime/superseded/request-plugin-reference-generation-boundary.md` for the historical accepted limits; no snapshot implementation was added in that work.
- Commit: `6df7f67f`; never pushed by the campaign.
