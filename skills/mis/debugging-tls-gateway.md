# Debugging TLS Gateway Routing Issues

## Overview

Guide for diagnosing and fixing TLS connection failures through the Edgion
gateway (TLS-terminate-to-TCP mode via TLSRoute + EdgionTls).

## Diagnostic Flow

### 1. Verify K8s Resources Are Accepted

```bash
kubectl get gateway -n edgion-system
kubectl get edgiontls -A
kubectl get tlsroutes -A

# Check detailed status — look for Accepted/Programmed/ResolvedRefs conditions
kubectl get tlsroute <name> -n <ns> -o jsonpath='{.status}' | python3 -m json.tool
kubectl get edgiontls <name> -n <ns> -o yaml
```

Key conditions to verify:
- **TLSRoute**: `Accepted=True`, `Programmed=True`, `ResolvedRefs=True`
- If `ResolvedRefs=False` with `BackendNotFound`, the controller cache may be stale.
  Re-apply the TLSRoute to trigger re-reconciliation.

### 2. Check Gateway Pod Logs

Logs are written to files, not stdout:

```bash
GW_POD=$(kubectl get pods -n edgion-system -l app=edgion-gateway \
  -o jsonpath='{.items[0].metadata.name}')

# Main log (startup, config sync, route registration)
kubectl exec -n edgion-system $GW_POD -- \
  tail -50 /usr/local/edgion/logs/edgion-gateway.$(date +%Y-%m-%d)

# TLS connection log (per-connection events)
kubectl exec -n edgion-system $GW_POD -- \
  tail -20 /usr/local/edgion/logs/tls_access.log
```

### 3. Key Log Patterns to Search For

```bash
# Gateway TLS matcher status — ports:0 means no TLS listeners registered
kubectl exec -n edgion-system $GW_POD -- \
  grep "gateway_tls_matcher" /usr/local/edgion/logs/edgion-gateway.*

# TLSRoute registration and stale cleanup
kubectl exec -n edgion-system $GW_POD -- \
  grep "tls_route_manager\|stale\|TLSRoute configured" /usr/local/edgion/logs/edgion-gateway.*

# Connection-level errors
kubectl exec -n edgion-system $GW_POD -- \
  grep "No matching TLSRoute\|NoMatchingRoute" /usr/local/edgion/logs/edgion-gateway.*

# EdgionTls cert loading
kubectl exec -n edgion-system $GW_POD -- \
  grep "TLS matcher rebuilt\|valid_certs\|invalid_certs" /usr/local/edgion/logs/edgion-gateway.*
```

### 4. Common Failure Modes

#### A. `Gateway TLS matcher: ports:0, hostname_entries:0`

**Root cause**: `GatewayTlsMatcher::rebuild_from_gateways()` in
`src/core/gateway/runtime/matching/tls.rs` skips listeners without
`certificateRefs`. When using `edgion.io/cert-provider: EdgionTls` (no
`certificateRefs`), the listener is silently skipped.

**Fix (code)**: Allow listeners with `edgion.io/cert-provider: EdgionTls`
option to pass through with empty `certificate_refs`.

**Workaround**: Add a `hostname` and `certificateRefs` to the Gateway
listener (same cert that EdgionTls uses).

#### B. `cleaned up stale gateway entries, stale:1` — routes disappear

**Root cause**: Race condition in `TlsRouteManager::rebuild_gateway_routes_map()`
(`src/core/gateway/routes/tls/routes_mgr.rs`). When TLSRoute and Gateway
updates arrive near-simultaneously, the route may be registered then
immediately cleaned as "stale" because the gateway config hasn't been
processed yet. The stale cleanup removes the `GatewayTlsRoutes` entry from
`gateway_tls_routes_map` and clears its data. But `EdgionTls` (the TLS
service) holds an `Arc<GatewayTlsRoutes>` obtained at startup. After
cleanup, a *new* Arc is created for the same gateway key, but `EdgionTls`
still references the old (now-empty) one.

**Workaround**: Restart the gateway deployment after all config changes are
settled, so `EdgionTls` binds to the fresh `GatewayTlsRoutes` Arc.

```bash
kubectl rollout restart deployment/edgion-gateway -n edgion-system
kubectl rollout status deployment/edgion-gateway -n edgion-system --timeout=120s
```

#### C. `Service 'ns/name' not found` in TLSRoute status

**Root cause**: Controller has stale cache. Service exists but controller
hasn't picked it up yet.

**Fix**: Delete and re-create the TLSRoute to trigger re-reconciliation:

```bash
kubectl delete tlsroute <name> -n <ns>
sleep 3
kubectl apply -f <tlsroute-yaml>
```

### 5. TLS Connection Log Fields (tls_access.log)

| Field | Meaning |
|---|---|
| `status: NoMatchingRoute` | TLS handshake succeeded but no TLSRoute matched the SNI |
| `status: Connected` | Full success — connection proxied to backend |
| `connection_established: false` | Backend connection was never made |
| `sni_hostname` | The SNI sent by the client |
| `gateway_name` | Which Gateway listener handled the connection |

### 6. Live Testing via Port-Forward

```bash
# Port-forward to a specific gateway service
kubectl port-forward -n edgion-system svc/edgion-gateway-18443 38443:18443

# Test TLS connection
echo -e "hello\nquit" | openssl s_client \
  -connect 127.0.0.1:38443 \
  -servername test.aaa.example.com \
  -quiet
```

On macOS, `timeout` is not available. Use background process + kill:

```bash
echo -e "hello\nquit" | openssl s_client \
  -connect 127.0.0.1:38443 \
  -servername test.aaa.example.com \
  -quiet 2>&1 &
PID=$!; sleep 5; kill $PID 2>/dev/null; wait $PID 2>/dev/null
```

### 7. Build and Deploy a Fixed Binary

```bash
# Use cross to compile for linux amd64
~/sh/copy-gw.sh

# Or manually:
cd ~/ws/ws1/Edgion
cross build --release --target x86_64-unknown-linux-gnu --bin edgion-gateway

# Copy to all gateway pods
for POD in $(kubectl get pods -n edgion-system -l app=edgion-gateway \
  -o jsonpath='{.items[*].metadata.name}'); do
  kubectl cp target/x86_64-unknown-linux-gnu/release/edgion-gateway \
    edgion-system/$POD:/usr/local/edgion/edgion-gateway
  kubectl exec -n edgion-system $POD -- chmod +x /usr/local/edgion/edgion-gateway
done

# The restart loop in the deployment will pick up the new binary automatically
# (kill the running process or wait for it to cycle)
```

### 8. Entering Gateway / Controller Pods

```bash
~/sh/in-gw.sh    # exec into edgion-gateway pod
~/sh/in-ctl.sh   # exec into edgion-controller pod
```

## Architecture Reference

TLS connection flow:

```
Client --TLS--> Gateway (port 18443)
                  |
                  +--> TLS Handshake (cert from Layer 1: EdgionTls store
                  |                    or Layer 2: GatewayTlsMatcher)
                  |
                  +--> Route Matching (GatewayTlsRoutes.match_route(sni))
                  |
                  +--> Backend Connection (ppv2-echo-aaa:9001)
```

Key components:
- `EdgionTls` (`routes/tls/edgion_tls.rs`) — TLS service, holds `Arc<GatewayTlsRoutes>`
- `GatewayTlsRoutes` (`routes/tls/gateway_tls_routes.rs`) — SNI-to-route map
- `TlsRouteManager` (`routes/tls/routes_mgr.rs`) — manages route lifecycle
- `GatewayTlsMatcher` (`runtime/matching/tls.rs`) — cert lookup from Gateway listeners
- `TlsCallback` (`tls/runtime/gateway/tls_pingora.rs`) — cert loading callback
- `tls_store` (`tls/store/`) — EdgionTls CRD cert store
