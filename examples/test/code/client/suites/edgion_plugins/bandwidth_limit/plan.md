# BandwidthLimit Plugin Integration Test Plan

## Objective
Verify that the `BandwidthLimit` plugin correctly limits the downstream response bandwidth according to the configured rate.

## Test Environment
- **Gateway**: `edgion-test` namespace
- **Backend**: `test-http` service (Test Server)
- **Plugin Config**: `BandwidthLimit` attached via `EdgionPlugins` CRD
- **Test Route**: `bandwidth-limit.example.com/test/bandwidth-limit`

## Test Scenarios

### 1. Functional Verification (Time-based)
**Goal**: Verify that downloading a large file takes at least the expected duration.
- **Config**: Rate = `100KB/s` (102400 bytes/s)
- **Action**: Client sends POST request with `100KB` body to `/echo` endpoint.
- **Server**: Echoes back the `100KB` body.
- **Expected Behavior**:
  - Response body size: `~100KB`
  - Duration: `> 1.0 seconds` (plus some overhead)
  - Verify `duration >= size / rate`

### 2. Metrics Verification
**Goal**: Verify that the gateway records the correct latency metrics indicating throttling occurred.
- **Config**: Enable test mode with `edgion.io/metrics-test-type: "latency"` annotation.
- **Action**:
  - Perform the request from Scenario 1.
  - Use a unique `test_key` (e.g., `bw_test_<timestamp>`).
- **Verification**:
  - Fetch `edgion_backend_requests_total` metrics for the specific `test_key`.
  - Analyze `latency_ms` field in `test_data`.
  - Assert that `avg_latency_ms` correlates with the expected throttling duration (`~1000ms`).

### 3. Fail-open Verification (Invalid Rate)
**Goal**: Verify that an invalid rate configuration does not block traffic.
- **Config**: Rate = `0` or invalid string (simulated or configured if possible).
- **Action**: Send request.
- **Expected Behavior**: Request succeeds immediately (no throttling).

## Configuration Files
- `Edgion/examples/test/conf/EdgionPlugins/BandwidthLimit/01_BandwidthLimit_plugin.yaml`
- `Edgion/examples/test/conf/EdgionPlugins/BandwidthLimit/HTTPRoute_bandwidth_limit.yaml`

## Test Code
- `Edgion/examples/test/code/client/suites/edgion_plugins/bandwidth_limit/bandwidth_limit.rs`
