---
name: http-grpc-route-regex-matching-no-fallback
description: Use when reviewing findings about hot-path Regex::new() fallback in match_header or match_query_param, or suggesting adding runtime regex compilation to the match path.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# HTTP/gRPC Route Regex Matching: Controller Validates + No Fallback on Hot Path

**Historical false positive**: in `match_header` / `match_query_param`, there used to be a fallback branch that called `Regex::new()` when `compiled_regex` was `None`; the audit repeatedly flagged it as a hot-path performance issue.

**Defense at the time**: claimed "the route is uniformly precompiled at creation; the fallback branch only fires on a config bug; normal runtime does not trigger".

**Flaw in the defense**: at the time, the Controller-side `validate_http_route_if_enabled` / `validate_grpc_route_if_enabled` **only validated the `match_type` enum value**; it did not actually try `Regex::new()`. Illegal regex could pass through to the data plane unchecked; `compile_match_regexes` used `Regex::new(...).ok()` to silently swallow the error; ultimately every request would land on the fallback and recompile + refail — both hiding the config error and paying hot-path cost.

**Historical fix (2026-04-25)**:
1. On the Controller side, `ref_grant/validator.rs::check_regex_pattern` is now invoked inside `validate_*_route_if_enabled` to try `Regex::new()` for all `RegularExpression` fields (HTTPRoute path/header/query, GRPCRoute method.service/method.method/header); failures are pushed to errors → route `Accepted=False`.
2. On the Gateway side, `match_unit.rs::match_header` / `match_query_param` (HTTP & gRPC) **runtime fallback removed**; `compiled_regex == None` returns `Ok(false)` directly, fail-closed.
3. On the Gateway side, `compile_match_regexes` still keeps `Regex::new(...).ok()` (defense against controller bugs); on failure it loudly alerts via `tracing::error!`, not silent.

**Decision criteria during review**:

**Current GRPCRoute method policy (2026-09-13)** supersedes the route-wide
method-validation statement above: `GRPCRouteMatch::compile_method` is shared by
Controller per-rule classification and Gateway compilation. An invalid method
pattern drops the entire rule, preserving valid sibling rules; all-invalid rules
reject attachment. Header validation remains in `validate_grpc_route_if_enabled`.
The compiled method outcome explicitly distinguishes Any, Exact, regex, and
invalid; neither failed regex field can become Exact or catch-all. See the
[matching regression guard](../../routing/regression-guards/grpc-method-matching-and-precedence.md).
No regex is compiled during request matching. Configuration validation alone is
not proof that a status-less or stale payload cannot reach the Gateway.
- See no `Regex::new` call inside `match_header` / `match_query_param` → already in the post-fix state, not an issue.
- See `Ok/Err` two-arm with `tracing::error!` inside `compile_match_regexes` → this is the data plane's last line of defense against controller bugs, not a performance issue.
- Controller `check_regex_pattern` calls establish validation only for the fields
  and ingestion paths actually covered. Keep Gateway fail-closed checks for
  status-less or stale input; do not infer that malformed payloads are unreachable.

**Verdict**: in the current implementation the fallback has been completely removed; any "suggest adding runtime Regex::new to match_header" finding is a reverse regression.
