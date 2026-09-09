# Reconcile Gateway Frontend TLS Knowledge

## Meta

- Status: pending
- Type: docs
- Priority: P3

## Evidence and Scope

Revalidate these descriptions against the current implementation before editing:

- `skills/01-architecture/02-gateway/04-tls/SKILL.md` and the module rustdoc in
  `edgion-gateway/src/runtime/matching/tls.rs` describe unconditional EdgionTls-first
  fallback. The early selection callback in
  `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs` first honors the matching
  Gateway frontend decision: StrictReject cannot be rescued by an overlay, and
  overlay selection requires exact attachment identity.
- `skills/04-review/tls/regression-guards/frontend-mtls-empty-store-on-missing-ca-key.md`
  refers to the removed `certs_added` guard and tolerating an earlier missing CA
  when a later source succeeds. `compile_frontend_mtls` validates the entire
  ordered source set; missing or mismatched sources reject strict configuration.
- `skills/04-review/tls/regression-guards/frontend-validation-is-mtls-empty-ca.md`
  and the corresponding frontend-validation rustdoc in `tls_pingora.rs` describe
  empty/absent references as `Ok(false)`. Only NotConfigured yields that outcome;
  strict empty references compile into StrictReject.
- `skills/04-review/tls/intentional-design/cert-matcher-duplication-accepted.md`
  lists removed `is_listener_not_programmed` gating. Retain the still-valid
  rationale for separate matchers, but replace obsolete implementation evidence.
- Check `skills/04-review/tls/regression-guards/multi-certificate-ref.md` and
  `skills/04-review/tls/regression-guards/runtime-expiry-recheck-symmetry.md`
  against compiled certificate ownership and installation. References to removed
  `compiled_serving_secrets` / `apply_secret_to_ssl` APIs and per-handshake PEM or
  Asn1Time construction must not be presented as current implementation.
  The Gateway path uses `compiled_serving_certificates`,
  `select_gateway_certificate` and `apply_compiled_certificate_to_ssl`, comparing
  compiled Unix validity bounds at installation. The EdgionTls installation path
  still parses PEM and compares Asn1Time; do not erase this distinction or
  reintroduce per-handshake compilation. Correct adjacent stale callback comments
  as documentation only.

## Acceptance

- Correct only the owning knowledge pages and inaccurate rustdoc. Preserve
  useful historical findings while explicitly distinguishing current behavior.
- Do not change runtime policy to restore an obsolete description. Preserve
  strict rejection, ordered serving/CA identity, aggregate limits, exact overlay
  attachment and canonical early BoringSSL selection.
- Read `runtime/matching/tests/tls_test.rs` and the actual callback branches.
  Basic matching fixtures can return a rejection entry; `Some(entry)` alone
  does not prove a successful handshake or a usable serving certificate.
- Run the TLS matcher unit tests and documentation consistency guard. Existing
  integration tests stay unchanged; no new integration scenario is required for
  these documentation corrections.
