use crate::ctx::EdgionHttpContext;
use crate::routes::grpc::GrpcRouteRuleUnit;
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use crate::runtime::GatewayInfo;
use edgion_algo::matcher::radix_hostname::{
    RadixHostnameBuildError, RadixHostnameMatcher, RadixHostnamePattern, RadixHostnameQuery,
};
use edgion_resources::err::EdError;
use edgion_resources::resources::grpc_route::CompiledGrpcMethodMatch;
use pingora_proxy::Session;
use std::collections::HashMap;
use std::sync::Arc;

/// One immutable ranked route array, shared by all hostname indexes.
/// Hostname specificity is resolved by ordered hostname lookup; every remaining
/// precedence input is ranked at build time. Index entries are numeric ranks,
/// so hostname fan-out never duplicates rules, plugins, or backend selectors.
pub struct GrpcMatchEngine {
    routes: Vec<Arc<GrpcRouteRuleUnit>>,
    hosts: RadixHostnameMatcher<GrpcCandidateIndex>,
    catch_all: GrpcCandidateIndex,
}

#[derive(Default)]
struct GrpcServiceCandidates {
    methods: HashMap<String, Vec<usize>>,
    all_methods: Vec<usize>,
}

/// Indexes select candidate subsets, never winner priority. Each list contains
/// ascending ranks into GrpcMatchEngine::routes.
#[derive(Default)]
struct GrpcCandidateIndex {
    services: HashMap<String, GrpcServiceCandidates>,
    methods: HashMap<String, Vec<usize>>,
    regex: Vec<usize>,
    any: Vec<usize>,
}

impl GrpcCandidateIndex {
    fn insert(&mut self, rank: usize, route: &GrpcRouteRuleUnit) {
        match route.matched_info.compiled_method_match.as_ref() {
            Some(CompiledGrpcMethodMatch::Any) => self.any.push(rank),
            Some(CompiledGrpcMethodMatch::RegularExpression { .. }) => self.regex.push(rank),
            Some(CompiledGrpcMethodMatch::Exact) => {
                let Some(matcher) = route.matched_info.matched.method.as_ref() else {
                    return;
                };
                match (matcher.service.as_deref(), matcher.method.as_deref()) {
                    (Some(service), Some(method)) => self
                        .services
                        .entry(service.to_owned())
                        .or_default()
                        .methods
                        .entry(method.to_owned())
                        .or_default()
                        .push(rank),
                    (Some(service), None) => self
                        .services
                        .entry(service.to_owned())
                        .or_default()
                        .all_methods
                        .push(rank),
                    (None, Some(method)) => self.methods.entry(method.to_owned()).or_default().push(rank),
                    (None, None) => {}
                }
            }
            None => {}
        }
    }

    fn candidates<'a>(&'a self, service: &str, method: &str) -> GrpcCandidates<'a> {
        let service_routes = self.services.get(service);
        GrpcCandidates {
            streams: [
                service_routes
                    .and_then(|s| s.methods.get(method))
                    .map_or(&[], Vec::as_slice),
                service_routes.map_or(&[], |s| s.all_methods.as_slice()),
                self.methods.get(method).map_or(&[], Vec::as_slice),
                self.regex.as_slice(),
                self.any.as_slice(),
            ],
        }
    }
}

/// Merge a fixed number of sorted streams without allocating or sorting on the
/// request path. Empty streams require no work beyond checking their head.
struct GrpcCandidates<'a> {
    streams: [&'a [usize]; 5],
}

impl Iterator for GrpcCandidates<'_> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        let (stream, rank) = self
            .streams
            .iter()
            .enumerate()
            .filter_map(|(index, ranks)| ranks.first().map(|&rank| (index, rank)))
            .min_by_key(|&(_, rank)| rank)?;
        self.streams[stream] = &self.streams[stream][1..];
        Some(rank)
    }
}

impl GrpcMatchEngine {
    #[cfg(test)]
    pub fn new(routes: Vec<Arc<GrpcRouteRuleUnit>>) -> Self {
        Self::rebuild(routes, None).expect("unit-test hostname topology fits the flat representation")
    }

    /// Rebuild ranks and every dependent candidate list together. Only the
    /// hostname topology can be shared with an earlier ranked generation.
    pub(crate) fn rebuild(
        routes: Vec<Arc<GrpcRouteRuleUnit>>,
        previous: Option<&Self>,
    ) -> Result<Self, RadixHostnameBuildError> {
        // Character counts are computed once, not inside the sort comparator.
        // Match type is deliberately absent: no Exact/regex bucket can override
        // the Gateway API service/method/header/route precedence chain.
        let mut ranked: Vec<_> = routes
            .into_iter()
            .filter(|route| route.matched_info.compiled_method_match.is_some())
            .filter(|unit| {
                unit.route
                    .as_ref()
                    .is_none_or(|route| route.spec.validate_hostnames().is_ok())
            })
            .map(|route| {
                let matcher = route.matched_info.matched.method.as_ref();
                let service_len = matcher
                    .and_then(|m| m.service.as_deref())
                    .map_or(0, |s| s.chars().count());
                let method_len = matcher
                    .and_then(|m| m.method.as_deref())
                    .map_or(0, |s| s.chars().count());
                (service_len, method_len, route)
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.specificity.cmp_precedence(&b.2.specificity))
        });
        let routes: Vec<_> = ranked.into_iter().map(|(_, _, route)| route).collect();
        let mut groups: HashMap<RadixHostnamePattern, GrpcCandidateIndex> = HashMap::new();
        let mut catch_all = GrpcCandidateIndex::default();
        for (rank, route) in routes.iter().enumerate() {
            // Listener intersection may narrow a wildcard Route to an exact
            // hostname, but must not promote its precedence over an exact Route.
            // Borrow the authored names; deep_match still enforces the resolved
            // intersection and listener attachment without another owned copy.
            let hostnames = route
                .route
                .as_ref()
                .and_then(|route| route.spec.hostnames.as_deref())
                .unwrap_or(&[]);
            if hostnames.is_empty() {
                catch_all.insert(rank, route);
            }
            for hostname in hostnames {
                let pattern =
                    RadixHostnamePattern::parse(hostname).expect("ranked routes passed whole-list hostname validation");
                groups.entry(pattern).or_default().insert(rank, route);
            }
        }
        let groups: HashMap<RadixHostnamePattern, Arc<GrpcCandidateIndex>> = groups
            .into_iter()
            .map(|(pattern, group)| (pattern, Arc::new(group)))
            .collect();
        let hosts = match previous {
            Some(previous) => previous.hosts.with_groups(groups),
            None => RadixHostnameMatcher::build(groups),
        }?;
        Ok(Self {
            routes,
            hosts,
            catch_all,
        })
    }

    pub fn match_route(
        &self,
        session: &Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
        hostname: &str,
    ) -> Result<(Arc<GrpcRouteRuleUnit>, Arc<GatewayInfo>), EdError> {
        let (service, method) = parse_grpc_path(ctx.path())?;
        self.select(service, method, hostname, |route| {
            route.deep_match(session, index, hostname)
        })?
        .map(|(route, gateway)| (Arc::clone(route), gateway))
        .ok_or_else(EdError::RouteNotFound)
    }

    /// Shared selection loop. Only a predicate no-match advances to the next
    /// candidate; errors retain their identity for the protocol dispatch owner.
    /// The callback is monomorphized; it also lets pure selection tests exercise
    /// the exact production traversal without inventing a mock session.
    fn select<T>(
        &self,
        service: &str,
        method: &str,
        hostname: &str,
        mut deep_match: impl FnMut(&GrpcRouteRuleUnit) -> Result<Option<T>, EdError>,
    ) -> Result<Option<(&Arc<GrpcRouteRuleUnit>, T)>, EdError> {
        // A non-DNS authority cannot hit a named group. Existing HTTP authority
        // validation and the separately authored hostless group remain in charge.
        for group in RadixHostnameQuery::new(hostname)
            .ok()
            .into_iter()
            .flat_map(|query| self.hosts.matching_values(query))
            .chain(std::iter::once(&self.catch_all))
        {
            for rank in group.candidates(service, method) {
                let route = &self.routes[rank];
                if let Some(CompiledGrpcMethodMatch::RegularExpression {
                    service: service_regex,
                    method: method_regex,
                }) = route.matched_info.compiled_method_match.as_ref()
                {
                    if service_regex.as_ref().is_some_and(|re| !re.is_match(service))
                        || method_regex.as_ref().is_some_and(|re| !re.is_match(method))
                    {
                        continue;
                    }
                }
                if let Some(result) = deep_match(route)? {
                    return Ok(Some((route, result)));
                }
            }
        }
        Ok(None)
    }

    #[cfg(test)]
    pub(crate) fn shares_hosts(&self, other: &Self) -> bool {
        self.hosts.shares_hosts(&other.hosts)
    }
}

/// Parse gRPC path: /{service}/{method}
/// Example: "/helloworld.Greeter/SayHello" → ("helloworld.Greeter", "SayHello")
/// Returns string slices into the original path to avoid heap allocation.
pub fn parse_grpc_path(path: &str) -> Result<(&str, &str), EdError> {
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    if let Some((service, method)) = trimmed.split_once('/') {
        if !service.is_empty() && !method.is_empty() && !method.contains('/') {
            return Ok((service, method));
        }
    }
    Err(EdError::InvalidGrpcPath(path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::grpc::match_unit::GrpcMatchInfo;
    use edgion_resources::resources::grpc_route::GRPCRouteRule;
    use edgion_resources::{GRPCHeaderMatch, GRPCMethodMatch, GRPCMethodMatchType, GRPCRouteMatch};

    #[test]
    fn test_parse_grpc_path_valid() {
        let result = parse_grpc_path("/helloworld.Greeter/SayHello");
        assert!(result.is_ok());
        let (service, method) = result.unwrap();
        assert_eq!(service, "helloworld.Greeter");
        assert_eq!(method, "SayHello");
    }

    #[test]
    fn test_parse_grpc_path_invalid() {
        assert!(parse_grpc_path("/invalid").is_err());
        assert!(parse_grpc_path("/").is_err());
        assert!(parse_grpc_path("").is_err());
        assert!(parse_grpc_path("/a/b/c").is_err());
    }

    #[test]
    fn test_parse_grpc_path_empty_parts() {
        assert!(parse_grpc_path("//").is_err());
        assert!(parse_grpc_path("/service/").is_err());
        assert!(parse_grpc_path("//method").is_err());
    }

    fn make_route(
        service: Option<&str>,
        method: Option<&str>,
        match_type: Option<GRPCMethodMatchType>,
        name: &str,
    ) -> Arc<GrpcRouteRuleUnit> {
        use crate::lb::BackendSelector;
        use crate::plugins::PluginRuntime;
        use crate::routes::grpc::match_unit::GrpcMatchSpecificity;
        use crate::routes::grpc::GrpcRouteInfo;

        let grpc_match = GRPCRouteMatch {
            method: Some(GRPCMethodMatch {
                match_type,
                service: service.map(|s| s.to_string()),
                method: method.map(|m| m.to_string()),
            }),
            headers: None,
        };
        let route_info = Arc::new(GrpcRouteInfo {
            parent_refs: None,
            effective_hostnames: vec!["*".to_string()],
        });
        let rule = Arc::new(GRPCRouteRule {
            name: None,
            matches: None,
            filters: None,
            backend_refs: None,
            timeouts: None,
            retry: None,
            session_persistence: None,
            parsed_timeouts: None,
            parsed_retry: None,
            parsed_allow_non_idempotent_retry: None,
        });
        Arc::new(GrpcRouteRuleUnit {
            resource_key: format!("default/{}", name),
            matched_info: GrpcMatchInfo::new("default".to_string(), name.to_string(), 0, 0, grpc_match, 0, None),
            rule,
            route_info,
            route: None,
            specificity: GrpcMatchSpecificity {
                header_count: 0,
                creation_ts_ms: 0,
                route_key: format!("default/{}", name),
                rule_index: 0,
                match_index: 0,
            },
            metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
            plugin_runtime: Arc::new(PluginRuntime::new()),
            backend_finder: Arc::new(BackendSelector::new()),
            backends: Arc::from(Vec::new()),
        })
    }

    #[test]
    fn test_multiple_routes_same_service_method() {
        let engine = GrpcMatchEngine::new(vec![
            make_route(Some("test.Service"), Some("Method"), None, "route2"),
            make_route(Some("test.Service"), Some("Method"), None, "route1"),
        ]);
        assert_eq!(
            candidate_names(&engine, "test.Service", "Method", "example.com"),
            ["default/route1", "default/route2"]
        );
    }

    #[test]
    fn test_grpc_regex_classification_and_sort() {
        for (service, method) in [
            (Some("test.Service"), Some("Say.*")),
            (Some(r"test\..*"), Some("Method.*")),
            (Some(r"test\..*"), None),
            (None, Some("Method.*")),
            (Some(""), Some("Method.*")),
        ] {
            let route = make_route(service, method, Some(GRPCMethodMatchType::RegularExpression), "regex");
            let Some(CompiledGrpcMethodMatch::RegularExpression {
                service: sr,
                method: mr,
            }) = route.matched_info.compiled_method_match.as_ref()
            else {
                panic!("compiled regex expected");
            };
            assert_eq!(sr.is_some(), service.is_some_and(|s| !s.is_empty()));
            assert_eq!(mr.is_some(), method.is_some_and(|s| !s.is_empty()));
            let engine = GrpcMatchEngine::new(vec![route]);
            assert_eq!(engine.catch_all.regex, [0]);
            assert!(engine.catch_all.services.is_empty());
            assert!(engine.catch_all.methods.is_empty());
            assert!(engine.catch_all.any.is_empty());
        }
    }

    #[test]
    fn test_grpc_regex_sort_prefers_service_length_before_method_length() {
        let longer_service = make_route(
            Some("service.*"),
            Some("x"),
            Some(GRPCMethodMatchType::RegularExpression),
            "longer-service",
        );
        let longer_method = make_route(
            Some("s.*"),
            Some("an-extremely-long-method-pattern.*"),
            Some(GRPCMethodMatchType::RegularExpression),
            "longer-method",
        );

        let engine = GrpcMatchEngine::new(vec![longer_method, longer_service]);
        let order: Vec<_> = engine.routes.iter().map(|route| route.resource_key.as_str()).collect();
        assert_eq!(order, ["default/longer-service", "default/longer-method"]);
    }

    #[test]
    fn test_grpc_regex_sort_uses_method_length_when_service_lengths_are_equal() {
        let shorter_method = make_route(
            Some("service.*"),
            Some("x"),
            Some(GRPCMethodMatchType::RegularExpression),
            "shorter-method",
        );
        let longer_method = make_route(
            Some("service.*"),
            Some("method.*"),
            Some(GRPCMethodMatchType::RegularExpression),
            "longer-method",
        );

        let engine = GrpcMatchEngine::new(vec![shorter_method, longer_method]);
        let order: Vec<_> = engine.routes.iter().map(|route| route.resource_key.as_str()).collect();
        assert_eq!(order, ["default/longer-method", "default/shorter-method"]);
    }

    #[test]
    fn test_grpc_regex_sort_counts_unicode_characters_not_utf8_bytes() {
        // Both patterns match "svc". `€*svc` is five characters but seven bytes,
        // while `^svc.?` is six characters and six bytes.
        let longer_in_bytes = make_route(
            Some("€*svc"),
            Some("method"),
            Some(GRPCMethodMatchType::RegularExpression),
            "longer-in-bytes",
        );
        let longer_in_characters = make_route(
            Some("^svc.?"),
            Some("method"),
            Some(GRPCMethodMatchType::RegularExpression),
            "longer-in-characters",
        );

        let engine = GrpcMatchEngine::new(vec![longer_in_bytes, longer_in_characters]);
        let order: Vec<_> = engine.routes.iter().map(|route| route.resource_key.as_str()).collect();
        assert_eq!(order, ["default/longer-in-characters", "default/longer-in-bytes"]);
    }

    #[test]
    fn test_grpc_regex_sort_falls_back_to_specificity() {
        fn with_specificity(
            mut route: Arc<GrpcRouteRuleUnit>,
            header_count: u16,
            route_key: &str,
            rule_index: u32,
            match_index: u32,
        ) -> Arc<GrpcRouteRuleUnit> {
            let specificity = &mut Arc::get_mut(&mut route).unwrap().specificity;
            specificity.header_count = header_count;
            specificity.route_key = route_key.to_string();
            specificity.rule_index = rule_index;
            specificity.match_index = match_index;
            route
        }

        let later_match = with_specificity(
            make_route(
                Some("svc.*"),
                Some("method.*"),
                Some(GRPCMethodMatchType::RegularExpression),
                "later-match",
            ),
            1,
            "default/shared",
            0,
            1,
        );
        let later_route = with_specificity(
            make_route(
                Some("svc.*"),
                Some("method.*"),
                Some(GRPCMethodMatchType::RegularExpression),
                "later-route",
            ),
            1,
            "default/z-route",
            0,
            0,
        );
        let later_rule = with_specificity(
            make_route(
                Some("svc.*"),
                Some("method.*"),
                Some(GRPCMethodMatchType::RegularExpression),
                "later-rule",
            ),
            1,
            "default/shared",
            1,
            0,
        );
        let earlier_match = with_specificity(
            make_route(
                Some("svc.*"),
                Some("method.*"),
                Some(GRPCMethodMatchType::RegularExpression),
                "earlier-match",
            ),
            1,
            "default/shared",
            0,
            0,
        );
        let more_headers = with_specificity(
            make_route(
                Some("svc.*"),
                Some("method.*"),
                Some(GRPCMethodMatchType::RegularExpression),
                "more-headers",
            ),
            2,
            "default/zz-route",
            9,
            9,
        );

        let engine = GrpcMatchEngine::new(vec![later_match, later_route, later_rule, earlier_match, more_headers]);
        let order: Vec<_> = engine.routes.iter().map(|route| route.resource_key.as_str()).collect();
        assert_eq!(
            order,
            [
                "default/more-headers",
                "default/earlier-match",
                "default/later-match",
                "default/later-rule",
                "default/later-route",
            ]
        );
    }

    #[test]
    fn test_grpc_regex_both_compilation_failed_skipped() {
        // F-020: failed regexes never enter an Exact or catch-all index.
        for (service, method) in [
            (Some("["), Some("[")),
            (Some("["), Some("Call")),
            (Some("svc"), Some("[")),
            (None, Some("[")),
        ] {
            let route = make_route(service, method, Some(GRPCMethodMatchType::RegularExpression), "invalid");
            assert!(route.matched_info.compiled_method_match.is_none());
            let engine = GrpcMatchEngine::new(vec![route]);
            assert!(engine.routes.is_empty());
            assert!(candidate_names(&engine, "svc", "Call", "example.com").is_empty());
        }
    }

    fn make_header_only_route(
        headers: Vec<(&str, &str)>,
        name: &str,
        rule_index: u32,
        match_index: u32,
    ) -> Arc<GrpcRouteRuleUnit> {
        use crate::lb::BackendSelector;
        use crate::plugins::PluginRuntime;
        use crate::routes::grpc::match_unit::GrpcMatchSpecificity;
        use crate::routes::grpc::GrpcRouteInfo;

        let grpc_match = GRPCRouteMatch {
            method: None,
            headers: if headers.is_empty() {
                None
            } else {
                Some(
                    headers
                        .iter()
                        .map(|(n, v)| GRPCHeaderMatch {
                            match_type: None,
                            name: n.to_string(),
                            value: v.to_string(),
                        })
                        .collect(),
                )
            },
        };
        let route_info = Arc::new(GrpcRouteInfo {
            parent_refs: None,
            effective_hostnames: vec!["*".to_string()],
        });
        let rule = Arc::new(GRPCRouteRule {
            name: None,
            matches: None,
            filters: None,
            backend_refs: None,
            timeouts: None,
            retry: None,
            session_persistence: None,
            parsed_timeouts: None,
            parsed_retry: None,
            parsed_allow_non_idempotent_retry: None,
        });
        Arc::new(GrpcRouteRuleUnit {
            resource_key: format!("default/{}", name),
            matched_info: GrpcMatchInfo::new("default".to_string(), name.to_string(), 0, 0, grpc_match, 0, None),
            rule,
            route_info,
            route: None,
            specificity: GrpcMatchSpecificity {
                header_count: headers.len() as u16,
                creation_ts_ms: 0,
                route_key: format!("default/{}", name),
                rule_index,
                match_index,
            },
            metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
            plugin_runtime: Arc::new(PluginRuntime::new()),
            backend_finder: Arc::new(BackendSelector::new()),
            backends: Arc::from(Vec::new()),
        })
    }

    #[test]
    fn test_header_only_routes_classified_as_catch_all() {
        let route1 = make_header_only_route(vec![("Version", "one")], "header-r1", 0, 0);
        let route2 = make_header_only_route(vec![("Version", "two")], "header-r2", 1, 0);
        let route3 = make_header_only_route(vec![("Version", "two"), ("Color", "orange")], "header-r3", 0, 1);

        let engine = GrpcMatchEngine::new(vec![route1, route2, route3]);
        assert_eq!(engine.catch_all.any.len(), 3);
        assert!(engine.catch_all.services.is_empty());
        assert!(engine.catch_all.methods.is_empty());
    }

    #[test]
    fn test_header_only_routes_sorted_by_specificity() {
        let route_1h = make_header_only_route(vec![("Version", "one")], "one-header", 0, 0);
        let route_2h = make_header_only_route(vec![("Version", "two"), ("Color", "orange")], "two-headers", 0, 1);

        let engine = GrpcMatchEngine::new(vec![route_1h, route_2h]);
        assert_eq!(engine.catch_all.any.len(), 2);
        assert_eq!(engine.routes[engine.catch_all.any[0]].specificity.header_count, 2);
        assert_eq!(engine.routes[engine.catch_all.any[1]].specificity.header_count, 1);
    }

    fn candidate_names(engine: &GrpcMatchEngine, service: &str, method: &str, hostname: &str) -> Vec<String> {
        let mut names = Vec::new();
        engine
            .select::<()>(service, method, hostname, |r| {
                names.push(r.resource_key.clone());
                Ok(None)
            })
            .unwrap();
        names
    }

    fn with_hosts(mut route: Arc<GrpcRouteRuleUnit>, hosts: &[&str]) -> Arc<GrpcRouteRuleUnit> {
        Arc::make_mut(&mut Arc::make_mut(&mut route).route_info).effective_hostnames =
            hosts.iter().map(|s| s.to_string()).collect();
        Arc::make_mut(&mut route).route = Some(Arc::new(
            serde_json::from_value(serde_json::json!({
                "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
                "metadata": {"name": "indexed", "namespace": "default"},
                "spec": {"hostnames": hosts.iter().filter(|host| **host != "*").collect::<Vec<_>>()}
            }))
            .unwrap(),
        ));
        route
    }

    #[test]
    fn indexed_selection_and_hostname_fallback_allocate_nothing() {
        let engine = GrpcMatchEngine::new(vec![
            with_hosts(
                make_route(Some("svc.User"), Some("Get"), None, "exact"),
                &["api.example.com"],
            ),
            with_hosts(make_route(None, Some("Get"), None, "fallback"), &["*.example.com"]),
        ]);
        for winner in ["default/exact", "default/fallback"] {
            let (selected, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
                engine.select("svc.User", "Get", "api.example.com", |route| {
                    Ok((route.resource_key == winner).then_some(()))
                })
            });
            assert_eq!(selected.unwrap().unwrap().0.resource_key, winner);
            assert_eq!(allocations.calls, 0);
            assert_eq!(allocations.requested_bytes, 0);
        }
        let regex = GrpcMatchEngine::new(vec![make_route(
            None,
            Some("^Get.*$"),
            Some(GRPCMethodMatchType::RegularExpression),
            "regex",
        )]);
        // Regex owns its existing lazy search cache. Measure repeated selection
        // after warming that cache, not a claim about first-use regex allocation.
        assert!(regex
            .select("svc.User", "GetItem", "api.example.com", |_| Ok(Some(())))
            .unwrap()
            .is_some());
        let (selected, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            regex.select("svc.User", "GetItem", "api.example.com", |_| Ok(Some(())))
        });
        assert!(selected.unwrap().is_some());
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn method_only_exact_matches_every_service_and_remains_below_service_constraints() {
        let engine = GrpcMatchEngine::new(vec![
            make_header_only_route(vec![], "fallback", 0, 0),
            make_route(None, Some("Get"), None, "method-only"),
            make_route(Some("svc.User"), None, None, "service-only"),
            make_route(Some("svc.User"), Some("Get"), None, "both"),
        ]);
        assert_eq!(
            candidate_names(&engine, "svc.User", "Get", "example.com"),
            [
                "default/both",
                "default/service-only",
                "default/method-only",
                "default/fallback"
            ]
        );
        assert_eq!(
            candidate_names(&engine, "other.Service", "Get", "example.com"),
            ["default/method-only", "default/fallback"]
        );
        assert_eq!(
            candidate_names(&engine, "other.Service", "Set", "example.com"),
            ["default/fallback"]
        );
    }

    #[test]
    fn exact_service_accepted_by_cel_remains_case_sensitive_at_runtime() {
        let engine = GrpcMatchEngine::new(vec![make_route(Some("pkg.Kervice"), None, None, "kelvin")]);
        assert_eq!(
            candidate_names(&engine, "pkg.Kervice", "Get", "example.com"),
            ["default/kelvin"]
        );
        assert!(candidate_names(&engine, "pkg.kervice", "Get", "example.com").is_empty());
    }

    #[test]
    fn regex_method_only_uses_pattern_length_and_checks_the_predicate() {
        let engine = GrpcMatchEngine::new(vec![
            make_route(None, Some("Get"), None, "exact"),
            make_route(
                None,
                Some("^Get.*$"),
                Some(GRPCMethodMatchType::RegularExpression),
                "regex",
            ),
            make_header_only_route(vec![], "fallback", 0, 0),
        ]);
        assert_eq!(
            candidate_names(&engine, "svc.User", "Get", "example.com"),
            ["default/regex", "default/exact", "default/fallback"]
        );
        assert_eq!(
            candidate_names(&engine, "other.Service", "GetAll", "example.com"),
            ["default/regex", "default/fallback"]
        );
        assert_eq!(
            candidate_names(&engine, "svc.User", "Set", "example.com"),
            ["default/fallback"]
        );
    }

    #[test]
    fn hostname_precedes_service_and_failed_candidates_continue_through_all_suffixes() {
        let engine = GrpcMatchEngine::new(vec![
            with_hosts(
                make_route(None, Some("Get"), None, "exact-host"),
                &["api.sub.example.com"],
            ),
            with_hosts(
                make_route(Some("svc.User"), Some("Get"), None, "wildcard"),
                &["*.example.com"],
            ),
            with_hosts(
                make_header_only_route(vec![], "specific-wildcard", 0, 0),
                &["*.sub.example.com"],
            ),
            make_header_only_route(vec![], "fallback", 0, 0),
        ]);
        assert_eq!(
            candidate_names(&engine, "svc.User", "Get", "api.sub.example.com"),
            [
                "default/exact-host",
                "default/specific-wildcard",
                "default/wildcard",
                "default/fallback"
            ]
        );
        assert_eq!(
            candidate_names(&engine, "svc.User", "Set", "api.sub.example.com"),
            ["default/specific-wildcard", "default/fallback"]
        );
        assert_eq!(
            candidate_names(&engine, "svc.User", "Get", "example.com"),
            ["default/fallback"]
        );
        let (winner, ()) = engine
            .select("svc.User", "Get", "api.sub.example.com", |r| {
                Ok((r.resource_key == "default/wildcard").then_some(()))
            })
            .unwrap()
            .unwrap();
        assert_eq!(winner.resource_key, "default/wildcard");
    }

    #[test]
    fn hostname_fanout_reuses_one_unit_and_nonmatching_host_cannot_supply_precedence() {
        let route = with_hosts(
            make_route(None, Some("Get"), None, "multi"),
            &["unrelated.long.example.com", "*.example.com"],
        );
        let engine = GrpcMatchEngine::new(vec![
            Arc::clone(&route),
            with_hosts(make_route(None, Some("Get"), None, "exact"), &["api.example.com"]),
        ]);
        assert_eq!(
            candidate_names(&engine, "svc", "Get", "api.example.com"),
            ["default/exact", "default/multi"]
        );
        let (winner, ()) = engine
            .select("svc", "Get", "other.example.com", |_| Ok(Some(())))
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(winner, &route));
    }

    #[test]
    fn real_predicate_errors_do_not_visit_broader_candidates() {
        let engine = GrpcMatchEngine::new(vec![
            with_hosts(make_route(None, Some("Get"), None, "exact"), &["api.example.com"]),
            with_hosts(make_route(None, Some("Get"), None, "wildcard"), &["*.example.com"]),
        ]);
        let mut visits = 0;
        let result = engine.select::<()>("svc", "Get", "api.example.com", |_| {
            visits += 1;
            Err(EdError::InvalidGrpcPath("predicate failure".to_string()))
        });
        assert!(matches!(result, Err(EdError::InvalidGrpcPath(_))));
        assert_eq!(visits, 1);
    }

    #[test]
    fn shared_hostname_index_handles_short_names_and_exact_key_collision() {
        let named = |name, host| with_hosts(make_route(None, Some("Get"), None, name), &[host]);
        let engine = GrpcMatchEngine::new(vec![
            named("exact", "api.sub.example.com"),
            named("wildcard", "*.example.com"),
            named("specific", "*.sub.example.com"),
            named("unrelated", "*.abcdefghijklmn.com"),
            named("short", "grpc"),
            named("short-wildcard", "*.com"),
        ]);
        assert_eq!(
            candidate_names(&engine, "svc", "Get", "foo.api.sub.example.com"),
            ["default/specific", "default/wildcard", "default/short-wildcard"]
        );
        assert_eq!(candidate_names(&engine, "svc", "Get", "grpc"), ["default/short"]);
        assert_eq!(
            candidate_names(&engine, "svc", "Get", "a.b.com"),
            ["default/short-wildcard"]
        );
    }

    #[test]
    fn reused_hostname_tree_never_reuses_ranks_from_the_previous_array() {
        let exact = with_hosts(make_route(None, Some("Get"), None, "exact"), &["api.example.com"]);
        let wildcard = with_hosts(make_route(None, Some("Get"), None, "wildcard"), &["*.example.com"]);
        let old = GrpcMatchEngine::new(vec![exact.clone(), wildcard.clone()]);
        // This rule sorts before both old rules and shifts their numeric ranks.
        let added = with_hosts(
            make_route(Some("svc.User"), Some("Get"), None, "added"),
            &["*.example.com"],
        );
        let updated = GrpcMatchEngine::rebuild(vec![exact.clone(), wildcard.clone(), added], Some(&old)).unwrap();
        assert!(old.shares_hosts(&updated));
        assert_eq!(
            candidate_names(&old, "svc.User", "Get", "api.example.com"),
            ["default/exact", "default/wildcard"]
        );
        assert_eq!(
            candidate_names(&updated, "svc.User", "Get", "api.example.com"),
            ["default/exact", "default/added", "default/wildcard"]
        );
        assert!(updated.routes.iter().any(|route| Arc::ptr_eq(route, &exact)));
        assert!(updated.routes.iter().any(|route| Arc::ptr_eq(route, &wildcard)));

        let topology = GrpcMatchEngine::rebuild(
            vec![with_hosts(make_route(None, Some("Get"), None, "renamed"), &["grpc"])],
            Some(&updated),
        )
        .unwrap();
        assert!(!updated.shares_hosts(&topology));
        assert_eq!(candidate_names(&topology, "svc", "Get", "grpc"), ["default/renamed"]);
        assert!(candidate_names(&topology, "svc", "Get", "api.example.com").is_empty());
        assert_eq!(
            candidate_names(&updated, "svc.User", "Get", "api.example.com"),
            ["default/exact", "default/added", "default/wildcard"]
        );
    }

    #[test]
    fn cross_index_precedence_is_independent_of_insertion_order() {
        let routes = vec![
            make_route(Some("svc.User"), Some("Get"), None, "exact"),
            make_route(
                Some(r"^svc\.User$"),
                None,
                Some(GRPCMethodMatchType::RegularExpression),
                "long-regex-service",
            ),
            make_route(None, Some("Get"), None, "method"),
            make_header_only_route(vec![], "any", 0, 0),
        ];
        for offset in 0..routes.len() {
            let mut permuted = routes.clone();
            permuted.rotate_left(offset);
            if offset % 2 == 0 {
                permuted.reverse();
            }
            let engine = GrpcMatchEngine::new(permuted);
            assert_eq!(
                candidate_names(&engine, "svc.User", "Get", "example.com"),
                [
                    "default/long-regex-service",
                    "default/exact",
                    "default/method",
                    "default/any"
                ]
            );
        }
    }

    #[test]
    fn equal_lengths_compare_headers_then_age_then_identity_then_rule_and_match() {
        let mut routes = Vec::new();
        for (name, headers, age, key, rule, matched) in [
            ("headers", 2, 99, "default/z", 9, 9),
            ("age", 1, 1, "default/z", 9, 9),
            ("identity", 1, 2, "default/a", 9, 9),
            ("rule", 1, 2, "default/b", 0, 9),
            ("match", 1, 2, "default/b", 1, 0),
            ("last", 1, 2, "default/b", 1, 1),
        ] {
            let mut route = make_route(None, Some("Get"), None, name);
            let spec = &mut Arc::get_mut(&mut route).unwrap().specificity;
            spec.header_count = headers;
            spec.creation_ts_ms = age;
            spec.route_key = key.to_string();
            spec.rule_index = rule;
            spec.match_index = matched;
            routes.push(route);
        }
        routes.reverse();
        let engine = GrpcMatchEngine::new(routes);
        assert_eq!(
            candidate_names(&engine, "svc", "Get", "example.com"),
            [
                "default/headers",
                "default/age",
                "default/identity",
                "default/rule",
                "default/match",
                "default/last"
            ]
        );
    }

    #[test]
    fn exact_selection_only_visits_candidates_for_the_requested_service_and_method() {
        let mut routes: Vec<_> = (0..1000)
            .map(|i| {
                make_route(
                    Some(&format!("svc.Service{i}")),
                    Some("Call"),
                    None,
                    &format!("route{i}"),
                )
            })
            .collect();
        routes.push(make_route(None, Some("Call"), None, "method"));
        let engine = GrpcMatchEngine::new(routes);
        let mut visited = 0;
        let (winner, ()) = engine
            .select("svc.Service500", "Call", "example.com", |_| {
                visited += 1;
                Ok(Some(()))
            })
            .unwrap()
            .unwrap();
        assert_eq!(winner.resource_key, "default/route500");
        assert_eq!(visited, 1);
        assert_eq!(
            candidate_names(&engine, "svc.Service500", "Call", "example.com"),
            ["default/route500", "default/method"]
        );
    }
}
