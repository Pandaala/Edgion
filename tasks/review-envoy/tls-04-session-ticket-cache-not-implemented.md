---
module: tls-secrets
title: TLS session tickets and session cache are schema-reserved no-ops; session resumption is entirely uncontrolled (BoringSSL defaults apply unconditionally)
severity: low
envoy_reference: Envoy `StatefulSession` filter, Envoy `ssl.session_reused` stats counter, Envoy TLS session timeout via `session_timeout` in `TlsParameters`, Envoy TLS session ticket key rotation via SDS `GenericSecret`
---

# TLS session tickets and session cache are not implemented

## Envoy's design (the proven baseline)

Envoy exposes explicit control over TLS session resumption at two levels:

1. **Session tickets** (`TlsCertificate.ocsp_staple` analogue for tickets):
   Envoy supports per-listener session ticket key rotation via SDS
   `GenericSecret`. Operators can deliver new ticket keys independently of
   certificates, rotate them on a configurable schedule, and invalidate all
   sessions instantly by removing the key. The `ssl.session_reused` counter
   tracks how many connections skip a full handshake.

2. **Session cache** (`ssl_context_config.session_ticket_key_paths`,
   `session_timeout`): Envoy lets operators bound the session cache size and
   TTL. In a multi-worker deployment, a session cache or ticket key shared
   across workers avoids forcing a full handshake when the client reconnects to
   a different worker thread.

Both mechanisms are configurable, measurable, and rotatable independently of
the certificate.

## Edgion's current design

`EdgionTlsExtend` defines both:

```rust
// edgion-resources/src/resources/edgion_tls.rs:203,207
pub session_ticket: Option<SessionTicketConfig>,
pub session_cache: Option<SessionCacheConfig>,
```

`SessionTicketConfig` has `enabled`, `lifetime_seconds`,
`rotation_interval_seconds`, and `key_secret_ref`
(`edgion-resources/src/resources/edgion_tls.rs:221-234`).
`SessionCacheConfig` has `enabled`, `max_entries`, `ttl_seconds`
(`edgion-resources/src/resources/edgion_tls.rs:239-249`).

None of these fields are consumed anywhere in `edgion-gateway/src/` or `edgion-common/src/`. A grep for
`session_ticket` and `session_cache` inside those directories returns zero hits
(only the type definitions and test fixtures reference them). The gateway
runs with BoringSSL's built-in session resumption defaults — session tickets
enabled by default with a randomly-generated key that is regenerated at each
process restart, and an in-process session cache whose size and TTL are not
operator-controlled.

## The problem / gap

Two distinct gaps relative to Envoy:

**Gap 1 — Security: uncontrolled session ticket keys**
BoringSSL generates a random session ticket encryption key at process start.
In a multi-pod K8s deployment each gateway pod generates an independent key.
This means:

- A client that establishes a session with pod A and then reconnects to pod B
  (after a load-balancer rebalance) will not be able to resume and must do a
  full handshake. This is a latency concern, not a security one.
- After a pod restart the old session ticket key is gone. All active resumed
  sessions with the restarted pod are invalidated and require a full handshake.
- There is no operator control over session ticket key rotation interval. If
  the BoringSSL default lifetime is long (hours), a stolen session ticket key
  would allow impersonation for that duration with no way to shorten the
  window except restarting all pods.

Envoy addresses this with SDS-delivered `GenericSecret` ticket keys: the same
key is pushed to all replicas and rotated on demand.

**Gap 2 — Observability: no `ssl.session_reused` metric**
Without a `session_reused` counter, operators cannot tell what fraction of
connections use session resumption, whether a cache size limit is causing
unnecessary full handshakes, or whether a configuration change inadvertently
disabled resumption.

**Gap 3 — False operator expectations**
The schema advertising `session_ticket.enabled` and `session_cache.max_entries`
implies these are operative. An operator who sets `session_ticket.enabled:
false` to disable resumption (e.g., for strict forward-secrecy enforcement with
no state) receives no error and no effect — BoringSSL continues enabling session
tickets regardless.

## Impact

- A compliance operator who attempts to disable session tickets (to enforce PFS
  with no session resumption, as some security policies require) believes they
  have done so but have not.
- A high-volume deployment cannot share session ticket keys across pods, forcing
  full handshakes on any pod-to-pod rebalance.
- No visibility into session resumption ratio via Prometheus.

## Suggested direction

- **Short term**: emit a config-time warning (same pattern as `ocsp_stapling` /
  `revocation_check`) when `session_ticket` or `session_cache` fields are set,
  making clear they have no runtime effect today.
- **Medium term (tickets)**: implement `SessionTicketConfig.enabled = false` by
  calling `SSL_CTX_set_options(SSL_OP_NO_TICKET)` on the BoringSSL context at
  listener setup. This is a single FFI call that actually enforces the policy.
- **Medium term (shared keys)**: the controller could resolve `key_secret_ref`
  and inline a shared ticket key into `EdgionTls.spec.extend.session_ticket`;
  `new_tls_settings_with_callback` would call
  `SSL_CTX_set_tlsext_ticket_keys` once per listener.
- Add a `edgion_tls_session_reused_total` counter incremented in
  `handshake_complete_callback` when `ssl.session_reused()` is true.
