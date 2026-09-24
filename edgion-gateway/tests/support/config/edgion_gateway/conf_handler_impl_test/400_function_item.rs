#[cfg(test)]
fn get_resolved_l4_timeouts(config_name: Option<&str>) -> ResolvedL4Timeouts {
    get_resolved_gateway_config(config_name).l4_timeouts
}
