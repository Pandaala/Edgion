# WS-04: A2A Gateway on `EdgionBackend.spec.a2a`

> Status update: the WS-00 F2 canonical bounded-exchange dependency was
> withdrawn in favor of ExtProc for external response processing. ExtProc is not
> an A2A backend fetch transport. This roadmap requires a fresh Agent Card fetch
> design before implementation; the dependency and flow text below are
> historical constraints, not implementation authorization.

**Goal:** add A2A backend identity, Agent Card public-URL rewriting, and
transparent JSON-RPC request-method observation without creating a second
backend resolver, rerunning route weighting, mandating body capture, or
buffering ordinary upstream responses.

This is no longer a standalone quick win. The original plugin-local/fresh
subrequest design failed independent review and is superseded by the unified
backend architecture.

## Dependency gates

All gates must pass before plugin implementation:

1. WS-00 F1 — `EdgionBackend.spec.ai` replacement proves the union/resource
   lifecycle.
2. WS-00 F2 — reduced one-time logical selection/runtime factoring, plus a
   separately approved Agent Card fetch transport.
3. WS-00 F3 — typed TLS/traffic-policy identity and capability matrix.
4. WS-00 F4 — `EdgionBackend.spec.a2a` Service/external targets.
5. WS-00 F5 — optional failure-open request-body observer.

The formal task under `pending/a2a-gateway/` remains blocked until these gates
are delivered and must be refreshed at kickoff against the then-current code.

## Backend/frontend separation

`EdgionBackend.spec.a2a` owns upstream identity and reachability:

```yaml
spec:
  a2a:
    target:
      serviceRef:
        name: travel-agent
        port: 8080
```

or:

```yaml
spec:
  a2a:
    target:
      endpoint: https://agent.example.com/base/
```

`A2aGateway` remains a request-stage frontend plugin and owns discovery paths,
public `externalUrl`, method observation, fetch deadline, and the sanctioned
non-auth `failOpen` behavior. Public gateway identity does not belong in the
backend resource.

Every existing, authorized, positive-weight backend on an A2A rule must be an
`EdgionBackend` with active `spec.a2a`. A Service or wrong union variant rejects
the plugin/rule with `Accepted=False`. Missing or denied references retain
their declared invalid weight and report `ResolvedRefs=False`.

## Agent Card discovery

```text
card GET
  -> match route and validate A2A-compatible logical slots
  -> select one logical slot once
  -> freshly reviewed bounded Agent Card fetch on that selected slot
  -> parse complete JSON object under hard caps
  -> rewrite protocol-defined endpoint URLs
  -> serialize fully
  -> write local response, then terminate request
```

Requirements:

- Reverify official A2A discovery paths, current/historical card shapes, and
  signed-card semantics at implementation kickoff.
- Inclusive 1 MiB input and serialized-output caps, checked arithmetic, total
  deadline, cancellation-safe cleanup, and redirect denial.
- Service and external targets use the canonical target/peer path, including
  ReferenceGrant, EndpointSlice, egress, DNS, TLS, SNI, authority, traffic
  policy, retry eligibility, and accounting.
- Preserve unknown JSON fields and rewrite only protocol-defined URL fields
  with a URL library.
- Explicit `externalUrl` wins. Automatic scheme comes from the actual
  downstream TLS state; raw forwarded proto/host is ignored without a trusted
  proxy mechanism.
- Validate request authority against matched listener/route hostnames.
- Do not forward backend authentication, cookie, hop-by-hop, topology, or stale
  validator headers.
- Serialize the complete local response before writing any downstream byte;
  write header and body before `ErrTerminateRequest`.

Fail-close returns a fixed generic response. Fail-open is permitted only before
local output begins and remains pinned to the already-selected logical slot; it
does not rerun route weighting. A second idempotent GET and its backend attempt
accounting must be explicit and tested.

## JSON-RPC method observation

- Only non-card POST requests register the generic WS-00 F5 observer.
- Observe only one top-level string `method`; batch arrays are not enumerated.
- Retain finite parser state and at most 64 decoded UTF-8 bytes.
- Reject control characters and drop, rather than truncate, overlong values.
- Malformed, duplicate, missing, wrong-type, empty, oversized, unreadable, or
  panic/error cases produce no method record and continue proxying byte-for-byte.
- Do not create BodyStorage, another request cache, or any response buffer.
- Emit one bounded sensitive-aware PluginLog token at body completion. Do not
  add a method-label metric or a second access-log mechanism.
- Tests prove params, IDs, bodies, authorization data, raw parser errors, and
  rejected methods never appear in logs.

## Phase 1 public configuration

The exact field validation is refreshed at kickoff. Durations use GEP-2257
strings through the shared parser; numeric millisecond fields are not allowed.
The intended surface is:

```yaml
type: A2aGateway
config:
  cardPaths:
    - /.well-known/agent-card.json
    - /.well-known/agent.json
  externalUrl: https://agents.example.com/base/
  observeMethods: true
  fetchTimeout: 5s
  failOpen: false
```

No template, credential, backendRef, TLS, retry, or endpoint field belongs in
this frontend config.

## Delivery phases

1. **Backend identity:** delivered by WS-00 F4, not by the plugin task.
2. **Observation seam:** delivered by WS-00 F5, not by the plugin task.
3. **A2A Phase 1:** plugin config/wiring, card rewrite, method logging,
   integration tests, hot reload, and English/Chinese docs.
4. **A2A Phase 2:** separately design response outcome/error/result/task-state
   observation and SSE event counting. It is not implicitly approved by Phase 1.

## Phase 1 non-goals

- AgentCard CRD or gateway-owned Agent Catalog;
- response outcome or SSE task-state observation;
- response-side BodyStorage or general response buffering;
- A2A-specific RBAC, SDK, client, server, or payload transformation;
- high-cardinality metrics;
- comparative benchmarking.

## Acceptance themes

- Compatible variant validation occurs before runtime selection.
- Logical weighting happens once; fail-open never selects another route slot.
- Card exchange shares canonical routing, transport, TLS, policy, and accounting.
- Main upstream responses remain streaming.
- Request observation is optional and failure-open at every call site.
- Signed cards, URL joining, forwarded-header distrust, partial-write behavior,
  dynamic config, backend deletion/recreation, and weighted slots are tested.
