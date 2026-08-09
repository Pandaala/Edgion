# WS-01: MCP Gateway (traffic-layer scope)

**Goal:** Terminating MCP service in the gateway: clients speak MCP (streamable
HTTP + legacy SSE) to Edgion; Edgion federates N upstream MCP servers with
session management and traffic-level tool filtering.

**Scope decision (user, 2026-08-09):** Edgion stays a traffic-layer gateway.
- **No process execution, ever**: stdio upstream transport is permanently out of
  scope — spawning operator-configured commands is a sandbox/runtime
  responsibility, and a CRD carrying cmd/args/env would turn config permission
  into code-execution permission. K8s users bridge stdio servers via
  sidecar/mcp-proxy adapters; document that pattern.
- **No MCP-specific identity/authorization for now**: tool-level RBAC, OAuth
  Protected Resource Metadata, IDP integrations, and token exchange are an
  authorization system embedded in a gateway. Edgion has no unified authz
  foundation yet; building a one-off for MCP would be rebuilt later. Coarse
  authentication comes from the existing generic route plugins (`jwt_auth`,
  `key_auth`, …) attached to the MCP route. The whole identity block moves to a
  future "authorization system" program.
- **Kept as the traffic-level middle ground**: an optional static per-member
  tool allow/deny list (config-layer filtering, identity-unaware) covers the
  common "only expose these tools" need without any authz machinery.

**Reference:** agentgateway `crates/agentgateway/src/mcp/` (rmcp-based). We match
the transport/federation/session capability, not the authz layer and not the
implementation.

**Depends on:** nothing hard; SseDecoder v2 (WS-06) improves the SSE leg but the
current decoder suffices for phase 1.

## Architecture

- New module `edgion-gateway/src/mcp/` — a **route-terminating service** selected
  the way `direct_endpoint` terminates routes today (route backendRef of a new
  kind ⇒ the proxy phase short-circuits into the MCP handler instead of
  `upstream_peer`). The client-facing leg uses the normal downstream session
  (H1/H2 + SSE streaming responses); every upstream MCP call is a **bounded
  outbound exchange** through `build_outbound_client` (invariant 3: 1 MiB cap per
  response, fail-not-truncate) — JSON-RPC messages are small; streaming upstream
  notifications use a dedicated persistent SSE client connection owned by the
  session object, parsed incrementally by `SseDecoder` (bounded), never
  accumulated.
- New CRD `EdgionMCPService` (namespaced) describing one upstream MCP server:
  transport (streamableHttp | sse), endpoint, auth injection (secretRef),
  timeouts, failure mode (failOpen/failClosed for federation fanout).
- Federation config lives on the route via a new plugin `mcp_gateway`
  (`EdgionPlugin::McpGateway`): member list (refs to `EdgionMCPService`), name
  prefix mode, session TTL, optional per-member tool allow/deny lists.
- Rust MCP SDK: evaluate `rmcp` (agentgateway's choice) vs hand-rolled JSON-RPC
  types. Proposal: **use `rmcp` for protocol types only** (serde structs), keep
  transport/session/routing our own so invariants and outbound rules stay ours.
  Decision recorded at kickoff after a license/API review.

## File map (create unless noted)

```
edgion-gateway/src/mcp/
├── mod.rs                  # facade; McpGatewayService entry from route dispatch
├── protocol.rs             # JSON-RPC framing + MCP message types (or rmcp re-export)
├── version.rs              # protocol version negotiation matrix
├── session.rs              # gateway-owned session store: id gen, TTL + reaper task,
│                           #   backend pinning (per agentgateway CVE lesson),
│                           #   stateless-server synthetic sessions
├── transport/
│   ├── streamable_http.rs  # client-facing streamable HTTP endpoint handling
│   ├── sse_legacy.rs       # client-facing legacy SSE endpoint
│   └── upstream.rs         # upstream connector: bounded POST exchanges +
│                           #   persistent SSE notification stream per session
├── router.rs               # tool/prompt/resource name prefixing, request routing
│                           #   to member servers, list-merge fanout with
│                           #   failOpen/failClosed per member, static per-member
│                           #   tool allow/deny filtering
└── observability.rs        # access-log fields (method, session, tool), EdgionStatus

edgion-resources/src/resources/
├── edgion_mcp_service.rs           # new CRD (follow edgion_ai_service.rs pattern)
└── edgion_plugins/plugin_configs/mcp_gateway.rs  # plugin config + validation

Modify:
- edgion-resources/src/resources/resource/kind.rs / defs.rs   (register kind)
- edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs (variant)
- edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs  (terminating dispatch,
    modeled on the existing direct_endpoint short-circuit)
- config/crd/edgion-crd/  (new CRD YAML)
- edgion-tests: test_server gains a mock MCP server mode; integration scenarios
```

## Phases (each = one feature-profile task dir at execution time)

### Phase 1 — single-upstream MCP proxy (P0 core)
1. `EdgionMCPService` CRD + store (ArcSwap index, `service_store.rs` pattern).
2. `mcp_gateway` plugin config + route-terminating dispatch.
3. Streamable HTTP client leg + bounded upstream exchange; version negotiation
   passthrough for one pinned protocol version (target: 2025-03-26 baseline +
   2025-06-18; chase-latest is a standing follow-up).
4. Session store with TTL + reaper + backend pinning; stateless wrapping.
5. Tests: mock MCP server in `edgion-tests` test_server; initialize/tools-list/
   tools-call round trip; session expiry; oversized-response fail-closed.

**Deliverable:** one MCP server proxied end to end with sessions.

### Phase 2 — federation (the gateway value-add)
1. Multi-member routing: name prefixing (`McpPrefixMode` equivalent), list-merge
   fanout across members, per-member failure mode, static per-member tool
   allow/deny lists (config-validated; identity-unaware by design).
2. Notification fan-in: per-member SSE streams → client stream (SseDecoder,
   bounded per-event; drop-with-status on overflow, mirroring decoder policy).
3. Tests: 2-member federation, one member down (failOpen vs failClosed), prefix
   collision validation controller-side, filtered tool listing.

### Phase 3 — polish
- Legacy SSE client leg; guardrail hook points (defer actual guards to WS-03);
  observability fields per our restraint policy; docs (`docs/en/mcp-gateway/`),
  including the sidecar/mcp-proxy adapter pattern for stdio servers.

### Permanently out of scope (traffic-layer decision)
- stdio upstream transport (process execution — sandbox territory).

### Deferred to a future authorization-system program
- Tool/prompt/resource-level identity RBAC; OAuth PRM (RFC 9728) + IDP
  integrations; token exchange/XAA. Also deferred: OpenAPI→MCP generation,
  cross-upstream subscriptions, MCP guardrails.

## Absorbed from the ws4 AI-6 program docs (design inputs, now in scope)

Source: `../archive/program-origin/06-mcp-and-agent/` (details there; summarized into
the phases above at expansion time):

- **Transport hardening checklist** (from `04-security-and-compat.md`): Origin
  header validation (DNS-rebinding defense — an MCP spec requirement); CORS
  must include `Access-Control-Expose-Headers: Mcp-Session-Id` (browser MCP
  clients are completely broken without it); `MCP-Protocol-Version` header
  negotiation; `Last-Event-ID` passthrough for SSE resume.
- **Aggregation specifics** (from `README.md` §1): per-member failure mode
  defaults FailClosed; stateless members get a synthetic initialize; prefix
  rename applies to tools/prompts/resources consistently.
- **tools/list hash-locking** ("rug-pull" detection, README §4): pin a hash of
  each member's tool list; on drift, surface a status condition instead of
  silently serving changed tools. Parameter-level DLP noted as a
  differentiator — defer to WS-03 machinery.
- **Policy rejections are JSON-RPC-shaped** (from `03-jsonrpc-governance.md`):
  tool-level limits/filters return JSON-RPC error bodies, not bare HTTP 429.
- **Session-target-dead semantics** (from `05-retry-semantics.md`): fast-fail
  with a JSON-RPC error prompting client re-initialize; never retry a
  session-bound request across instances.
- **Parked** (closed decision 2 — future authorization program): OAuth
  Protected Resource Metadata (RFC 9728), token exchange (RFC 8693), identity
  RBAC (`02-oauth-resource-server.md` and README §2/§3 remain the reference).

## Invariant compliance notes

- No response-side buffering: upstream JSON-RPC responses are bounded outbound
  exchanges (1 MiB, invariant 3); client-facing responses are locally generated
  (merge results) or streamed SSE.
- Fail-close error bodies: JSON-RPC error objects use fixed messages; detail to
  access log (rule 9).
- New egress: upstream endpoints are operator-configured → `EgressResolver` +
  proxy-env disabled + redirect deny per rule 3.
- Route authentication uses the existing generic auth plugins unchanged (their
  fixed fail-close classification per rule 10 already applies); the MCP layer
  itself adds no auth decision points in this scope.
