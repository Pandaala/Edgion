/// List all EdgionGatewayConfig resources.
/// Test-only: production reads go through `ConfigClient::list_edgion_gateway_configs`
/// against the conf_sync ClientCache, not this store.
#[cfg(test)]
pub fn list_edgion_gateway_configs() -> Vec<EdgionGatewayConfig> {
    let mut items: Vec<EdgionGatewayConfig> = EDGION_GATEWAY_CONFIG_STORE.load().configs.values().map(|config| (**config).clone()).collect();
    items.sort_by_key(|a| a.name_any());
    items
}
