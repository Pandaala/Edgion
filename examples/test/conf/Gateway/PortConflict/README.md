# Gateway Port Conflict Detection Test

This test suite validates the port conflict detection feature as per Gateway API specification.

## Gateway API Requirements

According to Gateway API spec (gateway-api-standard-v1.4.0.yaml):

1. **MUST**: Set `Conflicted` condition to `True` on conflicting Listeners
2. **MUST NOT**: Pick a "winner" - all indistinct Listeners must not be accepted
3. **MUST**: Set `ListenersNotValid` condition on Gateway status when conflicts exist
4. **MAY**: Accept Gateway with Conflicted Listeners, but only accept non-conflicting subset

## Distinct Listener Rules

- **HTTP, HTTPS, TLS**: Port + Hostname must be different
- **TCP, UDP**: Port must be different (hostname not considered)

## Test Scenarios

### 1. Single Gateway Internal Conflict (`Gateway_internal_conflict.yaml`)

One Gateway with two Listeners using the same port:
- Listener `http-1` on port 31260
- Listener `http-2` on port 31260 (conflict!)

**Expected Result**:
- Both Listeners marked as `Conflicted=True`
- Gateway marked as `ListenersNotValid=True`
- Gateway runtime skips both conflicting Listeners

### 2. Cross-Gateway Conflict (`Gateway_cross_conflict_A.yaml`, `Gateway_cross_conflict_B.yaml`)

Two Gateways each with one Listener using the same port:
- Gateway A: Listener `http` on port 31261
- Gateway B: Listener `http` on port 31261 (conflict!)

**Expected Result**:
- Both Gateways' Listeners marked as `Conflicted=True`
- Both Gateways marked as `ListenersNotValid=True`
- Neither Listener is bound at runtime

### 3. Same Port Different Hostname - HTTP (No Conflict) (`Gateway_same_port_diff_hostname.yaml`)

One Gateway with two HTTP Listeners on same port but different hostnames:
- Listener `api` on port 31262, hostname `api.example.com`
- Listener `web` on port 31262, hostname `web.example.com`

**Expected Result**:
- No conflict (HTTP allows same port with different hostnames)
- Both Listeners marked as `Conflicted=False`
- Gateway status shows no `ListenersNotValid` condition

## Port Allocation

| Scenario | Port |
|----------|------|
| Internal conflict | 31260 |
| Cross-Gateway conflict | 31261 |
| Same port diff hostname | 31262 |

## Running Tests

```bash
# Load test configurations
./examples/test/scripts/utils/load_conf.sh Gateway/PortConflict

# Verify via admin API
curl http://127.0.0.1:5900/admin/Gateway | jq '.data[] | {name: .metadata.name, status: .status}'
```

## Verifying Results

Check Gateway status for conflict conditions:

```bash
# Check for Conflicted condition on Listeners
curl -s http://127.0.0.1:5900/admin/Gateway | jq '.data[].status.listeners[] | {name, conditions}'

# Check for ListenersNotValid condition on Gateway
curl -s http://127.0.0.1:5900/admin/Gateway | jq '.data[] | select(.status.conditions[]? | .type == "ListenersNotValid")'
```
