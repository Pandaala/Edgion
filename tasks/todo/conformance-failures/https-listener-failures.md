# HTTPS Listener Conformance Test Failures

## Tests Analyzed

1. **HTTPRouteHTTPSListener** (Core)
2. **HTTPRouteRedirectPortAndScheme** (Extended)

---

## 1. HTTPRouteHTTPSListener

### Symptom

- Subtest 0: `example.org/` via HTTPS -> **EOF after ~29s** (FAIL)
- Subtest 1: `unknown-example.org/` via HTTPS -> **EOF after ~29s** (FAIL)
- Subtest 2: `second-example.org/` via HTTPS -> **PASS** (0.07s)

### Gateway Configuration

The `same-namespace-with-https-listener` Gateway has four HTTPS listeners on port 443:

| Listener name | Hostname | Cert |
|---|---|---|
| `https` | *(none / catch-all)* | `tls-validity-checks-certificate` |
| `https-with-hostname` | `second-example.org` | `tls-validity-checks-certificate` |
| `https-with-wildcard-hostname` | `*.wildcard.org` | `tls-validity-checks-certificate` |
| `https-with-hostname-matching-wildcard` | `fourth-example.wildcard.org` | `tls-validity-checks-certificate` |

### Port-Forward Path

Port-forward is correctly configured: `8443:443`. The `CustomDialContext` in `conformance_test.go` maps port `443` -> `proxyHost:8443`. HTTPS traffic does reach the gateway pod on port 443. This is confirmed by subtest 2 (`second-example.org`) passing.

### Root Cause: TLS Handshake Failure on Catch-All HTTPS Listener

The port-forward is **not** the issue. Traffic reaches the gateway.

The problem is that the gateway closes the connection (EOF) during the TLS handshake for requests routed to the catch-all `https` listener (no hostname). The `second-example.org` listener works because it has an explicit hostname, enabling SNI-based certificate selection.

Likely causes (investigate in order):

1. **SNI matching failure for catch-all listener**: When the client sends SNI=`example.org`, the gateway may fail to match it to the no-hostname listener because it first tries hostname-specific listeners, finds no match, and either drops the connection or fails to fall back to the catch-all listener.

2. **Default certificate selection**: The gateway may not properly serve a TLS certificate when no SNI hostname matches any listener with an explicit hostname. The catch-all listener should serve its configured cert for any SNI that doesn't match a more specific listener, but this fallback may not be implemented.

3. **`unknown-example.org` failure confirms this**: This hostname also has no matching listener hostname, so it should fall back to the catch-all `https` listener -- and it fails with the same EOF.

### Verdict

**Gateway-side TLS issue**: The catch-all (no-hostname) HTTPS listener does not properly serve TLS. Port-forward works fine.

---

## 2. HTTPRouteRedirectPortAndScheme

### Symptom

Three groups of subtests; failures span two groups:

**http-listener-on-80** (port 80): All 6 subtests **PASS**.

**https-listener-on-443** (port 443): All 6 subtests **FAIL** (30s timeout each):
- `example.org/scheme-nil-and-port-nil`
- `example.org/scheme-nil-and-port-443`
- `example.org/scheme-nil-and-port-8443`
- `example.org/scheme-http-and-port-nil`
- `example.org/scheme-http-and-port-80`
- `example.org/scheme-http-and-port-8080`

**http-listener-on-8080** (port 8080): All 3 subtests **FAIL** (30s timeout each):
- `/scheme-nil-and-port-nil`
- `/scheme-nil-and-port-80`
- `/scheme-https-and-port-nil`

### Root Cause Analysis

**https-listener-on-443 failures**: Same root cause as HTTPRouteHTTPSListener. The test creates a Gateway with an HTTPS listener on 443, and requests to `example.org` via HTTPS get EOF during TLS handshake. This is the same catch-all / SNI fallback issue.

**http-listener-on-8080 failures**: Completely different root cause. The test creates a Gateway `same-namespace-with-http-listener-on-8080` with an HTTP listener on port 8080. The `CustomDialContext` in `conformance_test.go` only handles ports 80 and 443:

```go
switch port {
case "80":
    addr = net.JoinHostPort(proxyHost, httpPort)
case "443":
    addr = net.JoinHostPort(proxyHost, httpsPort)
default:
    return nil, fmt.Errorf("unexpected port in gateway address: %s", addr)
}
```

When the conformance test tries to send requests to port 8080, the `CustomDialContext` returns `"unexpected port in gateway address"`. The port-forward also does not map port 8080. So requests to the 8080 listener never reach the gateway.

### Is Port 8080 Supported?

The Gateway manifest does **not** pre-create a port-8080 listener -- the test itself creates `same-namespace-with-http-listener-on-8080` dynamically. Whether Edgion supports dynamic listener creation on port 8080 is a separate question, but the **immediate** failure is that the test harness cannot route traffic to port 8080 at all.

---

## Summary

| Test | Failing subtests | Root cause | Category |
|---|---|---|---|
| HTTPRouteHTTPSListener | 0, 1 (example.org, unknown-example.org) | Catch-all HTTPS listener TLS handshake failure (no-hostname SNI fallback) | Gateway TLS bug |
| RedirectPortAndScheme / https-listener-on-443 | All 6 | Same TLS handshake issue as above | Gateway TLS bug |
| RedirectPortAndScheme / http-listener-on-8080 | All 3 | Port 8080 not handled in CustomDialContext + no port-forward mapping | Test harness limitation |

---

## Proposed Fixes

### Fix 1: Catch-All HTTPS Listener TLS (Gateway Bug)

**What**: Fix the gateway's TLS listener logic so that a no-hostname HTTPS listener serves its configured certificate as the default/fallback when no hostname-specific listener matches the SNI.

**Risk**: Medium. Requires changes to TLS handshake / SNI dispatch logic in the gateway data plane.

**Complexity**: Medium. Need to understand how Pingora's TLS callback selects certificates and ensure a fallback path exists.

**Benefit**: Fixes 8 out of 11 failing subtests across both tests. HTTPRouteHTTPSListener is a **Core** test -- required for GATEWAY-HTTP conformance profile.

### Fix 2: Port 8080 Support in Test Harness

**What**: Two changes needed:
1. Add port 8080 mapping to `CustomDialContext` in `conformance_test.go` (e.g., `case "8080": addr = net.JoinHostPort(proxyHost, port8080Port)`).
2. Add port 8080 to the `kubectl port-forward` command in `run.sh` (e.g., `8082:8080` or a configurable local port).
3. Ensure the gateway actually listens on port 8080 when a Gateway resource requests it (dynamic listener creation, or pre-create the Gateway).

**Risk**: Low for test harness changes. Medium if dynamic listener creation is needed.

**Complexity**: Low for harness fix. The gateway may already support port 8080 if the Gateway resource is pre-created; if not, dynamic listener support is a larger effort.

**Benefit**: Fixes 3 failing subtests. GatewayPort8080 is an Extended feature -- nice to have but not required for core conformance.

---

## Recommendation

1. **Priority 1**: Fix the catch-all HTTPS listener TLS bug. This is blocking a Core conformance test (HTTPRouteHTTPSListener) and also causes the https-listener-on-443 portion of RedirectPortAndScheme to fail. Investigate the gateway's SNI dispatch / default certificate logic in the Pingora TLS callback.

2. **Priority 2**: Add port 8080 support to the test harness (`conformance_test.go` CustomDialContext + `run.sh` port-forward). Then verify whether Edgion supports dynamic listener creation on non-standard ports, or pre-create the 8080 Gateway in `conformance-gateway.yaml`.

3. **Optional**: If the catch-all listener fix is complex, consider whether the `tls-validity-checks-certificate` Secret actually contains a cert valid for `example.org`. If the cert only covers `second-example.org` and wildcards, the TLS handshake would fail for `example.org` regardless. Verify the cert's SAN list.
