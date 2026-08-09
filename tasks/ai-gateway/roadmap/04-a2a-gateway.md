# WS-04: A2A Gateway

**Goal:** Parity with agentgateway's A2A support (which is deliberately small,
`a2a/mod.rs` = 222 lines): agent-card URL rewriting + JSON-RPC method/outcome
observability + the generic policy stack on A2A routes. Quick win — recommended
first workstream.

## What agentgateway actually does (scope ceiling)

1. Proxies A2A JSON-RPC POSTs transparently.
2. Intercepts agent-card fetches (`/.well-known/agent.json` v0.2/v0.3,
   `/.well-known/agent-card.json` v0.3+) and rewrites `url` (v0.3) /
   `supportedInterfaces[].url` (v1.0) to the gateway's external URL, honoring
   `X-Forwarded-Proto`.
3. Peeks JSON-RPC `method` on requests; classifies responses
   (success/error/kind/task state) for logs/metrics.
4. No A2A-specific RBAC vocabulary, no payload transformation.

## Architecture (invariant-clean)

- New plugin `a2a_gateway` (`EdgionPlugin::A2aGateway`), request stage.
- **Card rewrite without response buffering** (invariant 2 stays intact): on a
  card-path GET, the plugin performs a **gateway-initiated bounded outbound
  fetch** of the card from the backend (invariant 3: `build_outbound_client`,
  1 MiB cap, timeout), rewrites the URL fields, and returns it as a local
  response (`PluginRunningResult::ErrTerminateRequest` — the sanctioned path
  for inherently dynamic responses, rule 8). The main proxy never streams the
  card. Backend resolution reuses the route's backendRef target; egress goes
  through `EgressResolver` (rule 3).
- External-URL derivation: listener host/proto + `X-Forwarded-Proto` via the
  existing `real_ip`/forwarded handling; overridable `externalUrl` config field
  (restricted template variables per rule 7).
- **Method observability**: request-stage JSON peek via the BodyStorage view
  (`json_get`-style bounded scan — the DSL scanner infra, no new parser),
  recorded as access-log fields. Response outcome classification: a small
  read-only `StreamingResponseProcessor`? No — response classification needs
  JSON parsing of a *bounded, non-streaming* JSON-RPC response; A2A responses
  may stream (SSE for tasks). Phase 1 logs request method only; phase 2 adds
  response outcome for non-streaming responses via a bounded (1 MiB,
  usage.rs-style latched) side accumulation in a processor — mirroring the
  existing bounded non-streaming usage buffer pattern, and SSE task streams get
  event-kind counting via SseDecoder without accumulation.

## File map

```
Create:
edgion-gateway/src/plugins/http/a2a_gateway/mod.rs
edgion-gateway/src/plugins/http/a2a_gateway/plugin.rs   # card intercept + method peek
edgion-gateway/src/plugins/http/a2a_gateway/card.rs     # card fetch + v0.2/v0.3/v1.0
                                                        #   rewrite (serde models kept
                                                        #   minimal: only url fields
                                                        #   typed, rest preserved via
                                                        #   serde_json::Value passthrough)
edgion-gateway/src/plugins/http/a2a_gateway/observe.rs  # response outcome processor
                                                        #   (phase 2)
edgion-resources/.../plugin_configs/a2a_gateway.rs      # config: cardPaths override,
                                                        #   externalUrl, observability
                                                        #   toggles; validation

Modify:
edgion-resources/.../edgion_plugin.rs      # variant; BodyRequirement: none for card
                                           #   GET; request-body declared for method
                                           #   peek (fail-open observability — it is
                                           #   not auth)
edgion-tests test_server                   # mock A2A backend: card endpoints (three
                                           #   versions) + JSON-RPC echo
docs/en/ (user guide page)
```

## Phases

1. Card rewrite (all three card shapes) + request method logging. Golden tests
   for each card version; proto/host derivation tests.
2. Response outcome observability (bounded non-streaming + SSE kind counting).
3. Docs + example manifests.

## Notes

- Failure policy: card fetch failure ⇒ pass through untouched card? No — the
  plugin intercepted the request, so it must respond; policy knob
  (`onCardFetchFailure: passthroughProxy | error502`, default error) — new
  behavior, not a new failure *shape* (rule on frozen shapes: reuses fixed
  fail-close error rendering).
- Observability fields go to the access log only; no new metrics without the
  inventory process.
