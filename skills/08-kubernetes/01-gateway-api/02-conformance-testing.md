---
name: gateway-api-conformance-testing
description: How to run, debug, and incrementally validate Gateway API conformance tests
---

# Gateway API Conformance Testing

## Script location

`examples/gateway-api-conformance/scripts/run.sh`

## Authoritative source for feature declarations

**`conformance_test.go` is the single authoritative source for supported features.**

`run.sh` does not pass `-supported-features` by default, letting `opts.SupportedFeatures` from `conformance_test.go` take effect.
It is overridden only when `--core`, `--extended`, or `--supported-features <list>` is explicitly passed.

When adding feature support:
1. Add it via `opts.SupportedFeatures.Insert(features.SupportXxx)` in `conformance_test.go`
2. `run.sh` does not need changes

## Timeout configuration

`conformance_test.go` configures timeouts in one place. `MaxTimeToConsistency` is set to 120s to accommodate
the certificate-propagation delay of TLS Passthrough data-plane tests:

| Parameter | Configured | Library default |
|-----------|------------|-----------------|
| `GatewayMustHaveAddress` | 10s | 180s |
| `GatewayMustHaveCondition` | 10s | 180s |
| `GatewayStatusMustHaveListeners` | 10s | 60s |
| `HTTPRouteMustHaveCondition` | 10s | 60s |
| `TLSRouteMustHaveCondition` | 10s | 60s |
| `RouteMustHaveParents` | 10s | 60s |
| `MaxTimeToConsistency` | 120s | 30s |
| `LatestObservedGenerationSet` | 10s | 60s |
| `DefaultTestTimeout` | 15s | 60s |
| `RequiredConsecutiveSuccesses` | 3 | 3 |

**Important**: When status-class tests time out (context deadline exceeded after 10s), the root cause is almost always an incorrect status condition setting, not a timeout that is too short. When the controller is healthy, status updates within 1s.

## Run granularity

### Single test

```bash
cd examples/gateway-api-conformance
GO_RUN_FILTER="TestConformance/GatewayObservedGenerationBump" bash scripts/run.sh
```

`GO_RUN_FILTER` is converted to `go test -run <filter>`, which correctly filters subtests under profile mode (`--run-test` is ineffective in profile mode).

### Run a batch by name prefix

```bash
# All Gateway / GatewayClass tests
GO_RUN_FILTER="TestConformance/Gateway" bash scripts/run.sh

# All HTTPRoute tests
GO_RUN_FILTER="TestConformance/HTTPRoute" bash scripts/run.sh
```

### Presets

| Preset | Command | Description |
|--------|---------|-------------|
| Default (all declared features) | `bash scripts/run.sh` | Use the full feature set declared in conformance_test.go |
| Core only | `--core` | Force only Gateway+HTTPRoute+ReferenceGrant |
| Extended | `--extended` | Extended feature set (includes redirect, mirror, etc.) |
| All features | `--all-features` | All features (including unimplemented ones; will FAIL) |

### Split-Profile Run (mandatory)

Edgion does not support HTTPS Terminate and TLS Passthrough on the same port.
Conformance test manifests hardcode port 443 for both modes, so **GATEWAY-HTTP
and GATEWAY-TLS profiles must be run separately**. Running all three in one
invocation causes TLSRoute data-plane tests to time out.

`in-cluster-runner.sh` handles this automatically with `--clean-slate`:

- Detects which profiles are active and applies only the matching manifests
- For TLS-only runs, sets `EDGION_BASE_MANIFESTS` to
  `base-manifests-tls-only.yaml` (upstream base manifest with HTTP Gateways
  stripped), preventing the framework from recreating port-443 HTTPS listeners
- Restarts both controller and gateway processes to ensure clean state
- Saves reports to `reports/` with profile-derived filenames

```bash
# Run 1: HTTP + gRPC
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC" --timeout 30m
# → reports/gateway-http-grpc-conformance-report.yaml

# Run 2: TLS only (separate gateway startup config)
./scripts/in-cluster-runner.sh --clean-slate \
  --conformance-profiles "GATEWAY-TLS" --timeout 30m
# → reports/gateway-tls-conformance-report.yaml
```

See cookbook 2.3.1 for technical details and affected tests.
See `reports/README.md` for current results and known failures.

### Specifying features individually (overrides conformance_test.go)

```bash
bash scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant"
```

### Port-Forward mode (TAL)

Use this when cluster pod IPs are unreachable. TAL (Transport Access Layer) handles address rewriting in a unified way and supports flexible port-mapping configuration.

**Quick enable:**

```bash
bash scripts/run.sh --port-forward
```

Automatically starts `kubectl port-forward` to the edgion-gateway pod, binding default local ports (HTTP:18080, HTTPS:18443, HTTPS-8443:28443).

**Advanced — unified port-mapping table (recommended):**

```bash
export GATEWAY_PROXY_PORT_MAP="80:18080,443:18443,8443:28443"
./scripts/run.sh --port-forward
```

`GATEWAY_PROXY_PORT_MAP` format: `gwPort:localPort,gwPort:localPort,...`, allowing each gateway port to have its own local forwarding port.

**Backward-compatible (specify individually):**

```bash
export GATEWAY_PROXY_HOST=127.0.0.1
export GATEWAY_PROXY_HTTP_PORT=18080
export GATEWAY_PROXY_HTTPS_PORT=18443
export GATEWAY_PROXY_HTTPS_8443_PORT=28443
./scripts/run.sh --port-forward
```

**Enable gRPC proxy:**

```bash
GATEWAY_PROXY_ENABLE_GRPC=true ./scripts/run.sh --port-forward
```

For the `GRPCRoute*` test family.

**Debug TAL address rewriting:**

```bash
TAL_DEBUG=true ./scripts/run.sh --port-forward
```

Look for `[TAL] resolved ...` lines in the test output.

## Update binaries (localpatch)

```bash
# Update only the controller
./examples/gateway-api-conformance/scripts/orbstack-dev.sh localpatch c

# Update controller + gateway
./examples/gateway-api-conformance/scripts/orbstack-dev.sh localpatch
```

**Note about pod process structure**: PID 1 is a shell loop (`while true; do /usr/local/edgion/edgion-controller; done`). Do not kill PID 1, otherwise OrbStack will restart the container from the original image (losing the new binary that was copied in).
Kill the controller subprocess so the shell loop restarts with the new binary:

```bash
POD=$(kubectl get pods -n edgion-system -l app=edgion-controller -o jsonpath='{.items[0].metadata.name}')
CTRL_PID=$(kubectl exec -n edgion-system $POD -- sh -c \
  "for p in \$(ls /proc | grep '^[0-9]*$'); do \
    if [ -f /proc/\$p/cmdline ] && grep -q edgion-controller /proc/\$p/cmdline 2>/dev/null && [ \$p != 1 ]; then echo \$p; fi; \
  done" | head -1)
kubectl exec -n edgion-system $POD -- sh -c "kill -9 $CTRL_PID"
```

## Status condition debugging

**The root cause of most conformance test failures is incorrect status condition settings**, not routing / data-plane issues.

### Debug workflow

1. Run a single failing test and inspect which condition the `context deadline exceeded` belongs to:
   ```
   Programmed condition set to Status False with Reason Invalid, expected Status True
   ```

2. Check the actual status of the resource in the cluster:
   ```bash
   kubectl get gateway <name> -n gateway-conformance-infra -o yaml | grep -A 30 "status:"
   ```

3. Compare with the logic in `update_status()` and `update_listener_conditions()` in `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs`

### Common status issues

| Symptom | Root cause | Fix location |
|---------|------------|--------------|
| `Programmed=False(Invalid)` but expected `True` | Conflict-detection misjudgment / unresolved-refs logic problem | `listener_port_manager.rs` / `update_listener_conditions()` |
| `Programmed=True` but expected `False` | `has_unresolved_refs` not factored into `Programmed` decision | `update_listener_conditions()` |
| `AttachedRoutes=0` but expected `>0` | Route tracker not counted; gateway with unresolved refs is not counted | `tracker.count_for_listener()` in `update_status()` |
| `ResolvedRefs=False(RefNotPermitted)` misjudgment | ReferenceGrant resolution logic | `validate_listener_resolved_refs()` |

### Hostname conflict-detection rule (important)

The Gateway API spec defines `Conflicted` as **exactly identical** socket-address+hostname combinations (the conflict-detection key is grouped by socket address via `make_conflict_key(SocketAddr)`), not as hostname intersection.

- `None` (wildcard) + `None` → **Conflicted** (both are wildcards)
- `foo.example.com` + `foo.example.com` → **Conflicted**
- `None` + `foo.example.com` → **not conflicting** (intersect but different)
- `*.example.com` + `foo.example.com` → **not conflicting** (intersect but different)

Mistakenly treating "intersection" as "conflict" causes multiple test failures (e.g. `GatewayHTTPListenerIsolation`).

## Latest conformance results (feature-04-06, 2026-04-07)

**ALL PASS** (in-cluster mode, split-profile run)

- GATEWAY-HTTP: Core 33/33, Extended 32/32
- GATEWAY-GRPC: Core 13/13
- GATEWAY-TLS: Core 18/18, Extended 3/3

For details see the profile status table in `SKILL.md` and `reports/README.md`.

## In-cluster execution (recommended)

For remote clusters, unstable Pod IPs, or unreliable gRPC over `kubectl port-forward`, prefer the script bundled with the repository:

`examples/gateway-api-conformance/scripts/in-cluster-runner.sh`

The script handles uniformly:
- Cross-compiling the conformance test binary (currently fixed at `linux/amd64`)
- Creating / reusing the `conformance-runner` Pod and RBAC
- Copying the test binary into the Pod
- Executing conformance inside the Pod and pulling the report back to the project directory

### Most common commands

```bash
cd examples/gateway-api-conformance

# Full run: compile + deploy/reuse runner + execute
./scripts/in-cluster-runner.sh \
  --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC,GATEWAY-TLS" \
  --timeout 30m

# Re-run only, do not recompile
./scripts/in-cluster-runner.sh \
  --run-only \
  --conformance-profiles "GATEWAY-TLS" \
  --timeout 20m

# Force re-copy of the binary
./scripts/in-cluster-runner.sh \
  --force-copy \
  --conformance-profiles "GATEWAY-TLS" \
  --timeout 20m

# Single test
./scripts/in-cluster-runner.sh \
  --run-only \
  --conformance-profiles "GATEWAY-TLS" \
  --run-test "TLSRouteHostnameIntersection" \
  --timeout 10m

# Clean up runner Pod + RBAC
./scripts/in-cluster-runner.sh --cleanup
```

### Output and notes

- Reports are pulled back into `examples/gateway-api-conformance/reports/`; filenames are derived from the profile:
  - `gateway-http-grpc-conformance-report.yaml`
  - `gateway-tls-conformance-report.yaml`
- The script internally always passes `-cleanup-base-resources=false` to prevent the conformance framework from removing the base Gateway and leaving subsequent test cases without bound ports
- `--clean-slate` automatically rolls out a restart of the TLS backend pods to ensure they load the new certificate after the cert Secret is recreated
- If your cluster nodes are not `linux/amd64`, do not copy the script verbatim — adjust `GOARCH` first
- For deeper failure modes, TLS pre-bind, and TLS-only profile troubleshooting, go to [06-conformance-debug-cookbook.md](06-conformance-debug-cookbook.md)
