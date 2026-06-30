# SanUnsupported — Redis SAN Pinning Rejection Fixture

## Purpose

This fixture tests that the controller correctly rejects a Redis `LinkSys`
resource that specifies `tls.validation.subjectAltNames`. SAN pinning is
**not supported** for Redis because the `fred` TLS connector does not expose
per-connection SAN override hooks.

## Expected Behavior

When `01_LinkSys_default_redis-san-unsupported.yaml` is applied:

- The resource is **admitted** to the API server (schema is valid).
- The controller sets the `Accepted` condition to **False**.
- The `reason` field contains `"SAN pinning not supported"` (mapped from
  `EdgionStatus::OutboundTlsVerifyUnsupported`).
- No Redis client is registered in the gateway (`/api/v1/testing/link-sys/redis/clients`
  does NOT contain `redis-san-unsupported`).

## Verification Steps (future integration run)

```bash
# Apply the fixture
kubectl apply -f 01_LinkSys_default_redis-san-unsupported.yaml

# Verify Accepted=False
kubectl get linksys redis-san-unsupported -n default \
  -o jsonpath='{.status.conditions[?(@.type=="Accepted")].status}'
# Expected: False

# Verify reason
kubectl get linksys redis-san-unsupported -n default \
  -o jsonpath='{.status.conditions[?(@.type=="Accepted")].message}'
# Expected: contains "SAN pinning not supported"
```

## Notes

- The fixture does not need a running Redis server; the rejection happens
  during controller admission before any connection attempt.
- The Secret referenced by `auth.secretRef` is intentionally absent — the
  SAN rejection is expected before auth resolution.
