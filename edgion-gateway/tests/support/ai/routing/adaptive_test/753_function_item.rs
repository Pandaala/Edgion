#[cfg(test)]
fn collect_retained_keys(
    routes: &[Arc<HTTPRoute>],
    mut plugin_uses_adaptive_selection: impl FnMut(&str) -> bool,
    mut logical_models: impl FnMut(&PolicyTargetKey) -> Option<Vec<String>>,
) -> HashSet<AdaptiveTelemetryKey> {
    let mut index = AdaptiveContributionIndex::default();
    for (position, route) in routes.iter().enumerate() {
        let route_key = position.to_string();
        index.attach_route(route_key.clone(), RouteAdaptiveFacts::from_route(route));
        index.recompute_route(&route_key, &mut plugin_uses_adaptive_selection, &mut logical_models);
    }
    index.retained_keys()
}
