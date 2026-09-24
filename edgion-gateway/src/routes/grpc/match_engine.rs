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
#[path = "../../../tests/unit/routes/grpc/match_engine_test.rs"]
mod tests;
