# LinkSys Development Guide

Use this for tasks such as:

- Adding or extending an external system connection capability in `LinkSys`.
- Adjusting runtime wiring for Redis / Etcd / Elasticsearch / Kafka / Webhook / HttpDns.
- Troubleshooting "the LinkSys YAML has synced but the gateway-side runtime client / webhook manager has not updated".

## Read these real entry points first

- [../../../edgion-resources/src/resources/link_sys/mod.rs](../../../edgion-resources/src/resources/link_sys/mod.rs)
- [../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs](../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs)
- [../../../edgion-gateway/src/conf_sync/conf_client/config_client.rs](../../../edgion-gateway/src/conf_sync/conf_client/config_client.rs)
- [../../../edgion-gateway/src/link_sys/runtime/conf_handler.rs](../../../edgion-gateway/src/link_sys/runtime/conf_handler.rs)
- [../../../edgion-gateway/src/link_sys/runtime/store.rs](../../../edgion-gateway/src/link_sys/runtime/store.rs)
- [../../../edgion-gateway/src/link_sys/providers/webhook/mod.rs](../../../edgion-gateway/src/link_sys/providers/webhook/mod.rs)

## First, identify which kind of change you are making

LinkSys is no longer "one resource kind per external system" — it is a single `LinkSys` CRD plus `SystemConfig` variants:

- `Redis`
- `Etcd`
- `Elasticsearch`
- `Webhook`
- `Kafka`
- `HttpDns`

Another easy point of confusion:

- The `local_file` provider lives at `edgion-gateway/src/link_sys/providers/local_file/`.
- It is not a `LinkSys` `SystemConfig` variant — it is a local `DataSender` for log persistence.

So first decide:

1. Are you adding a new `SystemConfig` variant to the existing `LinkSys`?
2. Are you extending the runtime client / ops / config mapping of an existing provider?
3. Do you actually only need a non-CRD internal provider (similar to `local_file`)?

For most LinkSys development tasks, the answer is 1 or 2 — not "add a new resource kind".

## The current real mental model

The current main pipeline:

1. `LinkSys` YAML enters the controller.
2. Both `impl_resource_meta!(LinkSys, ...)` and the gateway-side `ConfHandler` call `validate_config()`.
3. The Controller's `LinkSysHandler` mainly handles standard status updates, with no complex dependency resolution.
4. The gateway `ConfigClient` has a dedicated `ClientCache<LinkSys>`.
5. That cache registers `create_link_sys_handler()`.
6. `LinkSysStore.full_set()` / `partial_update()` dispatches resources to each runtime manager.
7. Callers ultimately get the runtime client through typed APIs, e.g.:
   - `get_redis_client("namespace/name")`
   - `get_etcd_client("namespace/name")`
   - `get_es_client("namespace/name")`
   - `get_kafka_client("namespace/name")`
   - `get_webhook_manager().get("namespace/name")`

In short:

- The Controller side mainly manages "is the resource valid, is the status correct".
- The Gateway side `LinkSysStore` is the runtime lifecycle entry point.

## Subsystems already landed

### 1. Redis / Etcd / Elasticsearch

These three follow the pattern "CRD config -> runtime client -> typed store / ops".

The directory typically looks like:

- `edgion-resources/src/resources/link_sys/<system>.rs`
- `edgion-gateway/src/link_sys/providers/<system>/client.rs`
- `edgion-gateway/src/link_sys/providers/<system>/config_mapping.rs`
- `edgion-gateway/src/link_sys/providers/<system>/ops.rs`

Where:

- `client.rs` manages the lifecycle and wraps the underlying client.
- `config_mapping.rs` converts CRD configuration into library configuration.
- `ops.rs` exposes the higher-level operations Edgion actually needs.

`Elasticsearch` additionally has:

- `bulk.rs`
- `data_sender.rs`

because it now also serves `DataSender<String>` scenarios such as logs.

### 2. Webhook

Webhook is a compiled outbound-request provider. The resource owns both the backend target and the
allow-listed request template; consumers supply the inbound session plus an optional path override.
Do not reintroduce the removed consumer-owned `WebhookCallRequest`, `forwardHeaders`, or
`pathSuffix` model.

Module layout:

- `providers/webhook/manager.rs`: global registry and upsert/remove lifecycle.
- `providers/webhook/compiled_request.rs`: compile resource-owned method/path/headers/args/cookies.
- `providers/webhook/runtime.rs`: compiled request, resolved target, client, and health state.
- `providers/webhook/health.rs`: active + passive health checks.
- `providers/webhook/call.rs`: bounded provider call implementation.
- `providers/webhook/resolver.rs`: `KeyGet::Webhook` consumer adapter.
- `providers/webhook/client.rs`: HTTP client construction and TLS material.

Key points:

- The key remains `namespace/name`.
- `WebhookManager.upsert()` compiles the request, resolves credentials/target, and replaces runtime state.
- The target is `backendRef`-only; request shape and presence policy are resource-owned.
- Consumers must use the resolver/call APIs instead of building arbitrary outbound requests.
- Full architecture: [09-link-sys.md](09-link-sys.md#webhook) and
  [03-controller-gateway-link/webhook.md](../03-controller-gateway-link/webhook.md).

### 3. Kafka and HttpDns

Kafka is fully wired through its provider, `KAFKA_RUNTIME`, full/partial LinkSys dispatch, and
`get_kafka_client`. HttpDns is a `SystemConfig` variant consumed by the DNS resolver path; it is
intentionally not inserted into a generic LinkSys runtime-client store.

## The real ConfHandler bridging pattern

The bridging for LinkSys is now fixed:

1. `ConfigClient::new()` creates `ClientCache<LinkSys>`.
2. It calls `create_link_sys_handler()` to register the conf processor.
3. `create_link_sys_handler()` returns the global `LinkSysStore`.
4. `ConfHandler<LinkSys>::full_set()` / `partial_update()` first runs `validate_config()`.
5. Then calls `LinkSysStore.replace_all()` / `update()`.
6. `dispatch_full_set()` / `dispatch_partial_update()` in `runtime/store.rs` async-dispatches to
   Redis / Etcd / Elasticsearch / Kafka / Webhook runtimes and deliberately skips HttpDns runtime-client creation.

So if you are changing gateway-side runtime behavior, focus on:

- [conf_handler.rs](../../../edgion-gateway/src/link_sys/runtime/conf_handler.rs)
- [store.rs](../../../edgion-gateway/src/link_sys/runtime/store.rs)

Don't build a parallel watcher of your own.

## What you typically modify when adding a new LinkSys system

If you are adding a new system variant within the existing `LinkSys` resource, you typically touch at least:

1. `edgion-resources/src/resources/link_sys/<system>.rs`
2. `edgion-resources/src/resources/link_sys/mod.rs`
3. `config/crd/edgion-crd/link_sys_crd.yaml`
4. `edgion-gateway/src/link_sys/providers/<system>/`
5. `edgion-gateway/src/link_sys/runtime/store.rs`
6. If needed, `edgion-gateway/src/api/mod.rs`
7. `edgion-tests/integration/conf/LinkSys/<System>/`
8. `edgion-tests/integration/conf/Services/<system>/docker-compose.yaml`
9. `edgion-tests/integration/scripts/integration/link_sys/run_<system>_test.sh`

Typically you do not need to add a new resource kind, nor do you need to modify the controller processor framework itself.

## Recommended development order

### 1. Start with types and schema

First define or extend:

- The `SystemConfig` variant.
- The `<System>ClientConfig`.
- The `validate_config()` branch.
- The CRD schema.

If the schema is not in place, do not start writing human docs or test YAML.

### 2. Then add the provider runtime

Split the provider directory into three clear layers:

- config mapping
- runtime client
- high-level ops

Don't shove CRD types, library configuration translation, and business operations into a single file.

### 3. Then wire into `runtime/store.rs`

This is the easiest place to forget. At minimum, add:

- The `dispatch_full_set()` branch.
- The `dispatch_partial_update()` branch.
- The typed runtime store's insert / remove / replace-all logic.
- The cleanup logic for old clients / old tasks on deletion.

If the system supports hot reload, prefer the existing pattern of "swap in the new instance first, then init or shut down the old one in the background".

### 4. Add admin testing endpoints only when tests need them

If integration tests need to validate client behavior directly, add a testing-router endpoint in:

- `edgion-gateway/src/api/mod.rs`

The repo currently has these testing endpoints:

- Redis
- Etcd
- Elasticsearch

They are mounted onto the gateway admin API only under `--integration-testing-mode`.

### 5. Finally, add tests and compose

Existing scripts and drivers:

- `edgion-tests/integration/scripts/integration/run_integration.sh -r LinkSys -i Redis` (Redis is integrated into the main runner; the assertions live in `link_sys/redis_assertions.sh` and Redis docker is auto-started via the runner's `start_redis_service_if_needed` precondition)
- `edgion-tests/integration/scripts/integration/link_sys/run_etcd_test.sh` (standalone)
- `edgion-tests/integration/scripts/integration/link_sys/run_es_test.sh` (standalone)

Configurations and dependencies are also layered:

- `edgion-tests/integration/conf/LinkSys/<System>/`
- `edgion-tests/integration/conf/Services/<system>/docker-compose.yaml`

When adding a new system, follow this layout instead of inventing your own test-script naming.

## When NOT to use LinkSys

Do not force the following scenarios into `LinkSys`:

- A purely in-process tool-style sender with no CRD configuration need.
- No need to update dynamically by `namespace/name`.
- No need for controller -> gateway sync.
- A purely internal capability such as local log persistence.

These resemble the `local_file` provider rather than a new `SystemConfig`.

## Common pitfalls

- Adding only a `SystemConfig` variant and forgetting `validate_config()` or the CRD.
- The provider is finished but `runtime/store.rs` does not dispatch, so the YAML is synced but the runtime does not take effect.
- Only handling full sync, forgetting the partial-update / remove paths.
- Failing to abort the old health task on Webhook update.
- Treating LinkSys as "a new resource kind" and modifying controller framework code that should not be touched.
- Writing the test compose and YAML but forgetting the gateway admin testing endpoint, making runtime-behavior assertions hard.

## Validation

Run the smallest static check first:

```bash
cargo check -p edgion-gateway --all-targets
cargo check --workspace --all-targets
```

If you modified an existing system, run the corresponding entry point first:

```bash
# Redis: now part of the main runner
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r LinkSys -i Redis

# Etcd / Elasticsearch: standalone
./edgion-tests/integration/scripts/integration/link_sys/run_etcd_test.sh
./edgion-tests/integration/scripts/integration/link_sys/run_es_test.sh
```

If you modified docs / skill / AI entry points, also run:

```bash
python3 cicd/checks/validate_agent_docs.py
```

## Related

- [03-link-sys-testing.md](../../05-testing/03-link-sys-testing.md)
- [12-edgion-plugin-dev.md](12-edgion-plugin-dev.md)
