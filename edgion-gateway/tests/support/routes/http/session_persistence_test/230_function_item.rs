/// Pure rule-scoped lookup: walk the backendRef `(name, namespace)` pairs in
/// declaration order and return the first whose Service affinity map contains
/// `token`, together with the resolved endpoint address.
///
/// Declaration order makes the walk deterministic across gateway replicas. If
/// the same endpoint address is registered under two backendRefs' Services
/// (two Services selecting the same pods on the same targetPort), the first
/// declared backendRef wins — the same ambiguity a merged Envoy cluster has.
#[cfg(test)]
fn sp_lookup_rule_scoped<'a>(
    backends: impl Iterator<Item = (&'a str, Option<&'a str>, Option<i32>)>,
    route_ns: &str,
    token: &str,
) -> Option<(usize, std::net::SocketAddr)> {
    sp_lookup_rule_scoped_indexed(
        backends
            .enumerate()
            .map(|(index, (name, namespace, port))| (index, name, namespace, port)),
        route_ns,
        token,
    )
}
