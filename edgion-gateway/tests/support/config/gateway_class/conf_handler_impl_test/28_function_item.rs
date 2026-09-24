/// List all GatewayClass resources.
/// Test-only: production reads go through `ConfigClient::list_gateway_classes`
/// against the conf_sync ClientCache, not this store.
#[cfg(test)]
pub fn list_gateway_classes() -> Vec<GatewayClass> {
    let mut items: Vec<GatewayClass> = GATEWAY_CLASS_STORE.read().values().cloned().collect();
    items.sort_by_key(|a| a.name_any());
    items
}
