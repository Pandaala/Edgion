# Outbound TLS — BackendTLSPolicy Schema Alignment (Breaking)

**Release Date**: 2026-06-30  
**Type**: Breaking Change  
**Affects**: Plugins (OpenID Connect, LDAP Auth, Ext Proc), LinkSys providers (Redis, etcd, Elasticsearch), EdgionGatewayConfig global default

## Summary

The outbound TLS schema used in plugins, link-system providers, and the global gateway configuration has been reshaped to align with the Gateway API `BackendTLSPolicy` model. This is a breaking change: operators must migrate existing configurations from the legacy `caSource` / `caSecretRef` / `clientSecretRef` model to the new `validation.caCertificateRefs` / `clientCertificateRef` shape.

## Field Mapping

| Removed | New | Notes |
|---|---|---|
| `tls.caSource: SecretRef` + `tls.caSecretRef: {name: X}` | `tls.validation.caCertificateRefs: [{kind: Secret, name: X}]` | CA trust source now uses `caCertificateRefs` (supports multiple refs) |
| `tls.caSource: System` | `tls.validation.wellKnownCACertificates: System` or omit `validation` entirely | When `validation` is omitted, defaults to system webpki roots |
| `tls.caSource: Inline` + `tls.caCert: "<PEM>"` | Not supported; create a Secret or ConfigMap containing the CA certificate | Inline CA certificate in configuration is no longer supported |
| `tls.clientSecretRef: {name: Y}` | `tls.clientCertificateRef: {name: Y}` | Field renamed; same semantics |
| `tls.insecureSkipVerify: true` | `tls.verify: false` | Three-state outer switch; replaces the tri-state `caSource` model |

## New Capabilities

### Multiple CA Bundles

The `caCertificateRefs` field now accepts a list of certificate references. Verification succeeds if the server certificate chains to any of the provided CAs:

```yaml
tls:
  verify: true
  validation:
    caCertificateRefs:
      - kind: Secret
        name: internal-ca
      - kind: Secret
        name: partner-ca
```

### ConfigMap CA Source

In addition to Secrets, ConfigMaps can now serve as CA certificate sources (new capability for plugins oidc, ldap, ext_proc and EdgionGatewayConfig). LinkSys providers (Redis, etcd, Elasticsearch) currently support Secret refs only.

```yaml
tls:
  verify: true
  validation:
    caCertificateRefs:
      - kind: ConfigMap
        name: corporate-ca-bundle
```

### SAN Hostname Pinning (Schema Only, Phase 1)

The schema now exposes optional hostname and Subject Alternative Name (SAN) fields:

```yaml
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

These fields are **validated at the schema level only** in this release. Enforcement (actual SAN pinning verification) will be implemented in a future release for OpenID Connect and LDAP Auth providers. If you configure SAN pinning on a provider that does not yet support it, the resource will show `Accepted=False` with a reason explaining the limitation.

## Migration Guide

### Step 1: Identify Affected Resources

Find all resources using the legacy outbound TLS schema:

```bash
# Check EdgionGatewayConfig
kubectl get egc -A -o yaml | grep -A 10 "caSource:"

# Check plugins (oidc, ldap, ext_proc)
kubectl get edgionplugins -A -o yaml | grep -A 10 "caSource:"

# Check LinkSys providers
kubectl get edgionlinksys -A -o yaml | grep -A 10 "caSource:"
```

### Step 2: Migrate Configuration

Using the field mapping table above, rewrite your configuration. Example:

**Before (legacy schema):**

```yaml
apiVersion: edgion.io/v1beta1
kind: EdgionPlugin
metadata:
  name: ldap-auth
spec:
  type: ldap_auth
  config:
    ca_source: SecretRef
    ca_secret_ref:
      name: ldap-ca
    client_secret_ref:
      name: ldap-client-cert
    insecure_skip_verify: false
```

**After (new schema):**

```yaml
apiVersion: edgion.io/v1beta1
kind: EdgionPlugin
metadata:
  name: ldap-auth
spec:
  type: ldap_auth
  config:
    validation:
      caCertificateRefs:
        - kind: Secret
          name: ldap-ca
    clientCertificateRef:
      name: ldap-client-cert
    verify: true
```

### Step 3: Inline CA Migration

If you previously used inline CA certificates (`caSource: Inline`), migrate to a Kubernetes Secret:

1. Extract the CA certificate from your configuration
2. Create a Kubernetes Secret:

```bash
kubectl create secret generic my-ca \
  --from-literal=ca.crt="$(cat /path/to/ca.pem)" \
  -n <target-namespace>
```

3. Update your resource to reference the Secret:

```yaml
tls:
  verify: true
  validation:
    caCertificateRefs:
      - kind: Secret
        name: my-ca
```

Alternatively, use a ConfigMap:

```bash
kubectl create configmap my-ca \
  --from-literal=ca.crt="$(cat /path/to/ca.pem)" \
  -n <target-namespace>
```

## Behavior Changes

### Schema Validation

Stale configuration using the legacy keys will be handled as follows:

- **Structural CRD paths** (LinkSys `tls` block, EdgionGatewayConfig `outboundTls`): The Kubernetes API server **silently prunes** any fields not declared in the CRD's structural schema. Legacy keys will not cause an explicit error but will be silently ignored (becoming a no-op). Check your deployed configuration with `kubectl get -o yaml` to verify the migration is complete.

- **Plugin paths** (EdgionPlugins resource under `x-kubernetes-preserve-unknown-fields`): The schema will be enforced during plugin validation. Legacy keys will cause the resource to enter `Accepted=False` status with an explicit error message detailing the stale keys.

### Verification Behavior

- `tls.verify: false` continues to work as an operator-owned escape hatch for development/testing scenarios. When configured, the entire `validation` block is ignored, and certificate verification is skipped.
- When `tls.verify: true` (default) or omitted, the `validation` block is honored.
- If `validation` is omitted and `verify` is not `false`, the gateway defaults to the system webpki roots for verification.

### LinkSys Limitation

LinkSys providers (Redis, etcd, Elasticsearch) currently support Secret references only. ConfigMap CA sources are not yet supported for LinkSys. Use `kind: Secret` for all LinkSys TLS CA references.

## Related Documentation

For detailed schema design rationale and Phase 2 roadmap (SAN pinning enforcement), see the design specification: [`docs/superpowers/specs/2026-06-29-outbound-tls-align-backendtlspolicy-design.md`](../../../superpowers/specs/2026-06-29-outbound-tls-align-backendtlspolicy-design.md)

## Support & Questions

If you encounter issues during migration, check:

1. **Configuration status**: `kubectl describe` the affected resource to see any `Accepted=False` conditions
2. **Admission logs**: Look for pruned fields in structural paths
3. **Gateway logs**: Error messages on TLS connection failures will appear in the gateway audit/access logs with the `EdgionStatus` code and reason
