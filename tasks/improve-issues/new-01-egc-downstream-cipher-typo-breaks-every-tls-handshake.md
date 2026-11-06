# new-01: An unvalidated `EdgionGatewayConfig.downstreamTls.ciphers` value is admitted and then aborts every TLS handshake of the class

- **Category**: design-error
- **Severity**: P1
- **Scope**: cross-cutting (Controller admission + Gateway TLS termination)
- **Location**:
  - `edgion-resources/src/resources/downstream_tls.rs:59-73` (`validate_cipher_list` checks only NUL bytes)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs:56-59` (EGC preparse uses that validator)
  - `edgion-gateway/src/tls/downstream_policy.rs:17-30` (`CipherPolicy::compile` stores the joined string), `:44-60` (`SSL_set_strict_cipher_list` per handshake)
  - `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs:143-145` (EGC compile; no trial apply, no fallback)
  - `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs:515`, `:577-586` (select callback returns `SelectCertError::ERROR` on any policy error)

## Summary

`downstreamTls.ciphers` is a new class-wide default that every TLS identity inherits: native Gateway
listener certificates and every EdgionTls without its own `ciphers`. The Controller checks only for
NUL bytes, so an expression that BoringSSL cannot parse is admitted with `Accepted=True`. The Gateway
then stores the string unchanged and applies it with the *strict* BoringSSL setter in the early
ClientHello callback of every new handshake. On rejection the callback aborts the handshake. One typo,
or a cipher string copied from a common OpenSSL/nginx guide, therefore fails **every new TLS handshake
on every listener bound to that EdgionGatewayConfig**. Nothing rejects the configuration or keeps the
previous good generation.

## Current behavior (evidence)

- `validate_cipher_list` (`downstream_tls.rs:61-67`) returns `Ok` for any NUL-free strings. The doc
  comment says it does not try to "implement the backend cipher grammar". The EGC handler pushes an
  error only when this validator fails (`edgion_gateway_config.rs:56-59`).
- The Gateway compiles the list to `CipherPolicy::Explicit(CString)` (`downstream_policy.rs:25-26`) and
  never tries it against BoringSSL when the EGC generation is published (`conf_handler_impl.rs:143`).
- On each handshake, `apply_selected_downstream_policy` resolves `local.or(EGC)` and calls
  `SSL_set_strict_cipher_list` (`downstream_policy.rs:54-59`). In strict mode, BoringSSL rejects any
  unknown name (`ssl_cipher.cc` `ssl_cipher_process_rulestr`: `if (strict) { SSL_R_INVALID_COMMAND;
  return false; }`). Strict mode also accepts only `:` as a separator. The error makes the select
  callback return `SelectCertError::ERROR` (`tls_pingora.rs:577-586`).
- Realistic triggers include the Mozilla "intermediate" list, which contains
  `DHE-RSA-AES128-GCM-SHA256`/`DHE-RSA-AES256-GCM-SHA384` (BoringSSL 5.4 has no DHE suites), a comma
  inside one list element, or a TLS 1.3 suite name. The CRD field is documented as "OpenSSL format,
  similar to nginx ssl_ciphers", so operators are invited to paste such strings.
- Unit tests confirm this behavior is intended: `downstream_policy_test.rs:20-32` asserts that
  `NOT-A-CIPHER` yields `Err("cipher config rejected")`. The only escape is a valid local EdgionTls
  list that masks the default. Before this branch, the strict setter applied only to an EdgionTls's
  own `spec.ciphers`, so a typo affected one identity. The EGC default turns the same failure mode
  into a class-wide outage.
- The documentation (`docs/en/ops-guide/gateway/tls/tls-termination.md:28-32`,
  `skills/04-review/tls/regression-guards/downstream-ecdh-policy-ownership.md`) records that the
  Gateway validates cipher expressions only at apply time. It does not discuss the blast radius, and no
  status condition, metric or one-time log reports the bad generation. Only per-handshake `ssl.log`
  entries do.

## Industry comparison

- NGINX validates `ssl_ciphers` during `nginx -t`/reload, and a bad string fails the reload while the
  old workers keep serving. HAProxy refuses to load such a configuration.
- Envoy validates `cipher_suites` when the listener or secret is created
  (`ContextConfigImpl` → `SSL_CTX_set_strict_cipher_list` in the constructor). The xDS update is NACKed
  and the previous listener keeps serving.
- Istio and Envoy Gateway also enforce the cipher list at configuration time (webhook/translation
  validation), not per connection.

## Impact

A single EGC edit that passes admission causes a complete HTTPS/TLS outage of every listener bound to
that GatewayClass, at the moment the generation reaches the Gateways. The whole fleet is hit at once
because every replica receives the same EGC. Operators see `Accepted=True` and must search `ssl.log`
to learn why handshakes fail.

## Suggested direction

- At EGC (and EdgionTls) compile time on the Gateway, trial-apply the cipher string once to a scratch
  `SSL_CTX` with the same strict setter. If it fails, keep the previous `DownstreamTlsPolicy` for
  that axis (or refuse the generation), emit one ERROR log plus a counter, and report the failure.
  This uses the existing fail-closed rule only when there is no previous good generation.
- Give the Controller a static BoringSSL vocabulary check (cipher names, aliases, operators, `:`
  separator) in `edgion-resources`. Malformed lists are then rejected at admission (`Accepted=False`)
  like `ecdhCurves`, without linking BoringSSL into the Controller.
- Document that strict parsing rejects OpenSSL-only names (DHE, TLS 1.3 names). Add a dynamic
  integration case that shows a malformed EGC cipher list does not reach handshakes.

## Verification (independent)

- **Verdict**: CONFIRMED (P1), verified by the parent on feature-10-28.
- `edgion-resources/src/resources/downstream_tls.rs:59-67`: `validate_cipher_list` only rejects NUL bytes, so any cipher string is admitted.
- `edgion-gateway/src/tls/downstream_policy.rs:45-62`: `SSL_set_strict_cipher_list` is applied per handshake; failure returns `cipher config rejected`.
- `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs:577-586`: that error aborts the handshake (`SelectCertError::ERROR`) for every listener resolving the EGC — no compile-time trial or last-good fallback.
