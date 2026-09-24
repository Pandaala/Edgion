/// Resolve the gateway->backend connection-ESTABLISHMENT deadline, independent
/// of the request/read/write timeouts. Precedence: per-backend
/// `EdgionBackendTrafficPolicy.spec.connection.connectTimeout` (looked up via
/// `service_key`) else the global `defaultConnectTimeout`. `service_key` is
/// `None` only for paths with no resolvable Service (e.g. an ExternalJump),
/// which always fall back to the global default.
///
/// Returns `None` when the resolved deadline is zero. A global
/// `defaultConnectTimeout: "0s"` parses to `Duration::ZERO`, which Pingora would
/// treat as an instant connect deadline (every connect fails). Matching the
/// sibling peer-timeout guards (`effective_backend_bound`, `request_remaining`,
/// `grpc_remaining` all filter out zero), a zero deadline means "no connect
/// bound" — the caller leaves `connection_timeout` unset — not an instant
/// failure. The BTP override can never be zero (struct validation rejects it),
/// so this only guards a degenerate global config.
#[inline]
#[cfg(test)]
fn resolve_connect_deadline(service_key: Option<&str>, global: std::time::Duration) -> Option<std::time::Duration> {
    let d = service_key
        .and_then(|k| get_backend_traffic_policy_store().get_connect_timeout(k))
        .unwrap_or(global);
    (!d.is_zero()).then_some(d)
}
