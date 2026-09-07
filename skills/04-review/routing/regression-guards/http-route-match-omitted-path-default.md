---
name: http-route-match-omitted-path-default
description: Use when reviewing HTTPRouteMatch findings about an omitted `path`, empty `extract_paths()`, unreachable radix route units, or File/Etcd-vs-Kubernetes routing asymmetry; covers where the Gateway API structural default must be replicated.
status: fixed
finding_id: gw-http-match-01
closed: 2026-07-27
knowledge_type: regression-guards
---

# HTTPRouteMatch with an omitted `path` — FIXED at the deserialization layer

## Conclusion

`HTTPRouteMatch.path` is optional per the Gateway API spec and defaults to
`{type: PathPrefix, value: "/"}` when omitted. That default used to exist only
in the Kubernetes API server's structural-schema defaulting, so the File and
Etcd config-center backends — which never pass through admission — kept
`path: None` all the way into route building, where `HttpRouteRuleUnit::extract_paths()`
returned an empty `Vec` and the radix engine created no tree entry pointing at
the unit. The rule compiled, reported `Accepted=True`, and was unreachable at
every path. Fixed by replicating the default in `edgion-resources`
(`default_path_prefix_root`, a `#[serde(default = "...")]` on the field), so
every config-center backend now deserializes to the same value the API server
would have written.

Fix suggestions of the form "patch `extract_paths()` to fall back to `/`",
"special-case `is_regex_path`/`build_normal_route_unit`", or "add the default
in `append_route_to_hostname`" are **not accepted** — they fix routing only and
leave the other four `path` consumers still seeing `None`.

## Core Rationale

**1. The defect is a missing structural-schema replication, not a routing bug**

The gateway's `extract_paths()` behavior (no path value → no radix entry) is
correct for its contract; the wrong input is `path: None` reaching it at all.
The K8s CR path never produces it (vendored
`config/crd/gateway-api/gateway-api-standard-v1.5.0.yaml` declares
`path: default: {type: PathPrefix, value: /}`), so the origin is the one layer
that all backends share and that none of them defaulted: deserialization in
`edgion-resources/src/resources/http_route.rs`. Anything narrower is a
symptom-level patch.

**2. `path` has five consumers, not one**

Beyond `match_unit.rs::extract_paths` (radix insertion) and
`conf_handler_impl.rs::is_regex_path` (engine selection), the resolved match
item is read by `gateway_api_filters/url_rewrite.rs` and
`gateway_api_filters/request_redirect.rs` (both need the prefix for
`ReplacePrefixMatch`) and by `plugins/runtime/session_adapter.rs`. A
gateway-routing-only fix leaves three of them on the `None` branch. Defaulting
at deserialization fixes all five at once and keeps File/Etcd byte-identical to
what a K8s cluster would have served.

**3. The controller's two `None` branches — CORRECTED 2026-07-29**

The original closure claimed both delegation sites were unaffected. That was
true only for the two cases it checked, and it missed a third:

- `route_delegation.rs` scope derivation maps `None => ("PathPrefix", "/")` —
  literally the same value the new default produces, so it is a no-op change.
  (Still true.)
- `route_delegation.rs` synthesizes an `HTTPRouteMatch { path: None, .. }`
  **in code** for a child rule that declares no `matches` at all. Code-built
  values never pass through `Deserialize`, so that sentinel stays `None`.
  (Still true.)
- **Missed**: a child rule that DOES declare `matches` whose individual match
  omits `path` (method/header-only). Those matches ARE deserialized, so the
  backfill turned them into an explicit `PathPrefix /`, the containment check
  judged `/` outside every delegated prefix, and the rule was dropped —
  `child_match_without_path_inherits_prefix` sat red from this commit until
  the two-repo `cargo test` gate caught it. Fixed by treating the structural
  default as inherit-scope-prefix in `merge_match`; see
  [delegation-root-prefix-inherits-scope.md](delegation-root-prefix-inherits-scope.md).

The single behavior delta is a File/Etcd child rule that *explicitly declares*
`matches` with a `path`-less item: it now becomes `PathPrefix /`, fails the
scope-containment check, and is dropped — exactly what the same YAML already
did on Kubernetes. Aligning to K8s is the point of the fix.

## Fix Suggestions Not Accepted

- Fall back to `/` inside `extract_paths()` — fixes radix reachability only; `url_rewrite`, `request_redirect`, and `session_adapter` keep reading `None`.
- Materialize the default in `append_route_to_hostname` / `append_match_to_domain` — same partial coverage, plus it puts a spec default in the data plane instead of the schema layer.
- Add the default to the vendored Gateway API CRD — the CRD is upstream-mirrored (`config/crd/gateway-api/`) and already carries it; the CRD is not what File/Etcd sources read.
- Delete the two controller delegation `None` branches as "now dead" — the synthesized-sentinel branch is still reached (see rationale 3); removing it changes delegation semantics for a child rule with no `matches`.

## Re-evaluation Triggers

Re-open this decision only if:

- The Gateway API changes the documented default for an omitted `HTTPRouteMatch.path`.
- A caller starts needing to distinguish "operator omitted `path`" from "operator wrote `PathPrefix /`" (the default erases that distinction after deserialization).
- The same File/Etcd-vs-admission asymmetry is confirmed for another structural default on a route resource — GRPCRoute's `method`/`service` match defaulting is the flagged sibling and was **not** audited by this fix.

## Reference Cases

- `gw-http-match-01` (2026-07-27) — original finding under `tasks/fable-review/`, disposition `confirmed`, Medium.
- `edgion-resources/src/resources/http_route.rs` — `default_path_prefix_root` and the field attribute; unit test `match_without_path_defaults_to_path_prefix_root`.
- `edgion-tests/integration/conf/HTTPRoute/Match/HTTPRoute_default_match-test.yaml` rule 10 + `edgion-tests/src/client/suites/http_route/match/matches.rs::test_omitted_path_defaults_to_root_prefix` — FileSystem-mode e2e coverage (the K8s path could not have caught this).
- `docs/en/user-guide/http-route/matches/README.md` — the documented "omitted `path` means `PathPrefix /`" contract.

## Resolution references

- `gw-http-match-01` — 2026-07-27.
