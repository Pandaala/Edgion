---
name: 06-tracing
description: Debugging and troubleshooting skill for Edgion. Use when local code changes break routing, plugins, sync, readiness, or runtime behavior.
---

# 06 Debugging and Troubleshooting

> Daily development troubleshooting: quickly locate the layer of an issue after a code change.
> Toolchain: Admin API, edgion-cli three views, Access Log Store, Metrics, resource_diff.

## File list

| File | Topic | Status |
|------|------|------|
| [00-debugging.md](00-debugging.md) | Daily development debugging, Admin API, edgion-cli, Access Log Store, Metrics, quick lookup | Refactored |

## Quick entry points

### Fastest local debugging

```bash
./examples/test/scripts/integration/run_integration.sh --keep-alive -r <Resource> -i <Item>
```

### Three views to locate the issue layer

```bash
./target/debug/edgion-cli get httproute -n default
./target/debug/edgion-cli -t server get httproute -n default
./target/debug/edgion-cli -t client get httproute -n default
```

### Related knowledge

- TLS-specific troubleshooting: [../09-misc/debugging-tls-gateway.md](../09-misc/debugging-tls-gateway.md)
- Running integration tests: [../05-testing/01-integration-testing.md](../05-testing/01-integration-testing.md)
