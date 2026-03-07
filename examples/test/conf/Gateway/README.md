# Gateway Integration Test Guide

This directory contains the integration test manifests and test scenarios for Gateway API behavior.

## Suite Layout

### 1. Listener Hostname Constraints (`ListenerHostname/`)

Verifies listener hostname matching behavior:

- Exact hostname match: `api.example.com`
- Wildcard hostname match: `*.wildcard.example.com`
- Wildcard does not match the root domain: `wildcard.example.com`
- No hostname restriction: the listener accepts any compatible `HTTPRoute` hostname

**Ports:** `31240-31242`  
**Entry point:** `./test_client -g -r Gateway -i ListenerHostname`

### 2. AllowedRoutes (`AllowedRoutes/`)

Verifies `AllowedRoutes` constraints on Gateway listeners.

#### 2.1 Same Namespace (`AllowedRoutes/Same/`)

- Same-namespace routes are allowed.
- Cross-namespace routes are rejected with `404`.

**Port:** `31210`  
**Entry point:** `./test_client -g -r Gateway -i AllowedRoutes/Same`

#### 2.2 All Namespaces (`AllowedRoutes/All/`)

- Same-namespace routes are allowed.
- Cross-namespace routes are also allowed.

**Port:** `31211`  
**Entry point:** `./test_client -g -r Gateway -i AllowedRoutes/All`

#### 2.3 Kinds (`AllowedRoutes/Kinds/`)

- `HTTPRoute` is allowed.
- `GRPCRoute` is rejected.

**Port:** `31213`  
**Entry point:** `./test_client -g -r Gateway -i AllowedRoutes/Kinds`

#### 2.4 Selector (`AllowedRoutes/Selector/`)

- Routes in namespaces with matching labels are allowed.
- Cross-namespace routes that do not match the selector are rejected.

Current fixture assumptions:

- The `edgion-test` namespace has label `env=prod`.
- The Gateway is created in `edgion-test`.
- `selector-same-ns-route` should succeed.
- `selector-cross-ns-route` is in `edgion-default` and should return `404`.

**Port:** `31276`  
**Entry point:** `./test_client -g -r Gateway -i AllowedRoutes/Selector`

### 3. Combined Scenarios (`Combined/`)

Verifies how multiple listener constraints work together.

#### 3.1 Listener Hostname + AllowedRoutes

- Hostname match + same namespace -> allowed
- Hostname match + different namespace -> rejected
- Same namespace + hostname mismatch -> rejected

#### 3.2 `sectionName` + Listener Hostname

- `sectionName` match + hostname match -> allowed
- `sectionName` match + hostname mismatch -> rejected

**Ports:** `31230-31232`  
**Entry point:** `./test_client -g -r Gateway -i Combined`

## Edge Cases

The existing suites already cover these cases implicitly:

1. Default `parentRef.namespace`
   The route namespace is used when `parentRef.namespace` is omitted.

2. Routes without `hostnames`
   Listener hostname constraints still apply when an `HTTPRoute` omits the `hostnames` field.

## Running the Tests

### Run All Gateway Suites

```bash
cd examples/test
./scripts/integration/run_integration.sh -r Gateway
```

### Run a Specific Suite

```bash
# Listener hostname tests
./scripts/integration/run_integration.sh -r Gateway -i ListenerHostname

# AllowedRoutes Same
./scripts/integration/run_integration.sh -r Gateway -i AllowedRoutes/Same

# AllowedRoutes All
./scripts/integration/run_integration.sh -r Gateway -i AllowedRoutes/All

# AllowedRoutes Kinds
./scripts/integration/run_integration.sh -r Gateway -i AllowedRoutes/Kinds

# AllowedRoutes Selector
./scripts/integration/run_integration.sh -r Gateway -i AllowedRoutes/Selector

# Combined scenarios
./scripts/integration/run_integration.sh -r Gateway -i Combined
```

## Configuration Notes

### File Naming

Gateway manifests use the `01_Gateway.yaml` prefix so the Gateway is loaded before dependent `HTTPRoute` objects.

### Port Allocation

- `ListenerHostname`: `31240-31242`
- `AllowedRoutes/Same`: `31210`
- `AllowedRoutes/All`: `31211`
- `AllowedRoutes/Kinds`: `31213`
- `Combined`: `31230-31232`

Avoid conflicts with existing ports such as `31200`, which is already used by `EdgionTls`.

### 4. Dynamic Configuration Tests (`DynamicTest/`)

These tests verify runtime updates for Gateway resources.

1. Gateway hostname constraint removal
   Initial state: a non-matching hostname is rejected with `404`.
   Update: remove the hostname constraint.
   Verification: the same hostname becomes reachable with `200`.

2. AllowedRoutes change (`Same -> All`)
   Initial state: cross-namespace routes are rejected with `404`.
   Update: allow all namespaces.
   Verification: the cross-namespace route becomes reachable with `200`.

3. Dynamic HTTPRoute add
   Initial state: the route does not exist and returns `404`.
   Update: add a new `HTTPRoute`.
   Verification: the new route becomes reachable with `200`.

4. Dynamic HTTPRoute match update
   Initial state: only `GET` matches and `POST` returns `404`.
   Update: switch the match to `POST`.
   Verification: `POST` becomes reachable with `200`.

5. Dynamic HTTPRoute delete
   Initial state: the route exists and returns `200`.
   Update: delete the `HTTPRoute`.
   Verification: the route becomes unreachable with `404`.

**Ports:** `31250-31252`  
**Entry point:** `./test_client -g -r Gateway -i Dynamic --phase initial|update`  
**Full flow:** `./run_integration.sh -r Gateway --dynamic-test`

See [DynamicTest/TEST_SUMMARY.md](DynamicTest/TEST_SUMMARY.md) for details.

## Coverage Summary

| Feature | Scenario | Status |
|---------|----------|--------|
| Listener hostname exact match | Positive and negative coverage | Complete |
| Listener hostname wildcard match | Positive and negative coverage | Complete |
| Listener without hostname | Positive coverage | Complete |
| AllowedRoutes Same Namespace | Positive and negative coverage | Complete |
| AllowedRoutes All Namespaces | Cross-namespace positive coverage | Complete |
| AllowedRoutes Selector | Positive namespace-label coverage and negative cross-namespace coverage | Complete |
| AllowedRoutes Kinds | `HTTPRoute` allowed and `GRPCRoute` rejected | Complete |
| Combined hostname + AllowedRoutes | Positive coverage and two negative cases | Complete |
| Combined `sectionName` + hostname | Positive and negative coverage | Complete |
| Default `parentRef.namespace` | Implicitly covered | Complete |
| Route without `hostnames` | Implicitly covered | Complete |
| Dynamic Gateway hostname removal | `404 -> 200` transition | Complete |
| Dynamic AllowedRoutes update | `404 -> 200` transition | Complete |
| Dynamic HTTPRoute add/update/delete | Three CRUD scenarios | Complete |

## References

- [Kubernetes Gateway API Specification](https://gateway-api.sigs.k8s.io/)
- Implementation: `src/core/gateway/runtime/matching/route.rs` (`check_gateway_listener_match`)
- Test code: `examples/code/client/suites/gateway/`
