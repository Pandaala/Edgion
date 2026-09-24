/// Query EdgionGatewayConfig by name (cluster-scoped: key == name).
/// Test-only: production reads go through `ConfigClient::get_edgion_gateway_config`
/// against the conf_sync ClientCache, not this store.
#[cfg(test)]
pub fn get_edgion_gateway_config_by_name(name: &str) -> Option<EdgionGatewayConfig> {
    EDGION_GATEWAY_CONFIG_STORE.load().configs.get(name).cloned()
}
