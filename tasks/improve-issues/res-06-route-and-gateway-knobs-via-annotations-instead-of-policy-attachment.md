# res-06: Route/Gateway-scoped extension knobs live in ~34 untyped annotations instead of a targetRef policy (GEP-713)

- **Category**: best-practice
- **Severity**: P3 — downgraded from P2 by independent verification
- **Scope**: large-direction
- **Location**:
  - `edgion-resources/src/constants/annotations.rs:11-145` (operator annotations: HTTP/2, redirect, PROXY protocol in/out, forward-raw-path, retry-non-idempotent, 7× mirror-*, max-connect-retries, 3× tcp-keepalive-*, require-sni-host-match, plugin attachments, 4× DNS, local-bind-ip, ...)
  - `edgion-resources/src/resources/http_route_preparse.rs:32-180` (annotation parsing; invalid values → `tracing::info!` + default)
  - `edgion-resources/src/resources/http_route.rs:216-252` (`parsed_*` fields justified by "the vendored Gateway API CRD silently prunes undeclared filter fields")
  - `edgion-resources/src/resources/edgion_backend_traffic_policy.rs:45-60, 115-125` (the only targetRef policy; targets only `Service`/`EdgionBackend`, no `sectionName`)
  - `skills/02-features/05-annotations/references/annotations-metadata.md` (34 operator-facing keys)

## Summary

Edgion has exactly one policy-attachment resource (EdgionBackendTrafficPolicy) and it can only target backends.
Every route-, listener- or Gateway-scoped extension that Gateway API lacks is therefore added as an
`edgion.io/*` annotation: retries of non-idempotent requests, mirror buffering/timeouts/concurrency, raw-path
forwarding, L4 connect retries, upstream TCP keepalive, PROXY protocol, HTTP/2 enablement, HTTPS redirect,
SNI/Host enforcement, DNS resolver overrides, plugin attachment. The registry now lists 34 operator-facing
keys. Annotations are untyped, not schema-validated, invisible to `kubectl explain`, and most invalid values are
logged at INFO and silently replaced by defaults with no status condition. Some features are simply not
configurable in Kubernetes mode (e.g. session-persistence `strict`, which the vendored CRD prunes).

## Current behavior (evidence)

- `HTTPRoute::preparse` reads `forward-raw-path`, `retry-non-idempotent` and seven `mirror-*` keys; any
  unparsable value is `tracing::info!(.. "ignored: invalid value, using default")` and becomes the default
  (`http_route_preparse.rs:37-178`). `parse_mirror_tuning_annotations` even normalizes invalid capacity
  combinations silently. Nothing is reflected on the HTTPRoute status.
- The code comments state the reason explicitly: these knobs "live in annotations rather than on
  `HTTPRequestMirrorFilter` because the vendored Gateway API CRD is a structural schema that silently prunes
  undeclared filter fields" (`annotations.rs:66-71`, `http_route.rs:242-249`).
- Annotations apply to the whole object (all rules of a route; all listeners of a Gateway); there is no
  per-rule/per-listener scoping (`preparse` copies one value into every rule).
- `inbound-proxy-protocol` carries a strict JSON document inside an annotation value; `edgion-plugins` /
  `edgion-stream-plugins` carry object references (see res-01 for the authorization gap this creates).
- EBTP `PolicyTargetRef` has `group/kind/name` only; per-port policy on a multi-port Service is impossible,
  and route/Gateway targets are rejected (`target_kind()` accepts only `Service`/`EdgionBackend`).

## Industry comparison

- Envoy Gateway: `BackendTrafficPolicy` (retries, timeouts, circuit breaking, LB, health check, TCP keepalive,
  PROXY protocol to upstream), `ClientTrafficPolicy` (HTTP/2/HTTP/3, keepalive, client timeouts, PROXY protocol
  inbound, XFF/client IP, TLS options) and `SecurityPolicy` all use GEP-713 `targetRefs` with `sectionName`
  to Gateway, listener, xRoute and rule; merge/override precedence and `PolicyAncestorStatus` are defined.
- Istio and Traefik also keep such settings in typed CRDs (DestinationRule/EnvoyFilter; Middleware /
  ServersTransport). The Gateway API project itself moved away from annotations because Ingress's annotation
  sprawl was the main portability problem — Edgion's own whitepaper makes this argument
  (`docs/en/promotion/whitepaper.md:47`).

## Impact

- Typos and invalid values are silent (INFO log on the controller, no condition) — e.g. a mistyped
  `mirror-max-buffered-bytes` quietly uses 1 MiB; `retry-non-idempotent: True` is ignored.
- No RBAC separation: whoever can edit a route can set every knob; a platform team cannot own retry/mirror
  limits separately from application owners (GEP-713's main motivation).
- Granularity is wrong for several knobs (whole route instead of rule; whole Gateway instead of listener).
- Each new knob adds parse code, docs and a review-knowledge page, and K8s-mode users cannot reach features
  that FS/etcd users can.

## Suggested direction

- Recommended: introduce two typed GEP-713 policies, mirroring Envoy Gateway's proven split, instead of a
  per-feature CRD: an `EdgionRouteTrafficPolicy` (targets HTTPRoute/GRPCRoute/TCPRoute/TLSRoute with optional
  `sectionName` = rule name; carries retry-non-idempotent, mirror tuning, raw-path, connect retries, upstream
  keepalive/PROXY protocol, session-persistence strict) and an `EdgionClientTrafficPolicy` (targets Gateway with
  optional listener `sectionName`; HTTP/2, HTTPS redirect, inbound PROXY protocol, SNI/Host enforcement, DNS
  overrides). Reuse the existing `PolicyAncestorStatus`/`prepared_precedence` machinery already built for EBTP
  and BackendTLSPolicy. Add `sectionName` to EBTP targets.
- Delete the corresponding annotations once migrated (pre-release, no alias period), keeping annotations only
  for controller/system markers.
- Minimum if the above is deferred: surface every invalid annotation value as a status condition
  (`Accepted=True` + an `InvalidAnnotation` condition/message) instead of an INFO log.

## Notes

- `skills/04-review/routing/intentional-design/session-persistence-two-state-inheritance-and-k8s-strict-scope.md`
  rejected an `EdgionSessionPersistencePolicy` for a single feature because demand did not justify "a whole new
  resource type + targetRef/sectionName resolution + precedence". This finding presents the aggregate
  evidence that page did not weigh: ~20 route/listener-scoped knobs already carried by annotations, silent
  invalid-value handling, and the policy-precedence machinery already existing for EBTP/BackendTLSPolicy. It
  proposes one general mechanism, not a per-feature CRD.
- `skills/00-design-rules.md` §L prefers Gateway API semantics first; GEP-713 *is* the Gateway API extension
  mechanism, while §O only governs how annotations are added, not whether they are the right carrier.

## Verification (independent)

- **Verdict**: DOWNGRADED — P2 → P3 (direction).
- The facts are accurate: `http_route_preparse.rs:37-58` logs invalid values at INFO and falls back to the default, and `annotations.rs` declares 37 constants.
- The project canon deliberately chooses annotations for per-route knobs on standard kinds. `skills/00-design-rules.md:314` (🔴: a new per-route knob is an annotation parsed in `preparse()`, never a new spec field) and §O (`:212-213`: documented carrier choice; per-key parsing strictness is intentional) both say so.
- `routing/intentional-design/session-persistence-two-state-inheritance-and-k8s-strict-scope.md:20,59-60` explicitly declined a policy-attachment resource for lack of demand.
- The aggregate argument for GEP-713 policies is a legitimate strategic proposal, not a defect. The concrete, cheaper sub-item worth acting on is surfacing invalid annotation values as a status condition instead of an INFO log.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `constants/annotations.rs`, `http_route_preparse.rs`, `http_route.rs`, `edgion_backend_traffic_policy.rs` and the annotations reference are unchanged; invalid annotation values still fall back with an INFO log.
- `skills/00-design-rules.md` changed only a dialer name (fred→redis-rs); the annotation-carrier rule cited in the verification still stands.
