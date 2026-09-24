/// Add one resource's hosts to a listener bucket using the same canonical
/// ASCII case as SNI lookup. Duplicate case variants within one resource are
/// collapsed so they cannot create a false self-collision.
#[cfg(test)]
fn add_canonical_hosts(
    host_map: &mut HashMap<String, Vec<Arc<CompiledEdgionTls>>>,
    tls: &Arc<CompiledEdgionTls>,
) -> usize {
    let mut added = 0;
    for host in &tls.tls.spec.hosts {
        let tls_list = host_map.entry(host.to_ascii_lowercase()).or_default();
        if tls_list.last().is_none_or(|existing| !Arc::ptr_eq(existing, tls)) {
            tls_list.push(tls.clone());
            added += 1;
        }
    }
    added
}
