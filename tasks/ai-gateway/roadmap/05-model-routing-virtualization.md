# WS-05: Model Routing Virtualization

**Goal:** Model-level routing indirection matching agentgateway's virtual models
(`model_router.rs:95`): weighted / failover / conditional routing *per model
name*, wildcard aliasing, `/v1/models` synthesis, header-based model selection —
layered on our existing (already richer) candidate-selection machinery.

**Today:** aliases are flat per-service (`edgion_ai_service.rs:106-222`);
selection algorithms (Weighted/Priority/LowestLatency/LowestUsage/CostWeighted)
operate over route backendRefs (`ai/routing.rs:115-318`); no cross-service
model indirection; `onUnknownModel` Reject/Passthrough.

## Architecture

- New optional cluster of config on `AiProxyConfig`: `modelRoutes` — an ordered
  list of model-pattern rules evaluated after `canonical_openai_request`
  extracts the requested model and before candidate resolution:

  ```yaml
  modelRoutes:
    - match: { model: "gpt-4*" }          # wildcard, specificity-sorted
      action:
        rewrite: "gpt-4o"                  # optional canonical rewrite
        candidates:                        # optional narrowing/weighting override
          - service: openai-primary        # backendRef name
            weight: 90
          - service: azure-fallback
            weight: 10
    - match: { model: "cheap", header: {name: "x-tier", value: "free"} }
      action: { rewrite: "gpt-4o-mini" }
  ```

  Failover stays the existing scheduler's job (services in a rule = candidate
  set; Priority algorithm = failover semantics). Conditional routing phase 1 is
  header+model match only — **no expression language** (decision recorded:
  agentgateway uses CEL here; our structured matchers cover the demonstrated
  use cases; a DSL hook can come later if users ask).
- Wildcard aliasing: extend per-service alias tables to allow `*` suffix
  patterns with longest-prefix specificity ordering, validated for ambiguity at
  admission.
- `/v1/models` synthesis (with WS-02's ingress endpoint): union of the route's
  resolved candidate models post-modelRoutes, with alias visibility flags
  (`public: true|false` per model entry on the CR).
- Downgrade action (`onExceeded: Downgrade`) reuses modelRoutes rewrite
  machinery once present (single rewrite implementation).

## File map

```
Create:
edgion-gateway/src/ai/model_router.rs      # ModelRouteTable: compile-from-config,
                                           #   specificity sort, match(model, headers)
                                           #   -> ModelRouteDecision {rewrite, candidate
                                           #   filter/weights}

Modify:
edgion-resources/.../plugin_configs/ai_proxy.rs   # modelRoutes schema + validation
                                           #   (pattern syntax, ambiguity, candidate
                                           #   refs must be route backendRefs)
edgion-resources/src/resources/edgion_ai_service.rs  # alias wildcard patterns +
                                           #   per-model `public` flag
edgion-gateway/src/plugins/http/ai_proxy/plugin.rs   # invoke model router between
                                           #   canonical parse and candidate resolve;
                                           #   thread decision into resolve_candidate_models
edgion-gateway/src/ai/ingress/models.rs    # (WS-02 file) list synthesis honors flags
edgion-gateway/src/ai/routing.rs           # accept candidate filter/weight overrides
edgion-tests                               # scenarios: wildcard precedence, header
                                           #   match, rewrite + quota identityModel
                                           #   interaction, /v1/models output
```

## Phases

1. `ModelRouteTable` + rewrite action + wildcard aliases (pure request-side).
2. Candidate filter/weight overrides threaded through `routing.rs`.
3. `/v1/models` visibility integration (with WS-02) + docs.

## Interactions to test explicitly

- Quota scope `identityModel` keys must use the **post-rewrite** model
  (consistent with today's Downgrade re-admission behavior in
  `ai_proxy/plugin.rs:362-424`).
- Adaptive telemetry (`adaptive.rs`) keys include model — rewritten model must
  be the telemetry key, or LowestLatency data fragments.
- `onUnknownModel: Passthrough` bypasses modelRoutes only if no rule matched;
  a matched-then-rewritten model re-resolves normally.
