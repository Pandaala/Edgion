# Control-plane logging and Tracing rules

> The control plane uses the `tracing` crate for structured logging. This document covers structured-log conventions, level selection,
> error context, instrument naming, graceful degradation, and other best practices.
>
> For hot-path constraints and sensitive-information rules see [../01-log-safety.md](../01-log-safety.md).

## Information layering

The three-layer model (access log / `tracing` system events / Prometheus metrics) and the
"they complement each other" principle are defined once in [../SKILL.md](../SKILL.md#information-layering).
This document covers the **System events** layer (`tracing::*`) in detail.

---

## Structured-log conventions

Use key-value structured fields so the log system can index them:

```rust
// Correct: structured fields
tracing::warn!(
    component = "grpc_client",
    error = %e,
    backoff_secs = backoff.as_secs(),
    "WatchServerMeta connection failed, retrying"
);

// Wrong: spliced string, structure lost
tracing::warn!("WatchServerMeta connection failed: {}, retry in {}s", e, backoff.as_secs());
```

**Always use a `component` field to identify the module**, so logs can be filtered by component:

```rust
tracing::info!(component = "grpc_server", ...);
tracing::info!(component = "k8s_status", ...);
tracing::info!(component = "conf_client", ...);
```

**Span inheritance rule**: if `component` is already declared in an outer span, logs inside the span inherit it automatically and do not need to repeat it. For example, the span of `process_resource` carries `component = "resource_processor"`, so all logs inside it do not need `component`, `kind`, `name`, `ns`, `rv`, or `sv` repeated.

---

## Level selection

| Level | When to use |
|-------|---------|
| `error!` | Errors affecting requests/data (require immediate attention) |
| `warn!` | Recoverable exceptions, retries, degradations (need attention but not urgent) |
| `info!` | Important lifecycle events (startup, shutdown, reload, connection established/dropped) |
| `debug!` | Intermediate state useful while troubleshooting (not needed during normal operation) |
| `trace!` | Extremely detailed flow tracing (for performance analysis) |

---

## Error messages must carry locating value

```rust
// Valuable: contains what/where/why
tracing::error!(
    component = "k8s_status",
    kind = "HTTPRoute",
    namespace = namespace,
    name = name,
    error = %e,
    "Failed to update K8s status"
);

// Worthless: cannot tell what happened
tracing::error!("error: {}", e);
```

---

## Graceful degradation

In observability-related code, a data-collection failure must not cause request failure.
On the request hot path, use the access-log object's bounded fallback rather than adding
a tracing event:

```rust
// AccessLogEntry::to_json returns bounded fallback JSON containing
// AccessLogSerializeFailed; it does not trace from the hot path.
let json = entry.to_json();

// panic interrupts the request
let json = serde_json::to_string(&entry).unwrap();
```

---

## instrument and span naming

For long-running logic involving async tasks, make good use of `instrument` to name the span:

```rust
use tracing::instrument;

#[instrument(name = "watch_server_meta", skip(self), fields(client_id = %self.client_id))]
pub async fn start_watch_server_meta(self: Arc<Self>) {
    // ...
}
```

### Processor span pattern

`process_resource` / `process_delete` use `info_span` + `.instrument()` to wrap the whole processing flow (async-compatible; `.entered()` cannot be used):

```rust
let span = tracing::info_span!("process_resource", kind = self.kind, name = %name, ns = %namespace, rv, sv);
self.process_resource_inner(obj, ctx, is_init, existing_status_json, sv)
    .instrument(span)
    .await
```

All logs inside the span inherit `kind/name/ns/rv/sv` automatically — **do not repeat those fields manually**; just write the message and extra fields (such as `error`, `reason`, `phase`).

---

## Related rules

- **[../00-logging-and-tracing-ids.md](../00-logging-and-tracing-ids.md)** — rv / sv propagation mechanism and troubleshooting flow
- **[../01-log-safety.md](../01-log-safety.md)** — Iron rules for sensitive-information protection and data-plane tracing prohibition
