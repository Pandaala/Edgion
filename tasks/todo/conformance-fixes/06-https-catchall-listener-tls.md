# HTTPS Catch-All Listener TLS

## Priority: Medium | Effort: Small | Tests Fixed: 2 (5 subtests)

## Affected Tests
- HTTPRouteHTTPSListener (subtests 0 and 1 fail; subtest 2 passes)
- HTTPRouteRedirectPortAndScheme (https-listener-on-443 group: all 6 subtests fail)

Note: HTTPRouteRedirectPortAndScheme also has 3 failing subtests for http-listener-on-8080 due to a separate issue (port 8080 not in CustomDialContext/port-forward — same dynamic listener limitation as HTTPRouteListenerPortMatching, deferred).

## Root Cause

The gateway closes the connection (EOF) during the TLS handshake for requests routed to a catch-all HTTPS listener (no hostname configured). Connections to listeners with explicit hostnames work fine (confirmed by `second-example.org` subtest passing).

The `same-namespace-with-https-listener` Gateway has four HTTPS listeners on port 443:

| Listener name | Hostname | Cert |
|---|---|---|
| `https` | *(none / catch-all)* | `tls-validity-checks-certificate` |
| `https-with-hostname` | `second-example.org` | `tls-validity-checks-certificate` |
| `https-with-wildcard-hostname` | `*.wildcard.org` | `tls-validity-checks-certificate` |
| `https-with-hostname-matching-wildcard` | `fourth-example.wildcard.org` | `tls-validity-checks-certificate` |

When the client sends SNI=`example.org`, the gateway fails to match it to the no-hostname listener. It first tries hostname-specific listeners, finds no match, and either drops the connection or fails to fall back to the catch-all listener.

The `unknown-example.org` failure confirms this — it also has no matching listener hostname, so it should fall back to the catch-all `https` listener, and fails with the same EOF.

Likely causes (investigate in order):
1. **SNI matching failure for catch-all listener**: No fallback path when no hostname-specific listener matches the SNI.
2. **Default certificate selection**: The gateway does not properly serve a TLS certificate when no SNI hostname matches any listener with an explicit hostname.

## Code Locations
- Gateway TLS/SNI dispatch logic (Pingora TLS callback for certificate selection) — exact file TBD, investigate the TLS handshake path
- Port-forward is correctly configured: `8443:443`, `CustomDialContext` maps port `443` -> `proxyHost:8443` — port-forward is NOT the issue

## Proposed Fix

Fix the gateway's TLS listener logic so that a no-hostname HTTPS listener serves its configured certificate as the default/fallback when no hostname-specific listener matches the SNI.

The catch-all listener should:
1. Be selected when no more-specific listener matches the client's SNI.
2. Serve its configured TLS certificate (`tls-validity-checks-certificate`).
3. Complete the TLS handshake and proceed to HTTP routing.

**Optional investigation**: Verify the `tls-validity-checks-certificate` Secret actually contains a cert valid for `example.org`. If the cert only covers `second-example.org` and wildcards, the TLS handshake would fail for `example.org` regardless. Check the cert's SAN list.

## Risk Assessment

Medium. Requires changes to TLS handshake / SNI dispatch logic in the gateway data plane. Need to understand how Pingora's TLS callback selects certificates and ensure a fallback path exists.

## Benefit

Fixes 2 conformance tests (8 out of 11 failing subtests across both tests). HTTPRouteHTTPSListener is a **Core** test — required for GATEWAY-HTTP conformance profile. The remaining 3 subtests (http-listener-on-8080) are blocked by dynamic listener creation (deferred).

## Verification
```bash
./scripts/orbstack-dev.sh rebuild g
./scripts/orbstack-dev.sh test --run-test HTTPRouteHTTPSListener
./scripts/orbstack-dev.sh test --run-test HTTPRouteRedirectPortAndScheme
```
