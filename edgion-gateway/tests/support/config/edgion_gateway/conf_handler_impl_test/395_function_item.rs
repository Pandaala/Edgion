// Name-based accessors are confined to publication tests. Runtime consumers
// bind once and use GatewayConfigHandle::load.
#[cfg(test)]
fn get_resolved_gateway_config(config_name: Option<&str>) -> Arc<ResolvedGatewayConfig> {
    bind_gateway_config(config_name).load()
}
