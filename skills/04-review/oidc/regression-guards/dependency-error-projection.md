---
name: oidc-dependency-error-projection
description: Review OIDC dependency error producers, static public responses and diagnostics, authentication fallback, and response delivery boundaries.
knowledge_type: regression-guards
---

# OIDC dependency error projection

## Conclusion and scope

OIDC dependency failures carry bounded operation and failure classifications,
not provider text. Public response selection is separate from diagnostics and
from the caller's authentication/fallback decision. An infrastructure failure
that the caller rejects as 502 has the exact HTTP body `502 Bad Gateway`, no
Bearer challenge, and no authentication-failure delay.

The error boundary covers discovery, JWKS, authorization URL construction,
token exchange, refresh, introspection, userinfo, and revocation. Parsing,
transport, HTTP status, TLS, and endpoint errors must not carry URLs, addresses,
provider bodies, parser messages, tokens, claims, cookies, or credentials into
public messages or logs. Constructor and prewarm diagnostics use safe categories
as well; the shared TLS/dial policy itself is unchanged.

## Rationale and evidence

Canonical authority: [graded design rules](../../../00-design-rules.md), sections
A, B, C, D, and F, and the [fail-closed body rule](../../security/mandatory-design/fail-closed-error-body-no-internal-detail.md).

The source task `redact-openid-connect-dependency-error-responses.md` (Issue field
`N/A`; referenced as D0058 by the discovery-backoff guard) tracked a real defect:
`OidcVerifyError::Other { status, message: String }` returned its message through
`user_message`, and callback/request-filter response writers emitted it directly.
Token exchange and introspection non-success responses exposed provider status;
authorization URL parsing and JWK key construction exposed parser/dependency text.
Discovery additionally formatted provider errors into request logs.

Refresh produced similar strings, but its session callers normally consumed the
failure before fallback. That producer is part of the error-model correction,
not proof that every refresh error was publicly exposed. Previously fixed
transport/egress and authentication-body-reader branches already used generic
responses; their existence did not close the remaining projection defect.

## Alternatives and limits

- Replacing only the final 502 message leaves arbitrary diagnostic strings and
  401 key-construction errors unprotected. Classify at the producer and expose
  only static public messages.
- Do not infer infrastructure status from every error encountered. Auto mode
  preserves its original invalid-JWT 401 after failed introspection; the bounded
  diagnostic retains failure ownership independently.
- A failed preemptive refresh does not invalidate a still-valid token. Session
  recovery continues through the existing `unauthAction` and refresh-wait leeway
  paths. Callback and ordinary Bearer scope failures retain their existing
  response routing.
- Discovery failure backoff, JWKS stale-cache publication, Dye outcomes, and SAN
  metrics retain their existing ownership. Replaying a cached error must not
  increment the metric for a new handshake that never occurred.
- Userinfo and revocation remain best-effort. Their safe failure category does
  not authorize rejecting an otherwise accepted request or abandoning logout.
- Fixed 502 replies use the runtime's `ErrResponse` path. Dynamic authentication
  challenges and cookie redirects reuse existing writers; a failed delivery
  never starts a second response or resumes proxying.
  On a gRPC route, the canonical runtime maps 502 to `INTERNAL` and carries the
  same static `502 Bad Gateway` text as `grpc-message`.
- OIDC JSON readers compose the canonical `stream_body_capped` primitive, reject
  truncation before parsing, and retain each endpoint's original byte limit.
  This is not a new cache, outbound client, retry policy, or response buffer.
- No new metrics, request tracing, schema fields, or compatibility aliases.

## Verification and re-evaluation

On 2026-09-14, `cargo test -p edgion-gateway --lib
plugins::http::openid_connect -- --include-ignored` passed all 157 tests,
including the 16 explicitly enabled local-network cases. The new
`dependency_error_tests.rs` and `dependency_error_integration.rs` cover provider
status/parser failures, endpoint admission, configured caps, fixed 502 projection,
Auto 401, callback 401 versus Bearer 403, delay-before-write, terminal writer
failure, all session `unauthAction` branches, and best-effort userinfo/revocation
and preemptive refresh. Provider-local coverage verifies that replaying a cached
SAN failure does not increment the handshake metric.

The freshly built Gateway test binary also passed three `plugin_runtime` tests
filtered by `err_response`, its `self_written_writer_failure_records_client_closed`
test, and all 14 `runtime::server::error_response::tests`. These cover canonical
gRPC projection, failed delivery, and termination fallback. OIDC network fixtures
exercise real IdP producers with plugin/mocked-session boundaries; they are not
a complete deployed Gateway/Controller integration sweep.

`cargo fmt --all`, `cargo check --workspace --all-targets`,
`cargo clippy --workspace --all-targets`, agent-doc consistency, SSA-force, metrics
inventory, Pingora source policy, Gateway no-tracing, and `git diff --check`
passed. Clippy retains the unrelated existing `redundant_async_block` warning in
`edgion-tests/src/client/suites/edgion_plugins/dsl/grpc_error.rs`; this fix has no
remaining Clippy warnings. No full workspace test suite was run.
Independent read-only review returned LGTM after checking the final code,
runtime response boundaries, and knowledge/documentation changes.

Re-evaluate when adding an OIDC endpoint or error variant, changing response
writers or authentication fallback, or altering provider cache/metric ownership.
The no-detail rule also applies to Debug and Display output of retained errors.

## Resolution references

### Secret material ownership (ARCH-157)

Generic Secret lifecycle processing no longer treats every EdgionPlugins reverse
dependency as an OIDC Secret. Such edges carry wakeup identity, not plugin or
owning-field semantics. OIDC's bound-field resolver retains separate client/session
alias sets and the canonical `read_secret_utf8` contract after authorization.
No replacement warning or provider failure/fallback behavior is introduced.

The Secret-handler regression first resolves an enabled BasicAuth entry, rotates
its Secret, captures actual lifecycle events without an OIDC warning or credential
values, verifies the dependent queue item, and resolves the updated password.
The three removed helper-only tests exercised the obsolete generic alias union;
normalization, initialization and delete/recreate coverage remain intact.

### Existing dependency projection references

- `redact-openid-connect-dependency-error-responses.md` / D0058: dependency
  projection task; original disposition pending, corrected and verified on
  2026-09-14.
- Production owner: `edgion-gateway/src/plugins/http/openid_connect/`.
- [Authentication response byte caps](authentication-response-byte-caps.md).
- [Discovery failure backoff](discovery-failure-backoff.md).
- [Dial-time egress policy](dial-time-egress-policy.md).
