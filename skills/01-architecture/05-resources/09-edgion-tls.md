---
name: resource-edgion-tls
description: EdgionTls resource — extended TLS certificate configuration, mTLS, Secret reference resolution, Gateway binding.
---

# EdgionTls resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionTls is Edgion's custom-extension resource that provides richer TLS certificate management than Gateway's built-in certificateRefs, supporting independent server-side certificate configuration, client authentication (mTLS), minimum TLS version, and cipher suite selection.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_tls.rs`
- Type definitions: `edgion-resources/src/resources/edgion_tls.rs`

## Controller-side processing

### validate

1. Check that `parent_refs` exists and is non-empty; if unset, warn that it will not take effect
2. Check from GLOBAL_SECRET_STORE whether the Secret referenced by `secret_ref` exists; if not, warn (it may arrive later)
3. **Cert/private-key match check**: if the Secret is already present, extract `tls.crt` and `tls.key` and compare public-key bytes via `check_key_matches_cert()` (`x509-parser` + `rcgen`, pure Rust with no TLS-backend dependency); on mismatch, warn. When the Secret has not arrived, skip and rely on SecretRefManager requeue to retry.

### parse

1. **Clear stale references**: clear records for this EdgionTls in SecretRefManager
2. **Resolve server-side certificate Secret**: from `spec.secret_ref`, obtain the Secret reference, register in SecretRefManager, read Secret data from GLOBAL_SECRET_STORE, and populate `spec.secret`
3. **Resolve CA certificate Secret (mTLS)**: if `spec.client_auth.ca_secret_ref` is configured, similarly register and resolve the CA Secret, populating `client_auth.ca_secret`
4. **Resolve resolved_listeners**: resolve `Vec<ListenerKey>` from parentRef via `resolve_listener_keys()`; logic is identical to route resources: always look up the Gateway to obtain address and protocol, validate consistency when sectionName+port are both present, subject to the allowedRoutes namespace policy

### on_change

Register into GatewayRouteIndex to ensure that hostname/port changes on the Gateway can requeue this EdgionTls (recomputing resolved_listeners).

### on_delete

Clear SecretRefManager references and the GatewayRouteIndex registration.

### update_status

Produce an independent RouteParentStatus for each parentRef:
- `Accepted`: True when there are no validation_errors; False (reason=Invalid) when errors exist; also validates whether the Gateway corresponding to parentRef exists
- `ResolvedRefs`: checks whether the Secret referenced by secret_ref exists in GLOBAL_SECRET_STORE; if not, False (reason=SecretNotFound)
- Sets only the Accepted and ResolvedRefs Conditions; does not set Programmed/Ready

## Gateway-side processing

After EdgionTls is synced to the Gateway side, it is stored in TlsStore and provides certificates for SNI matching. When a request arrives, the certificate is matched against the SNI field of the TLS ClientHello, supporting precise matching by hostname and port.

When TlsStore writes, it calls `validate_cert()` for a secondary check (including cert/key matching); invalid certificates do not enter `TlsCertMatcher`, serving as a fallback to the Controller-side validation.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionTls → Secret | Secret | secret_ref | Server-side TLS certificate and private key |
| EdgionTls → Secret | Secret | client_auth.ca_secret_ref | CA certificate for mTLS |
| EdgionTls → Gateway | Gateway | parentRef | Bound to a specific Listener of a Gateway |
| EdgionTls ← SecretRefManager | Secret | cascading requeue | Secret changes automatically re-process EdgionTls |
