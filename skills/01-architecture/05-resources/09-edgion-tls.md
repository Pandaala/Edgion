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
2. Use actual-owner bound readonly acquisition for `secret_ref`; authorize before reads and report pending references without registering edges
3. **Cardinality bounds**: the wire remains decodable after Kubernetes admission; Controller and Gateway runtime validation reject more than 32 `parentRefs`, more than 16 `hosts`, duplicate normalized logical parent attachments, and case-insensitive duplicate hosts. This lets an invalid update revoke an older effective generation instead of poisoning a typed watch. The CRD carries matching `maxItems` constraints, while the runtime checks also protect filesystem/etcd and forged conf-sync input.
4. **Certificate-content status**: `update_status()` owns Common certificate validation (complete ordered chain, supported PKCS#8/RSA PKCS#1/EC SEC1 key, expiry, SAN and key correspondence). Failures appear on ResolvedRefs, not as content warnings from `validate()`. Missing material retains SecretRefManager recovery.

### parse

1. **Install parent dependencies first**: atomically clear-and-replace this EdgionTls entry in GatewayRouteIndex before any Gateway lookup. This closes the publication race with Gateway `after_publish`/`after_unpublish` requeues.
2. **Clear stale references**: clear records for this EdgionTls in SecretRefManager
3. **Resolve server-side certificate Secret**: borrow `EdgionTlsRefs::server`, register valid pending dependencies, authorize through canonical bound acquisition before local-store reads, and populate the scrubbed transport slot
4. **Resolve CA certificate Secret (mTLS)**: if `spec.client_auth.ca_secret_ref` is configured, similarly register and resolve the CA Secret, populating `client_auth.ca_secret`
5. **Capture listener attachment identity**: each resolved listener carries Gateway namespace/name, listener section name, `ListenerKey`, and the exact resolved Gateway proof in `resolved_listener_attachments`. The Gateway TLS callback requires an exact same-generation identity match before an EdgionTls certificate may overlay a standard Gateway frontend-validation winner.
6. **Resolve resolved_listeners**: resolve `Vec<ListenerKey>` from parentRef; at most 2,048 resolved attachments are admitted, bounding the cold attachment×host index to 32,768 entries.

### publication barrier

GatewayRouteIndex registration happens at the beginning of `parse`, not in `on_change`. Gateway requeues run only from `after_publish` (after the new snapshot is visible) and `after_unpublish` (after deletion is visible), so EdgionTls can never resolve the previous Gateway generation after receiving a change notification.

### on_delete

Clear SecretRefManager references and the GatewayRouteIndex registration.

### update_status

Produce an independent RouteParentStatus for each parentRef:
- `Accepted`: True when there are no validation_errors; False (reason=Invalid) when errors exist; also validates whether the Gateway corresponding to parentRef exists
- `ResolvedRefs`: validates actual server/CA fields and carried material; readonly authorization distinguishes RefNotPermitted from SecretNotFound without payload reads or dependency registration.
- Present authorized server material is checked with the shared certificate validator. Missing `tls.crt` or `tls.key` remains `ResolvedRefs=False/InvalidCertificate` with its fixed diagnostic; a missing Secret remains `SecretNotFound`. All validator errors are projected without re-reading payloads or changing Accepted (ARCH-136).
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

## Actual Secret field ownership

`EdgionTlsRefs` binds server and declared client-CA fields independently. Both use
strict Secret references: omitted/empty group, omitted/Secret kind, strict object
names and namespace validation; literal `core` is rejected. Omitted owner namespace
means default, and omitted target namespace inherits that effective namespace.
Only these finite material consumers accept actual Secret metadata namespace None
as default. The default store selector checks qualified and bare entries, rejects
ambiguity and preserves actual metadata. Semantic Secret events supplement raw
notifications only for the finite consumers that use this projection.

Common certificate admission proves the server pair before parsing. Cold Gateway
identity compilation repeats that proof for direct callers; cold mTLS compilation
proves the present CA pair independently after its Terminate early return. No
wire field, new parser or handshake-time reference check is added. Server failure
and absent required CA reference follow whole-generation LKG; valid server and successful remaining `validate_cert` checks with
present invalid/unresolved CA publish a failing current trust generation. See
`skills/04-review/tls/intentional-design/last-known-good-follows-resource-key.md`.
