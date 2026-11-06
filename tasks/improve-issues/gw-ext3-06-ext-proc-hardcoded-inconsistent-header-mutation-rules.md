# gw-ext3-06: ExtProc header-mutation policy is hard-coded, differs per phase, and lacks Envoy `HeaderMutationRules` controls

- **Category**: best-practice
- **Severity**: P3 (direction/nice)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/ext_proc/mutation.rs:27-41` (`is_request_proxy_managed_header`, 8 fixed names), `mutation.rs:119-158` (`apply_to_request`: rejected names are logged and skipped), `mutation.rs:161-200` (`apply_to_response`), `mutation.rs:220-247` (`local_reply_ops`), `mutation.rs:129,171,230` (unknown `append_action` silently becomes `AppendIfExistsOrAdd`), `mutation.rs:250-270` (`reject_protected` / `reject_protected_request`), `edgion-gateway/src/plugins/http/ext_proc/response_trailer.rs:277-330` (`is_forbidden_trailer_name`, `apply_mutation`: rejected names or unknown actions fail the whole verdict), `edgion-resources/src/response_header_policy.rs:9-13` (`is_protected_response_header`)

## Summary
Envoy's ext_proc lets operators bound what a processor may change through
`mutation_rules` (`HeaderMutationRules`: `allow_all_routing`, `allow_envoy`, `disallow_system`,
`disallow_all`, `allow_expression`, `disallow_expression`, `disallow_is_error`). Edgion instead has
three hard-coded and mutually inconsistent rule sets inside one plugin, with no configuration:
request headers reject pseudo-headers plus eight framing names and silently skip; response headers
use the shared response policy and silently skip; trailers additionally forbid `host` and fail the
whole verdict (subject to `failOpen`) on the same violation. Unknown `append_action` values are
coerced to append for headers but are a hard error for trailers.

## Current behavior (evidence)
- Request: `reject_protected_request` blocks `:`-prefixed names and `connection`, `keep-alive`,
  `proxy-connection`, `transfer-encoding`, `content-length`, `te`, `upgrade`, `trailer`
  (`mutation.rs:27-41,263-270`). Everything else — including `host`, `authorization`,
  `cookie`, identity headers produced by earlier auth plugins, and any `x-edgion-*` marker — can be
  set, appended or removed. Violations are `log.push_err(...)` + `continue` (`mutation.rs:124-127`).
- Response: `reject_protected` uses `is_protected_response_header` (framing/hop-by-hop + pseudo),
  again skip-and-log (`mutation.rs:250-261`).
- Trailers: `is_forbidden_trailer_name` = response policy **plus `host`**, and any violation returns
  `Err(())`, which `run_upstream_response_trailer` turns into `finish_failure(..., "bad-verdict")`
  (`response_trailer.rs:150-160,277-330`).
- `HeaderAppendAction::try_from(opt.append_action).unwrap_or(AppendIfExistsOrAdd)` for request,
  response and local-reply mutations (`mutation.rs:129,171,230`) versus `.map_err(|_| ())?` for
  trailers (`response_trailer.rs:301`).
- `:path`, `:method`, `:scheme`, `:authority` can never be changed, so a processor cannot perform
  the URL/method rewrites that Envoy allows with `allow_all_routing: true`; there is no opt-in.
- `ExtProcConfig` has no mutation-rules field; the only knob is the global `failureModeAllow`.

## Industry comparison
- Envoy `envoy.extensions.filters.http.ext_proc.v3.ExternalProcessor.mutation_rules` applies one
  `HeaderMutationRules` evaluator to request headers, response headers, trailers and immediate
  responses. Defaults: system/routing headers (`:`-prefixed and `host`) and `x-envoy-*` are
  protected; `allow_all_routing` opts into routing rewrites; regex allow/deny lists narrow the set;
  `disallow_is_error` chooses between ignoring and failing the message, uniformly.
- Istio and Gateway API implementations relying on ext_proc (Envoy Gateway `EnvoyExtensionPolicy`)
  surface these rules to the operator rather than embedding them.

## Impact
- The same processor bug (e.g. writing `host`) is ignored in one phase and fails the request in
  another, which is hard to reason about and to test.
- Operators cannot express "this processor may only touch `x-ai-*` headers" or "never let the
  processor remove `authorization`", a common least-privilege requirement when the processor is
  operated by another team.
- Processors written for Envoy that rewrite `:path` (common for tenant/version routing) silently do
  nothing on Edgion.

## Suggested direction
Introduce one `ExtProcMutationPolicy` evaluator, compiled at config load, used by request headers,
response headers, trailers and immediate-response headers alike:
- fixed floor: pseudo-headers (except opted-in routing fields), framing/hop-by-hop names;
- configurable `allow`/`deny` name lists (exact + prefix, or the shared `Condition` matcher where it
  fits) and an `onViolation: Ignore | Error` switch replacing the per-phase behaviors;
- an explicit opt-in for `:path`/`:method` rewrite before upstream selection, if the route model can
  honour it (otherwise document it as unsupported in one place);
- unknown `append_action` handled identically everywhere.
Keep the configuration small (one block) to avoid over-design; Envoy's expression fields can be
reduced to name lists.

## Notes
- Checked `skills/04-review/ext-proc/regression-guards/` (`opaque-header-fidelity.md`,
  `ext-proc-phase-verdict-matching.md`, `ext-proc-interactive-continue-and-replace.md`): they cover
  value fidelity and verdict matching, not mutation authorization.
- Distinct from `gw-ext-06` (valid no-op verdicts rejected) and `gw-ext-02` (stream cap).
- Request header and trailer framing safety itself is sound: STREAMED body mutation forces chunked
  upstream framing (`routes/http/proxy_http/mod.rs`, `request_relay_plan`).

## Verification (independent)

Verdict: CONFIRMED. Severity P3 kept.
- Request mutations block only pseudo + 8 framing names and skip-and-log (`ext_proc/mutation.rs:27-39,119-135`), so `host` and `authorization` are writable; trailers forbid `host` and fail the verdict (`response_trailer.rs:277-330`).
- `append_action` is coerced with `unwrap_or(AppendIfExistsOrAdd)` for headers (`mutation.rs:129`) but `map_err(|_| ())?` for trailers (`response_trailer.rs:301`).
- No mutation-rules field exists in the ExtProc config. Direction-level consistency/least-privilege gap; P3 is right.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P3 unchanged)
- `ext_proc/mutation.rs`, `response_trailer.rs` and `edgion-resources/src/response_header_policy.rs` are unchanged; `is_request_proxy_managed_header` is still at `mutation.rs:27`.
- The only ext-proc review change is a one-line edit in `opaque-header-fidelity.md`, which does not address mutation-rule consistency.
