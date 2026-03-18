# `options` Keys And Related Labels

Use this file when the key looks like `edgion.io/*`, but it is not actually stored under `metadata.annotations`.

## Verify In Code

- `src/core/gateway/runtime/matching/tls.rs`
- `src/types/resources/backend_tls_policy.rs`
- `src/types/constants/labels.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/leader_election.rs`
- `src/core/controller/services/acme/service.rs`

## Gateway Listener `tls.options`

These keys live under:

```yaml
spec:
  listeners:
    - tls:
        options:
          <key>: <value>
```

| Key | Value | Effect |
|-----|-------|--------|
| `edgion.io/cert-provider` | currently `EdgionTls` in examples | Tells Gateway TLS matching logic that listener certificates may come from the `EdgionTls` CRD instead of static `certificateRefs`. |

Important:

- This is not `Gateway.metadata.annotations`.
- The current TLS matcher checks `listener.tls.options["edgion.io/cert-provider"]`.

## BackendTLSPolicy `spec.options`

These keys live under:

```yaml
spec:
  options:
    <key>: <value>
```

| Key | Value | Effect |
|-----|-------|--------|
| `edgion.io/client-certificate-ref` | `secret-name` in the same namespace | Selects the Secret used for upstream mTLS client authentication. |

Important:

- Cross-namespace `namespace/name` is currently treated as invalid by the parser.
- Validation and parsing live in `src/types/resources/backend_tls_policy.rs`.

## Related `edgion.io/*` Labels

These are labels, not annotations.

| Key | Where | Effect |
|-----|-------|--------|
| `edgion.io/leader` | controller Pod / leader Service selection | Marks leader state in Kubernetes HA mode. |
| `edgion.io/managed-by` | ACME-managed Secrets and related internal resources | Indicates ownership by the ACME subsystem. |
| `edgion.io/acme-resource` | ACME-managed Secrets and related internal resources | Stores the owning `EdgionAcme` resource name/key. |

## Quick Placement Test

If the task mentions one of these questions, use this reference:

- "Why does `cert-provider` not show up in `metadata.annotations`?"
- "Where should `client-certificate-ref` live?"
- "Is `edgion.io/leader` a label or annotation?"

If the answer is "inside `tls.options`, `spec.options`, or labels", stay in this file.
