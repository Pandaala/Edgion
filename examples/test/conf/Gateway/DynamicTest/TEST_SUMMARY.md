# Gateway Dynamic Configuration Test Summary

## Goal

Validate that Gateway and `HTTPRoute` configuration can be updated at runtime and take effect immediately.

## Test Scenarios

### Scenario 1: Dynamic Gateway Hostname Removal

**Goal:** Verify that a Gateway listener hostname constraint can be removed dynamically.

- Initial state: listener `http-with-hostname` on port `31250` is restricted to `hostname: api.example.com`
- Initial check: requests to `other.example.com` are rejected with `404`
- Dynamic update: remove the hostname field through the API
- Final check: requests to `other.example.com` now succeed with `200/502`

**Key point:** Gateway runtime updates correctly apply hostname changes.

### Scenario 2: Dynamic `HTTPRoute` Method Update

**Goal:** Verify that an `HTTPRoute` match rule can be changed dynamically.

- Initial state: the `HTTPRoute` matches `GET /api/v1`
- Initial check: `GET` succeeds with `200/502`
- Dynamic update: switch the route to `POST /api/v1`
- Final check: `GET` fails with `404`, `POST` succeeds with `200/502`

**Key point:** `HTTPRoute` match rules update correctly at runtime.

## File Layout

```text
DynamicTest/
├── initial/              # Initial configuration
│   ├── 01_Gateway.yaml                 # Gateway with 2 listeners
│   ├── HTTPRoute_hostname_match.yaml   # Hostname-focused route
│   └── HTTPRoute_method.yaml           # Matches GET /api/v1
└── updates/              # Runtime updates
    ├── Gateway_remove_hostname.yaml    # Removes hostname
    └── HTTPRoute_method_update.yaml    # Switches to POST /api/v1
```

## Test Flow

1. Start the Gateway and Controller.
2. Load the `initial/` directory through the API.
3. Run the initial checks:
   `other.example.com -> 404` because the hostname restriction is active.
   `GET /api/v1 -> 200/502` because the original method rule matches.
4. Apply `updates/` with `edgion-ctl apply`.
5. Wait 2 seconds.
6. Run the update-phase checks:
   `other.example.com -> 200/502` after removing the hostname restriction.
   `GET -> 404` and `POST -> 200/502` after updating the method rule.
7. Verify configuration sync with `resource_diff`.

## Running the Test

```bash
cd /Users/caohao/ws1/Edgion
./examples/test/scripts/integration/run_integration.sh -r Gateway --dynamic-test
```

## Expected Output

```text
[✓] Initial Phase Tests (2/2 passed)
[✓] Dynamic Update Applied
[✓] Update Phase Tests (2/2 passed)
[✓] Resource Sync Verified
```

## Technical Details

### Gateway Runtime Behavior

- Static parts: listener ports and protocols still require a restart.
- Dynamic parts: `hostname`, `allowedRoutes`, and similar fields are swapped in place through `ArcSwap`.

### `HTTPRoute` Runtime Behavior

- All route fields are designed to update dynamically.
- Changes are expected to become visible in under one second.

### Implementation Pieces

1. `GatewayConfigStore`: global runtime configuration store backed by `ArcSwap`
2. `ConfHandler<Gateway>`: reacts to configuration change events
3. Controller API: supports create and update operations for `Gateway` and `HTTPRoute`

## Key Files

- `/src/core/gateway/runtime/store/config.rs`: configuration store
- `/src/core/gateway/runtime/handler.rs`: Gateway handler
- `/src/core/controller/api/namespaced_handlers.rs`: Controller API
- `/examples/test/scripts/utils/load_conf.sh`: configuration loader
