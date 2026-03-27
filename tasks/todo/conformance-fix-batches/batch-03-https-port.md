# Batch 3: HTTPS Catch-All + Port 8080

## Status: [ ] Not Started

## Tests to Fix
- HTTPRouteHTTPSListener (currently: FAIL individually -- subtests 0 and 1 fail, subtest 2 passes)
- HTTPRouteRedirectPortAndScheme (currently: FAIL individually -- https-listener-on-443 group fails; http-listener-on-8080 group deferred)

## Problem

**HTTPS catch-all listener:** The gateway closes the connection (EOF) during TLS handshake for requests to a catch-all HTTPS listener (no hostname configured). When the client sends SNI=`example.org`, the gateway fails to match it to the no-hostname listener. It tries hostname-specific listeners first, finds no match, and drops the connection instead of falling back to the catch-all listener.

The `same-namespace-with-https-listener` Gateway has four HTTPS listeners on port 443:
- `https` -- no hostname (catch-all), cert: `tls-validity-checks-certificate`
- `https-with-hostname` -- `second-example.org`
- `https-with-wildcard-hostname` -- `*.wildcard.org`
- `https-with-hostname-matching-wildcard` -- `fourth-example.wildcard.org`

Subtest 2 (`second-example.org`) passes because it matches the hostname-specific listener.

**Port 8080 issue (deferred):** HTTPRouteRedirectPortAndScheme also has 3 failing subtests for `http-listener-on-8080`. This requires dynamic listener creation or adding port 8080 to `CustomDialContext`. Deferred as architectural limitation.

## Fix Steps

### Step 1: Investigate TLS/SNI dispatch logic

Find the gateway's TLS callback that selects certificates based on SNI. This is likely in the Pingora TLS callback configuration or a custom SNI resolver.

```bash
# Search for SNI/TLS callback registration
rg -n "sni|ssl_callback|cert_select|tls_accept" src/core/gateway/ --type rust
```

### Step 2: Add catch-all listener fallback

In the SNI dispatch logic, after trying all hostname-specific listeners:
1. If no hostname-specific listener matches the client's SNI, check for a no-hostname (catch-all) listener on the same port.
2. If found, serve its configured TLS certificate and complete the handshake.
3. After TLS handshake, proceed to HTTP routing as normal.

### Step 3: Verify certificate validity

Check that `tls-validity-checks-certificate` is a wildcard or multi-SAN cert that can serve arbitrary hostnames. If it only covers specific domains, the TLS handshake will fail even with the correct listener selection. The conformance test likely uses `-k` (skip TLS verification) or a custom CA.

### Step 4: Add port 8080 to CustomDialContext (optional, for partial fix)

**File:** conformance test setup / `CustomDialContext` configuration

If port 8080 is already bound by the gateway but just not in the dial context, add it:
```go
CustomDialContext: map[string]string{
    "8080": "proxyHost:port8080mapping",
}
```

This would fix the `http-listener-on-8080` subtests without dynamic listener creation, but only if the gateway already binds port 8080.

## Build & Deploy
```bash
# Gateway only
./scripts/orbstack-dev.sh rebuild g
```

## Verify
```bash
# Run individual test(s)
./scripts/orbstack-dev.sh test --run-test HTTPRouteHTTPSListener
./scripts/orbstack-dev.sh test --run-test HTTPRouteRedirectPortAndScheme

# After all tests in this batch pass individually, run core batch:
./scripts/orbstack-dev.sh test --core
```

## Result
- [ ] Individual tests pass
- [ ] Core batch score improved
- Previous score: __/60
- New score: __/60
