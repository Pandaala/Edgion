---
name: certificate-identity-header-projection
description: Use when reviewing HeaderCertAuth SAN/DN projection, missing certificate fields, anonymous continuation, or identity header mutation failures.
knowledge_type: regression-guards
---

# Certificate identity destinations exclude untrusted input

## Conclusion and scope

Executing HeaderCertAuth instances clear their active output names before publishing identity:
`upstreamHeaders.dnHeader`, `sanHeader`, and `fingerprintHeader` with `skipConsumerLookup: true`;
otherwise `consumerHeader` and `fingerprintHeader`. Cleanup removes all input field lines,
including on anonymous continuations. An absent SAN or a certificate value that is not valid
HTTP header text (`HeaderValue::from_str`; since 2026-11-09, formerly only CR/LF/NUL) must not
preserve a client-supplied value: in consumer mode it is an invalid identity
(`allowAnonymous` continuation when allowed), in `skipConsumerLookup` mode the DN/SAN field is
skipped, and the fingerprint is skipped. Certificate bytes therefore never reach the write as a
mutation error. Distinct output names express distinct certificate fields.

Removal and write errors (only configuration-driven now: invalid or gRPC-transcoder-sealed
names) deny through the plugin's `reject` path — the configured `errorStatus`, the shared
failure delay, push_err `certificate header mutation failed`, and
`EdgionErrorCode::AuthHeaderMutationRejected` — including anonymous identity writes and
source-credential removal (since 2026-11-09; formerly a fixed undelayed 500, changed by user
decision so the response matches an authentication failure and does not reveal a valid
certificate by status or timing). They cannot enter `allowAnonymous` fallback.
Authentication/identity errors retain the existing authentication-failure policy. Static
PluginLog errors carry no certificate, header name/value, or underlying adapter detail.

## Rationale and evidence

ISSUE-003 reported a trusted CN-only client authenticating in Connection mode while sending
a forged custom SAN destination. The old `apply_headers` skipped `sans.first() == None`,
returned success, and left that input available to upstream authorization.
TLS whitelist validation does not require SANs unless a SAN allowlist is configured.
Handshake metadata extraction and HTTP `capture_tls_metadata` therefore legitimately carry
an empty `ClientCertInfo.sans` into the plugin.

The correction belongs to `edgion-gateway/src/plugins/http/header_cert_auth/plugin.rs`.
It extracts the certificate before cleanup so Header-mode input/output aliasing does not
erase the credential before verification. It clears all active names before any write to
avoid erasing earlier trusted writes when destinations alias. `HeaderApplicationError`
separates identity selection failure from mutation failure, preventing anonymous fail-open.
No resource schema, public API, TLS verification rule, or cross-plugin order changes.

## Alternatives and limits

The [per-plugin ownership decision](../../security/intentional-design/auth-identity-headers-overwrite-written-names.md)
does not accept untrusted values on active certificate outputs. This fix does not reserve
a global identity-header family or sanitize disabled/condition-skipped plugin paths.
Inactive DN/SAN names in consumer mode and the consumer name on successful direct projection
remain unowned. Later plugins retain ordinary execution-order mutation semantics.

Output aliases retain existing last-write behavior: DN, SAN, fingerprint; or consumer,
fingerprint. Anonymous marker/consumer writes can also alias configured destinations.
Such aliases carry the final trusted write, not necessarily a value of the destination's
nominal certificate field. Source hiding still removes the source name after projection.
Use distinct names when separate fields or retained output are required.

Envoy's analogous XFCC `SANITIZE_SET` reconstructs the complete header from the current
certificate, so missing SAN fields never preserve the old input. Its explicit forwarding
modes have separate semantics; they do not justify accidental pass-through here.
See [Envoy XFCC implementation](https://github.com/envoyproxy/envoy/blob/main/source/common/http/conn_manager_utility.cc).

## Verification and re-evaluation

Fresh verification for ISSUE-003:

- `cargo test --locked -p edgion-gateway --lib plugins::http::header_cert_auth`:
  26 passed. Real Pingora request headers cover duplicate/missing/unsafe output values,
  inactive destinations, anonymous continuation, sensitive values, and input/output aliases.
  Failure injection covers removal, certificate/anonymous writes, and source hiding.
- `cargo test --locked -p edgion-tests --lib header_cert_headers`: 2 passed,
  validating generated client trust/SAN differences and duplicate-header observation.
- `run_integration.sh --no-prepare -r EdgionPlugins -i HeaderCertAuth`: PASS with freshly
  built binaries. Existing Header-mode cases and `HeaderCertHeadersDynamic` passed, including
  real OptionalMutual handshakes, EdgionTls explicit certificate exposure, CN-only/SAN/anonymous
  upstream observations, and A-to-B-to-A destination reload. Final metrics, unknown-field,
  requeue and cache-residue gates passed. Local report:
  `tmp/integration_testing/testing_20261029_215501/report.log` (ephemeral verification artifact).
- K8s suite parity (73 dynamic scenarios), parity regression tests, and mocked dynamic-runner
  tests passed. The reset Gateway matches the existing shared mTLS Gateway; cleanup owns only
  this scenario's four resources. This is registration/restoration evidence, not a live K8s run.

- All nine project quick checks passed: `cargo fmt --all`, workspace all-target check
  and clippy, agent-doc validation, SSA force, metrics inventory, Pingora source policy,
  Gateway API transform contract, and unit-test layout. Clippy retained warnings at
  unrelated existing sites; no warning points to this correction. Full workspace tests
  were not run.

Re-evaluate when output selection, certificate exposure, anonymous behavior, session mutation
semantics, credential hiding, or plugin execution ordering changes. Header-mode verification
and Connection-mode TLS trust remain separate prerequisites.
Pre-existing user-guide example/schema discrepancies remain separately tracked in
[`header-cert-auth-documentation-schema-alignment`](../../../../tasks/todo/header-cert-auth-documentation-schema-alignment.md).

## Resolution references

- ISSUE-003 — configured HeaderCertAuth SAN destination retained forged input for a trusted
  client certificate without SAN; correction and verification tracked in this change.
