---
name: auth-claim-header-projection
description: Use when reviewing JwtAuth/OpenID Connect claimsToHeaders, JweDecrypt payloadToHeaders, destination collisions, aggregate header caps, or array claim formatting.
status: fixed
finding_id: D0052
closed: 2026-09-11
knowledge_type: regression-guards
---

# Authentication Claim Header Projection Is Precompiled and Deterministic

## Conclusion and scope

JwtAuth and OpenID Connect `claimsToHeaders`, plus JweDecrypt `payloadToHeaders`, compile through
`compile_claim_header_projection` before request processing. The plan validates HTTP header names,
canonicalizes destinations to ASCII lowercase, rejects destinations that collide ignoring ASCII
case within one plugin execution path, and sorts entries by canonical destination. OIDC reserves
each enabled fixed output (`X-Access-Token`, `X-ID-Token`, `X-Userinfo`, and `Authorization`), while
JweDecrypt reserves its `forwardHeader`; a configured projection cannot claim the same destination.
Request processing consumes only that immutable plan, never the source `HashMap`.

This correction is local to each executing plugin instance. Separate plugins retain execution-order,
last-write-wins behavior. ISSUE-002 additionally clears every configured projection destination
before bounded projection. Missing, empty, unsupported, unsafe, and oversized values leave no
header. Cleanup covers the entire plan before aggregate-budget exhaustion can stop projection.
Actual removal or projection write errors deny without forwarding. Since 2026-11-09 (user
decision) the deny is the plugin's authentication failure, not a fixed 500: JwtAuth answers its
401 (`invalid_token` when a token was verified, bare challenge for anonymous), JweDecrypt the
decrypt-failure 401 (bypass path: "JWE token required"), OpenID Connect a fixed 401
"Unauthorized - Authentication required", all after the shared failure delay, with push_err
`claim header mutation failed` and the access-log diagnostic
`EdgionErrorCode::AuthHeaderMutationRejected`.
A fixed undelayed 500 right after a valid token told the client by status and timing that the
token was valid; misconfiguration stays visible through the diagnostic. It is not a metric and
one occurrence is not an alert condition: the code is shared with credential-removal failures,
and a client can trigger one by sending a hidden KeyAuth/JwtAuth query credential to a
gRPC-JSON-transcoded route, whose query is sealed. Operators alert on its rate through
log-based alerting; do not re-report the absence of a dedicated metric or per-event alert.
JWT anonymous allowance, OIDC unauthenticated Pass, and JWE non-strict no-token allowance clear
the same destinations. Non-JSON JWE plaintext also clears them.

Disabled or condition-skipped plugins remain inert: this is not Envoy's filter-wide
sanitize-before-bypass contract. No global identity-header family is reserved. Since
2026-11-09 the enabled OIDC fixed outputs (`X-Access-Token`, `X-ID-Token`, `X-Userinfo`, and
`Authorization` in session-cookie mode with `accessTokenInAuthorizationHeader`) are removed on
every continuation (authenticated and `unauthAction: Pass`) before any write, so a skipped
oversized value, or one that is not valid HTTP header text (`is_header_value_safe` uses
`http::HeaderValue::from_str`, so DEL and other control bytes are skipped, not denied), leaves
the destination absent instead of a forged client copy; in bearer mode the verified client
`Authorization` stays. JWE forwardHeader keeps its separate write behavior (a write failure is a
400 "Invalid JWE format": the value is decrypted client data), but the non-strict no-token
bypass removes `forwardHeader` together with the projection destinations. JWT anonymous fixed
identity writes now fail closed on mutation error. Token verification/decryption consumes
borrowed input before projection cleanup, preserving input/output name collisions without
adding a request-path token copy. hideCredentials removal still runs last, but since
2026-11-09 a removal failure (header or query) denies with the same plugin response and the
`credential removal failed` tag instead of forwarding the credential (JwtAuth, OpenID Connect,
JweDecrypt; cookie removal keeps the shared cookie cleanup response).

Follow-up P1-C1 rejects `Host` destinations, ignoring ASCII case, for JWT/OIDC
in their shared Resources authenticated projection compiler. Header-map removal does
not remove HTTP/2 URI authority, and the pinned Pingora adapter recreates Host for an
HTTP/1 upstream. Host therefore cannot serve as an owned claim destination with the
missing-claim absence contract. Use a dedicated identity header instead. The generic
payload compiler and JweDecrypt admission remain unchanged; this is not a general
protocol-header prohibition or an authority-rewriting feature.

Since 2026-11-09 the same authenticated compiler rejects every name in
`auth_realm::RESERVED_AUTH_OUTPUT_HEADER_NAMES` (Host plus the framing/hop-by-hop
names `Content-Length`, `Transfer-Encoding`, `TE`, `Trailer`, `Expect`,
`Connection`, `Keep-Alive`, `Proxy-Connection`, `Upgrade`). Destinations are
cleared on every continuation, so a framing name would change how the upstream
parses the request. KeyAuth/HmacAuth `upstreamHeaderFields` and ForwardAuth
`decision.upstreamHeaders` share the list through `validate_owned_header_names`.

## Rationale and evidence

Before D0052, `set_claims_headers` iterated the operator `HashMap` directly. Case variants such as
`X-Identity` and `x-identity` therefore competed for one HTTP destination with an unstable winner.
When aggregate capacity could not admit every candidate, `break` made the admitted subset depend
on the same unstable order. Array claims also materialized a `Vec<String>` and joined the entire
candidate before applying the per-value cap.

The shared compiler now lives in
`edgion-resources/src/resources/edgion_plugins/plugin_configs/auth_header_projection.rs`. All three
resource validators call it, so Controller and Gateway admission use one collision and header-name
contract. `JwtAuth`, `OpenidConnect`, and `JweDecrypt` compile and retain the resulting plan during
runtime construction. `edgion-gateway/src/plugins/http/common/auth_common.rs` formats arrays
directly into one bounded candidate and commits aggregate bytes only after a sensitive header write
succeeds. A failed projection write now terminates the request. OIDC fixed token/userinfo headers
retain priority and their existing successful-write budget accounting; even zero remaining
projection budget must still invoke the shared helper to clear configured names.

## Alternatives and limits

ARCH-070: HeaderCertAuth's certificate source and four configured upstream header
names, plus HmacAuth's upstreamHeaderFields, use the canonical Resources HTTP token
check during shared Controller/Gateway admission. Existing empty/control checks
retain precedence; new diagnostics identify only the field or index. HMAC metadata
selection preserves exact whitelist spelling, and signed-header pseudo names are
outside this check. That change did not adopt claim-projection normalization,
collision rules or sanitization; preserving skipped writes was historical scope, not
acceptance of client input on active certificate outputs. ISSUE-003 subsequently clears
HeaderCertAuth's active destinations, including absent SAN and anonymous continuations;
see the [certificate projection guard](../../header-cert-auth/regression-guards/certificate-identity-header-projection.md).
HMAC behavior remains outside that correction. Character admission is not an additional
header-length policy.

- Appending or joining competing claims into one header was rejected because the mapping expresses
  one source per destination; implicit multi-value semantics would change what an upstream
  authorization service receives.
- Sorting the `HashMap` on every request was rejected because configuration order is independent of
  request data and belongs in runtime construction.
- Cross-plugin destination ownership remains out of scope. Later plugins overwrite earlier writes
  through the existing replace-all-lines session contract.
- The deterministic aggregate policy is canonical-destination order. A candidate that cannot fit
  the remaining total budget stops later plan entries, preserving the former stop-on-cap shape with
  a stable admitted set.
- Claim paths remain case-sensitive. Only destination header names are canonicalized.
- Array projection preserves scalar filtering, source element order, comma formatting, and UTF-8
  byte accounting. It discards the entire candidate when the next append would exceed the cap; it
  never truncates.

## Verification and re-evaluation

Historical D0052 verification: targeted resource tests covered canonical ordering, invalid destinations, and case-insensitive
mapping collisions through all three configuration validators, all four enabled OIDC fixed-output
collisions, and the JweDecrypt `forwardHeader` collision. Targeted Gateway tests cover exact and
plus-one per-value/aggregate boundaries, mixed array formatting, failed-write budget accounting,
sensitive lowercase writes in JwtAuth/OIDC/JweDecrypt, and JweDecrypt construction order.

ISSUE-002 verification in this run:

- Targeted Gateway JWT, shared auth helper, OIDC and JWE tests: 296 passed, 16 existing
  ignored tests, zero failures. Real Pingora adapters verify duplicate-header removal,
  absent/unsafe/oversized values, aggregate exhaustion, input/output name collisions,
  and anonymous cleanup. Failure-injection tests verify static fail-closed responses.
- `run_integration.sh --no-prepare -r EdgionPlugins -i JwtAuth` passed, including
  `JwtClaimHeadersDynamic`: A-to-B-to-A mapping reload, upstream echo of missing/present
  claims and anonymous requests, and unrelated/unowned header preservation. Final metrics,
  unknown-field, requeue and cache-residue gates passed.
- Follow-up independent review found and corrected the missing Kubernetes dynamic
  registration: the K8s manifest and three matching fixtures now include
  `JwtClaimHeadersDynamic`. Scenario enumeration, fixture identity/equivalence checks,
  suite-parity regression tests, dynamic-runner tests and main-runner wiring tests passed.
  That review also identified five pre-existing parity gaps (DslQueryLookupDynamic,
  ProxyRewriteHostDynamic, ResponseCacheDynamic, SharedHttpPolicyAdmission, and static
  EdgionPlugins/ResponseCache). A subsequent user-authorized repair added their K8s
  fixtures and executable registrations, selector-owned Service target changes,
  all-replica origin observations and an explicit Gateway process-restart guard.
  Repository-wide parity now passes with zero missing canonical entries. Live
  Kubernetes execution was not performed; runner regression tests cover the wiring.
- All nine project quick checks passed (format, workspace check/clippy and six Python
  guards). Clippy reported existing warnings outside this change.
- A broader HTTP-plugin test invocation was not clean: 2078 passed and 63 unrelated
  Wasm tests failed, including preparation-capacity exhaustion under concurrent execution.
  One representative failed Wasm case passed in isolation. This is not evidence of a
  passing full test suite; the targeted authentication run above is the verified scope.

Re-evaluate this guard if the configuration changes from one-source-per-destination mappings to an
ordered list with explicit append/merge semantics, if aggregate selection gains an operator-defined
priority, or if plugin composition adopts cross-plugin header ownership. D0052's historical
verification did not cover sanitization; ISSUE-002 supplies that correction for executing plugins.
Re-evaluate cleanup ordering if credential inputs or plugin bypass semantics change.

## Resolution references

- `D0052` — fixed 2026-09-11; the closed todo was migrated into this regression guard.
- `ISSUE-002` — configured JWT identity-header spoofing: clear configured destinations before
  projection, including no-claim successful continuations; fail closed on mutation errors.
- `P1-C1` — Host admission follow-up: Resources tests cover case variants, normal
  identity destinations and unchanged JWE admission; Gateway tests compare shared
  validation and runtime compilation errors for otherwise valid JWT/OIDC resources.
  The trigger was established statically through pinned Pingora revision
  `1aaa45cc70430bc782990bc20ddee3fef36d6b99`, `proxy_h1.rs` and
  `proxy_common.rs::set_h1_host_from_authority`. No transport change is claimed.
