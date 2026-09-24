/// Unit-test ownership view; production binds owner/policy through select_tls_route.
#[cfg(test)]
fn select_route_unit(manager: &TlsRouteManager, sni: &str) -> Option<Arc<TlsRouteUnit>> {
    manager.load_route_table().match_route(sni)
}
