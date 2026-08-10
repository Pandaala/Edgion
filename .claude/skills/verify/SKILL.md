---
name: verify
description: Build, launch, and drive the Edgion controller+gateway stack locally to observe a data-plane change end-to-end.
---

# Verifying Edgion data-plane changes locally

## Build (debug)

```bash
cargo build                                # edgion-controller, edgion-gateway, edgion-cli (default-members)
cargo build -p edgion-tests --bin test_server --bin resource_diff --bin test_client
```

All binaries land in `target/debug/`. The start script hard-requires
`test_server`, `resource_diff`, and `edgion-cli`.

## Launch the stack

```bash
./edgion-tests/integration/scripts/utils/start_all_with_conf.sh \
  --suites "HTTPRoute/Backend/BackendTLS"        # comma-separated Resource/Item dirs; omit for all
```

- Starts test_server (:30001) → controller (Admin API :12101) → gateway, then
  loads `edgion-tests/integration/conf/<suite>/` YAMLs via `edgion-cli apply`.
- `base/` (GatewayClass, EdgionGatewayConfig) always loads; suites are
  `Resource/Item` paths relative to `conf/`.
- Work dir (logs, pids, config) is printed at the end:
  `tmp/integration_testing/testing_<ts>/logs/{gateway.log,controller.log,load_config.log}`.
- Stop everything: `./edgion-tests/integration/scripts/utils/kill_all.sh`.
- NOTE: the `examples/test/scripts/...` path in the ws2 workspace CLAUDE.md is
  stale; scripts live under `edgion-tests/integration/scripts/utils/`.

## Apply ad-hoc resources

```bash
target/debug/edgion-cli --server http://127.0.0.1:12101 apply -f <file>.yaml
```

Auth is handled automatically (test token). Model custom scenarios on existing
suite YAMLs (Gateway + HTTPRoute + Service + EndpointSlice [+ BackendTLSPolicy]);
EndpointSlices can point at any local 127.0.0.1 port.

## Drive it

- Gateway listeners bind the ports declared in each suite's `Gateway.yaml`
  (e.g. BackendTLS listener is :31170). Route with
  `curl -H "Host: <route hostname>" http://127.0.0.1:<listener port><route path>`.
- Routes often match specific PathPrefixes — read the HTTPRoute before blaming
  routing; a 404 page with `<center>edgion</center>` is the gateway's own miss
  page, not a backend response.
- Known-good happy path for gateway→backend TLS:
  `curl -H "Host: test.example.com" http://127.0.0.1:31170/backend-tls/` → 200.
- Upstream error attribution shows up in `logs/gateway.log`
  (`Fail to proxy: ... tries: N`); access logs in the same dir.

## Gotchas

- Default retry policy retries connect-class failures (observed `tries: 4`), so
  a per-attempt timeout appears ×4 in wall-clock at the client.
- pingora 0.8.1 wraps the upstream TLS handshake in `connection_timeout`
  (per-phase), in addition to any `total_connection_timeout`.
