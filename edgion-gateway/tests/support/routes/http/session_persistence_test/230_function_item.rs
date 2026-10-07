/// Existing tuple fixtures are bound to a real Route before the production
/// borrowed-candidate lookup; the production path has no raw tuple adapter.
fn sp_lookup_rule_scoped<'a>(
    backends: impl Iterator<Item = (&'a str, Option<&'a str>, Option<i32>)>,
    route_ns: &str,
    token: &str,
) -> Option<(usize, std::net::SocketAddr)> {
    let refs = backends
        .map(|(name, namespace, port)| edgion_resources::HTTPBackendRef {
            name: name.to_owned(),
            namespace: namespace.map(str::to_owned),
            port,
            weight: None,
            group: None,
            kind: None,
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        })
        .collect::<Vec<_>>();
    let mut rule: edgion_resources::HTTPRouteRule = serde_json::from_value(serde_json::json!({})).unwrap();
    rule.backend_refs = Some(refs);
    let compiled = crate::routes::http::match_unit::compile_http_fixture(&rule, route_ns).unwrap();
    sp_lookup_typed_candidates(
        compiled.backends.iter().filter_map(|backend| {
            let service = backend.permitted_service()?;
            Some(StickyCandidate {
                slot: backend.slot(),
                service_key: service.key(),
                section: service.section(),
                scope: None,
            })
        }),
        token,
        |candidate, token| {
            crate::lb::runtime_state::sp_affinity_get_or_build_scoped(
                candidate.service_key,
                candidate.section,
                None,
                || crate::backends::selector::backends_for_service_section(candidate.service_key, candidate.section),
            )
            .get(token)
            .copied()
        },
    )
    .map(|(slot, addr, _)| (slot, addr))
}

fn sp_lookup_rule_scoped_indexed<'a>(
    backends: impl Iterator<Item = (usize, &'a str, Option<&'a str>, Option<i32>)>,
    route_ns: &str,
    token: &str,
) -> Option<(usize, std::net::SocketAddr)> {
    let mut refs = Vec::new();
    for (slot, name, namespace, port) in backends {
        while refs.len() < slot {
            refs.push(
                serde_json::from_value(
                    serde_json::json!({"name":"unsupported", "group":"example.io", "kind":"Service"}),
                )
                .unwrap(),
            );
        }
        refs.push(edgion_resources::HTTPBackendRef {
            name: name.to_owned(),
            namespace: namespace.map(str::to_owned),
            port,
            weight: None,
            group: None,
            kind: None,
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        });
    }
    let mut rule: edgion_resources::HTTPRouteRule = serde_json::from_value(serde_json::json!({})).unwrap();
    rule.backend_refs = Some(refs);
    let compiled = crate::routes::http::match_unit::compile_http_fixture(&rule, route_ns).unwrap();
    sp_lookup_typed_candidates(
        compiled.backends.iter().filter_map(|backend| {
            let service = backend.permitted_service()?;
            Some(StickyCandidate {
                slot: backend.slot(),
                service_key: service.key(),
                section: service.section(),
                scope: None,
            })
        }),
        token,
        |candidate, token| {
            crate::lb::runtime_state::sp_affinity_get_or_build_scoped(
                candidate.service_key,
                candidate.section,
                None,
                || crate::backends::selector::backends_for_service_section(candidate.service_key, candidate.section),
            )
            .get(token)
            .copied()
        },
    )
    .map(|(slot, addr, _)| (slot, addr))
}
