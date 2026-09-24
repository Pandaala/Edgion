/// Query GatewayClass by name (cluster-scoped: key == name).
/// Test-only: production reads go through `ConfigClient::get_gateway_class`
/// against the conf_sync ClientCache, not this store.
#[cfg(test)]
pub fn get_gateway_class_by_name(name: &str) -> Option<GatewayClass> {
    GATEWAY_CLASS_STORE.read().get(name).cloned()
}
