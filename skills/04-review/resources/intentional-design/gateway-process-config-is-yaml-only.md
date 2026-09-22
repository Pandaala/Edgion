---
name: gateway-process-config-is-yaml-only
description: Use when reviewing whether Pingora process settings belong to local Gateway YAML or EdgionGatewayConfig, or whether GatewayClass server settings hot-reload.
knowledge_type: intentional-design
---

# Gateway process configuration is local YAML only

## Conclusion

The Gateway process owns Pingora construction settings through the local YAML
configuration loaded by `edgion-gateway/src/cli/config.rs` (default path
`config/edgion-gateway.yaml`). These fields are resolved once by
`create_server_conf()` before `Server::new_with_opt_and_conf()`:

- `threads`
- `work_stealing`
- `grace_period_seconds`
- `graceful_shutdown_timeout_seconds`
- `upstream_keepalive_pool_size`
- `error_log`

They are local-only, process-wide, and restart-bound. They must not be exposed by
the cluster-synced `EdgionGatewayConfig` CRD.

`EdgionGatewayConfig.spec.server` instead owns two GatewayClass-scoped HTTP
runtime settings: `enableCompression` and
`downstreamKeepaliveRequestLimit`. Updates are published through the shared EGC
runtime snapshot without rebuilding the listener: compression is captured per
request, while the first HTTP/1 request captures the keepalive budget for that
connection. The local Gateway YAML and CLI must not expose a duplicate downstream
request limit.

## Why ownership is split

Pingora process construction needs one value before the serving loop starts,
while the cluster store may legitimately contain multiple
`EdgionGatewayConfig` resources. Selecting or aggregating class-specific values
for worker topology, shutdown, logging, or a process-global upstream pool would
invent precedence with no resource-model basis. Local deployment configuration
already provides the unambiguous per-process owner.

The remaining CR fields have a natural GatewayClass owner: listeners built for
Gateways in different classes may request different HTTP behavior. They are not
process-construction settings merely because they live under a field named
`server`.

## Regression boundary

The Rust schema and committed CRD must expose exactly these
`EdgionGatewayConfig.spec.server` fields:

```text
enableCompression
downstreamKeepaliveRequestLimit
```

`edgion-resources/tests/crd_coverage.rs` checks this exact set in addition to
the general struct-to-CRD coverage comparison. D0006 removed the six ignored
process-field copies from the resource schema and the ignored local downstream
limit from the Gateway CLI schema.

## Configuration format history

Gateway and Controller process configuration files are YAML, not TOML. The
Gateway loader uses `serde_yaml` with `singleton_map_recursive`. Historical
documentation and examples were corrected under `types-misc-02`; Cargo feature
tables, `deny.toml`, and `rust-toolchain.toml` remain genuine TOML.

`AccessLogOptions` uses camel-case serialized names, including `includeRawUri`.
This format history is independent of the process-versus-CRD ownership rule.

## Re-evaluation triggers

- Pingora supports safely reconfiguring one of these process settings after
  `Server::run()` begins.
- Edgion introduces an explicit singleton bootstrap resource with defined
  precedence over local process configuration.
- The EGC runtime snapshot or request/connection capture boundaries change.

## Resolution references

- `types-misc-02` — 2026-05-25.
- `D0006` / `define-gatewayclass-process-config-ownership`.
