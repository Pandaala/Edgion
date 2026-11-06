# res-05: EdgionPlugins exposes proxy callback phases as separate arrays instead of letting each plugin own its phases

- **Category**: best-practice
- **Severity**: P3
- **Scope**: large-direction
- **Location**:
  - `edgion-resources/src/resources/edgion_plugins/mod.rs:116-135` (four stage arrays + `waf`)
  - `edgion-resources/src/resources/edgion_plugins/entry.rs:102-540` (five near-identical entry structs)
  - `edgion-resources/src/resources/edgion_plugins/validate.rs:35-133` (stage eligibility table; Wasm response-companion special case)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/{ext_proc.rs:39-119,wasm.rs:276-294}` (response behavior configured from a request-stage entry)

## Summary

Operators must place each plugin in the array matching a Pingora callback (`requestPlugins`,
`upstreamResponseFilterPlugins`, `upstreamResponseBodyFilterPlugins`, `upstreamResponsePlugins`). Of ~55
HTTP plugin types, 49 are request-only, three are header-response-only, one is body-only, and only Dsl/Wasm
span stages. Meanwhile the plugins that genuinely need both directions (ExtProc, Guardrail, Wasm response
hooks) already moved to a different model — one request-stage entry that owns an attached response
companion. The API therefore has two composition models at once, and the phase split buys little.

## Current behavior (evidence)

- `HTTP_PLUGIN_STAGE_EXCEPTIONS` (`validate.rs:99-112`) is the full list of non-request placements; everything
  else is `REQUEST_ONLY`. A misplaced entry is a validation error that fails the route closed.
- ExtProc: response header/body/trailer modes live in the request entry's `processing_mode`
  (`ext_proc.rs:106-119`); "ExtProc response processing is owned by its request-stage entry" (`validate.rs:117-118`).
- Wasm: allowed in request, response-header and response-body stages (`StageSet::new(true,true,true,false)`),
  but `response_stage_plugin_validation_error` rejects a Wasm entry whose `response` hooks are enabled outside
  `requestPlugins` ("Wasm response companions require requestPlugins placement", `validate.rs:35-44`). So a
  response-side Wasm guest can be declared in two different ways with different semantics.
- Dsl: request and response logic are separate entries with separate configs and no shared per-entry state.
- The duplication propagates: five entry structs with copy-pasted `enable/conditions/alias/dye/body` and
  `validate_*` methods, per-stage condition/reference walkers, per-stage dye capabilities, per-stage runtime
  builders, and a four-column eligibility matrix every new plugin must be classified against
  (`tasks/todo/http-plugin-wiring-plan/README.md` counts these wiring sites).

## Industry comparison

- Envoy: an HTTP filter is one config object implementing both `decodeHeaders/decodeData` and
  `encodeHeaders/encodeData`; order is the HCM filter list, and response processing runs in reverse.
- Kong/APISIX: a plugin is listed once; the plugin implements `access`, `header_filter`, `body_filter`,
  `log` phases internally. Ordering is by (overridable) priority.
- Traefik: a Middleware is one object wrapping the handler (request and response).

## Impact

Operator confusion (which array does `Cors`/`ResponseRewrite`/`Wasm` go into?), two inconsistent models for
bidirectional plugins, and a large per-plugin maintenance cost in resources, controller walkers and gateway
builders. It also makes it hard to add a bidirectional plugin without inventing another "companion" shape.

## Suggested direction

Pre-release, so a schema change is allowed:

- Recommended: one ordered `plugins` list. Each plugin type declares (statically, in `edgion-resources`) which
  phases it implements; the gateway builds per-phase chains from that single list (response phases run in
  reverse order, onion model, which matches the existing cross-source precedence). Request/response halves of
  Dsl and Wasm become fields of one entry, matching what ExtProc/Guardrail already do.
- Minimal: delete the empty fourth stage (see res-04) and make every bidirectional plugin use the
  request-entry-owned companion model, removing Wasm's response-stage placement.

## Notes

- `tasks/todo/http-plugin-wiring-plan/` targets mechanical wiring cost (name reuse, factory dispatch); it keeps
  the stage arrays and does not discuss the operator-facing composition model. This finding is the API-level
  counterpart and would remove most of the per-stage wiring it measures.
- No intentional-design page in `skills/04-review/plugin-runtime` or `resources` defends the per-stage arrays
  as an operator contract.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P3 unchanged (direction-level API design).
- `validate.rs:99-112`: only six types are not request-only. Dsl and Wasm are the only multi-stage types.
- `validate.rs:35-44` rejects a Wasm entry with response hooks outside `requestPlugins` ("Wasm response companions require requestPlugins placement"), while still allowing Wasm in both response stages. Two composition models for bidirectional plugins therefore coexist, as claimed.
- `validate.rs:117-118`: "ExtProc response processing is owned by its request-stage entry".
- The minimal option overlaps res-04 (empty fourth stage). Keep both: res-04 is cleanup, this finding is the composition model.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `edgion_plugins/mod.rs`, `entry.rs` and `validate.rs` are unchanged; the four stage arrays and `HTTP_PLUGIN_STAGE_EXCEPTIONS` (`validate.rs:99-112`) remain, as does the Wasm response-companion special case (`validate.rs:35-44`).
