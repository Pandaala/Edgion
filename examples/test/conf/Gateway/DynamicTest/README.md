# Gateway Dynamic Configuration Test Guide

This directory contains the manifests used to verify runtime updates for Gateway resources.

## Directory Layout

```text
DynamicTest/
├── initial/          # Initial configuration loaded first
├── updates/          # Runtime updates loaded second
├── delete/           # List of resources to delete
└── README.md         # This file
```

## Test Scenarios

### Scenario 1: Remove a Gateway Hostname Constraint

**Port:** `31250` (`http-with-hostname` listener)

| Phase | Configuration | Test Request | Expected Result |
|-------|---------------|--------------|-----------------|
| Initial | `listener.hostname=api.example.com` | `Host: other.example.com -> /match` | `404` because the hostname does not match |
| Updated | Remove the hostname constraint | `Host: other.example.com -> /match` | `200` because the constraint is gone |

**Files:**
- Initial: `01_Gateway.yaml` with a hostname on the listener
- Update: `Gateway_remove_hostname.yaml`

**Verification:** A request that was previously blocked by the hostname constraint should succeed after the update.

### Scenario 2: Change `AllowedRoutes` from `Same` to `All`

**Port:** `31251` (`http-same-ns` listener)

| Phase | Configuration | Test Route | Expected Result |
|-------|---------------|------------|-----------------|
| Initial | `AllowedRoutes=Same` | `HTTPRoute` in `edgion-other` | `404` because the namespace differs |
| Updated | `AllowedRoutes=All` | The same `HTTPRoute` | `200` because cross-namespace access is now allowed |

**Files:**
- Initial: `01_Gateway.yaml` with `AllowedRoutes=Same`, plus `HTTPRoute_cross_namespace.yaml`
- Update: `Gateway_remove_hostname.yaml` with `AllowedRoutes=All`

**Verification:** The route changes from unreachable to reachable across namespaces.

### Scenario 3: Add an `HTTPRoute` Dynamically

**Port:** `31252` (`http-general` listener)

| Phase | Configuration | Test Request | Expected Result |
|-------|---------------|--------------|-----------------|
| Initial | No `/new-api` route | `GET /new-api` | `404` because the route does not exist |
| Updated | Add an `HTTPRoute` for `/new-api` | `GET /new-api` | `200` because the new route becomes active |

**Files:**
- Initial: none
- Update: `HTTPRoute_add_new.yaml`

**Verification:** The new route becomes available immediately after the update.

### Scenario 4: Update an `HTTPRoute` Match Rule

**Port:** `31252` (`http-general` listener)

| Phase | Configuration | Test Request | Expected Result |
|-------|---------------|--------------|-----------------|
| Initial | Match `GET /api/v1/*` | `POST /api/v1/users` | `404` because the method does not match |
| Updated | Match `POST /api/v1/*` | `POST /api/v1/users` | `200` because the rule changed |

**Files:**
- Initial: `HTTPRoute_get_only.yaml` with `method=GET`
- Update: `HTTPRoute_update_match.yaml` with `method=POST`

**Verification:** The new match rule takes effect immediately.

### Scenario 5: Delete an `HTTPRoute` Dynamically

**Port:** `31252` (`http-general` listener)

| Phase | Configuration | Test Request | Expected Result |
|-------|---------------|--------------|-----------------|
| Initial | `HTTPRoute` for `/temp` exists | `GET /temp` | `200` because the route exists |
| Updated | Delete that `HTTPRoute` | `GET /temp` | `404` because the route is gone |

**Files:**
- Initial: `HTTPRoute_temp.yaml`
- Delete: `delete/resources_to_delete.txt` containing `HTTPRoute/edgion-test/route-temp`

**Verification:** The route becomes unreachable immediately after deletion.

## Usage

### 1. Load the Initial State

```bash
# load_conf.sh automatically skips updates/ and delete/
./examples/test/scripts/utils/load_conf.sh Gateway/DynamicTest
```

### 2. Run the Initial-Phase Tests

```bash
./target/debug/examples/test_client -g -r Gateway -i Dynamic --phase initial
```

### 3. Apply Dynamic Updates

```bash
# Apply updated manifests
./target/debug/edgion-ctl --server http://127.0.0.1:5800 \
    apply -f examples/test/conf/Gateway/DynamicTest/updates/

# Delete resources listed in the delete file
while read -r resource; do
    [ -z "$resource" ] || [[ "$resource" =~ ^# ]] && continue
    ./target/debug/edgion-ctl --server http://127.0.0.1:5800 delete "$resource"
done < examples/test/conf/Gateway/DynamicTest/delete/resources_to_delete.txt

# Verify controller and gateway state stay in sync
./target/debug/examples/resource_diff \
    --controller-url http://127.0.0.1:5800 \
    --gateway-url http://127.0.0.1:5900

# Give the runtime a moment to swap in the new config
sleep 3
```

### 4. Run the Update-Phase Tests

```bash
./target/debug/examples/test_client -g -r Gateway -i Dynamic --phase update
```

### 5. Run the Full Automated Flow

```bash
./examples/test/scripts/integration/run_integration.sh -r Gateway --dynamic-test
```

## Port Allocation

- `31250`: scenario 1, hostname constraint
- `31251`: scenario 2, `AllowedRoutes`
- `31252`: scenarios 3-5, `HTTPRoute` add/update/delete

## Notes

1. Load `initial/` and finish the initial-phase tests before applying `updates/`.
2. After the suite finishes, restart services or clean up resources if you want a fresh rerun.
3. Wait 2-3 seconds after dynamic updates so the `ArcSwap` state is visible everywhere.
4. Repeated runs may require cleanup from previous test data.
