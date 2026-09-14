# Outbound TLS SAN Pinning Enforcement — OIDC + LDAP (Phase 2)

**Release Date**: 2026-06-30  
**Type**: Feature Enablement (Breaking for OIDC/LDAP with pre-configured SAN)  
**Affects**: OIDC Connect and LDAP Auth plugins; LinkSys and EdgionGatewayConfig defaults remain deferred

## Summary

Phase 2 builds on Phase 1's schema alignment by **enabling runtime SAN pinning verification for OIDC and LDAP plugins**. Previously, configuring `validation.hostname` or `validation.subjectAltNames` on these plugins resulted in `Accepted=False`. Now, the controller accepts the configuration, and the gateway enforces SAN matching at TLS handshake time using rustls's `OutboundSanVerifier`. Certificate mismatch results in handshake failure with the `OutboundTlsSanMismatch` error diagnostic.

Other backends—LinkSys providers (Redis, etcd, Elasticsearch, Kafka), ext_proc, and EdgionGatewayConfig global defaults—remain deferred to a future phase. Configuring SAN on these unsupported backends continues to result in `Accepted=False`.

## Behavior Changes

### OIDC and LDAP Plugins

**Previously (Phase 1):**
- Configuring `validation.subjectAltNames` or `validation.hostname` on an OIDC or LDAP auth plugin resulted in `Accepted=False` with the reason "SAN pinning not yet supported."
- No TLS verification took place; the configuration was effectively ignored.

**Now (Phase 2):**
- `validation.hostname` and `validation.subjectAltNames` are **accepted and enforced at runtime**.
- During TLS handshake to the OIDC provider (e.g., Keycloak, Okta, Azure AD) or LDAP server, the gateway's rustls verifier checks that the peer certificate's Subject Alternative Names (SANs) match the configured values.
- If verification fails (certificate SAN does not contain the expected hostname or value), the handshake is aborted immediately. The connection records `EdgionErrorCode::OutboundTlsSanMismatch` in gateway access-log diagnostics.
- Handshake failures propagate to the request: the client receives an error response, and the gateway records the rejection (see Diagnostics below).

### LinkSys, ext_proc, and Global Defaults (Still Deferred)

- **Redis, etcd, Elasticsearch, Kafka**: SAN pinning configuration continues to result in `Accepted=False`. See `tasks/todo/outbound-tls-sdk-san-audit.md` for the enforcement timeline.
- **ext_proc**: SAN pinning configuration results in `Accepted=False`; enforcement is deferred.
- **EdgionGatewayConfig global `outboundTls` defaults**: SAN pinning via `validation.hostname` or `validation.subjectAltNames` at the global level is not yet enforced; resources fall back to provider-specific overrides.

## New Error Code

| Kind | Code | Typical HTTP outcome | Condition |
|------|------|----------------------|-----------|
| `error` | `OutboundTlsSanMismatch` | 502 Bad Gateway | Outbound peer certificate Subject Alternative Names did not match the configured `validation.subjectAltNames` or `validation.hostname` |

> **Scope note.** `OutboundTlsSanMismatch` appears in the access log for the LDAP bind, and — on the
> OIDC side — only for the discovery fetch reached from the interactive login redirect and the
> callback handler. Every other OIDC outbound call (token exchange, refresh, revoke, introspection,
> userinfo, and the discovery fetch reached from any of them) records the
> `edgion_gateway_outbound_tls_san_mismatch_total` counter and stamps no per-request status.
> OIDC additionally carries static operation/failure categories to PluginLog diagnostics;
> these omit endpoint URLs and raw SAN reasons and do not change best-effort fallback. A SAN
> mismatch is a configuration error that fails every request identically, so the counter is the
> intended alerting signal on all paths.

## Operator Action Required

### ⚠️ Impact Assessment

If you have **pre-configured `validation.subjectAltNames` or `validation.hostname` on OIDC or LDAP auth plugins**, those configurations now become **active at runtime**. TLS handshakes will enforce SAN matching. If the configured values do not match the target service's certificate SANs, handshakes will fail.

### Step 1: Audit Existing Configuration

Find all OIDC and LDAP plugins with SAN configuration:

```bash
kubectl get edgionplugins -A -o yaml | grep -B 5 "subjectAltNames\|hostname" | grep -B 5 "ldap_auth\|openid_connect"
```

### Step 2: Verify Certificate SANs

For each OIDC provider and LDAP server, obtain the server certificate and inspect its SANs:

**For OIDC (HTTPS endpoint):**

```bash
echo | openssl s_client -servername idp.example.com -connect idp.example.com:443 2>/dev/null | \
  openssl x509 -noout -text | grep -A 1 "Subject Alternative Name"
```

**For LDAP (TLS-enabled LDAP server):**

```bash
echo | openssl s_client -connect ldap.example.com:636 2>/dev/null | \
  openssl x509 -noout -text | grep -A 1 "Subject Alternative Name"
```

Ensure that the SANs in the certificate match the values you have configured in `validation.subjectAltNames` or `validation.hostname`.

### Step 3: Monitor for Handshake Failures

After deploying Phase 2, alert on the `edgion_gateway_outbound_tls_san_mismatch_total` counter — this
is the intended signal; the diagnostic code is a stable string with no numeric representation:

```bash
# Alert on outbound TLS SAN pinning rejections
sum(rate(edgion_gateway_outbound_tls_san_mismatch_total[5m])) by (plugin) > 0
```

For ad-hoc log inspection:

```bash
kubectl logs -n edgion-system -l app=edgion-gateway -c edgion-gateway | grep "OutboundTlsSanMismatch"
```

If you see failures, adjust your configuration to match the certificate's actual SANs or temporarily remove the SAN pinning to unblock production (and file an issue).

### Step 4: Optional—Use SAN Pinning Proactively

If you have not yet configured SAN pinning on OIDC or LDAP, this is a good opportunity to add it as a security best practice:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugin
metadata:
  name: corporate-oidc
  namespace: edgion-system
spec:
  type: openid_connect
  config:
    issuer: https://idp.example.com
    clientId: my-client-id
    clientSecretRef:
      name: oidc-client-secret
    tls:
      verify: true
      validation:
        caCertificateRefs:
          - kind: Secret
            name: idp-ca
        hostname: idp.example.com
        subjectAltNames:
          - type: Hostname
            hostname: idp.example.com
```

## Design Reference

For detailed rationale and the full three-phase enforcement timeline (OIDC/LDAP in Phase 2, LinkSys/ext_proc/global defaults in Phase 3), see:

- [`docs/superpowers/specs/2026-06-29-outbound-tls-align-backendtlspolicy-design.md`](../../../superpowers/specs/2026-06-29-outbound-tls-align-backendtlspolicy-design.md) § 6
- [`tasks/todo/outbound-tls-sdk-san-audit.md`](../../../tasks/todo/outbound-tls-sdk-san-audit.md) (enforcement audit and timeline)

## Diagnostics

### Access Log Entry

When a handshake fails due to SAN mismatch, the access log's `diagnostics` array contains the
typed error diagnostic:

```json
{
  "diagnostics": [{ "kind": "error", "code": "OutboundTlsSanMismatch" }]
}
```

### SSL Log (Gateway Data Plane)

`ssl.log` records *inbound* TLS handshake diagnostics only (client → gateway) and is not touched by
outbound SAN pinning. For outbound TLS SAN mismatches, use the counter and the access-log
diagnostic described above.

### Debugging Commands

```bash
# Tail the access log for recent failures
kubectl logs -n edgion-system -l app=edgion-gateway --tail=100 -f | grep "OutboundTlsSanMismatch"

# Describe the plugin to see its configuration
kubectl describe edgionplugin corporate-oidc -n edgion-system

# Check the plugin's Accepted condition
kubectl get edgionplugin corporate-oidc -n edgion-system -o jsonpath='{.status.conditions[?(@.type=="Accepted")]}'
```

## Migration Checklist

- [ ] Run audit command to find OIDC/LDAP plugins with SAN configuration
- [ ] Obtain certificates from all configured OIDC providers and LDAP servers
- [ ] Verify SANs in certificates match configured `validation.subjectAltNames` and `validation.hostname`
- [ ] Deploy Phase 2 in a canary environment and alert on `edgion_gateway_outbound_tls_san_mismatch_total`
- [ ] Adjust configurations or certificates as needed to resolve any SAN mismatches
- [ ] Deploy to production with confidence

## Related Documentation

For detailed information on OIDC and LDAP plugin configuration, see the plugin documentation in your Edgion user guide. For outbound TLS SAN pinning troubleshooting, consult the gateway access logs and the `edgion_gateway_outbound_tls_san_mismatch_total` counter as described in the Diagnostics section above (ssl.log covers inbound TLS handshakes only).
