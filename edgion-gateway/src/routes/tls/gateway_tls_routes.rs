use crate::routes::tls::route_unit::TlsRouteUnit;
use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::matching::route::{check_gateway_listener_match, ResolvedParentRef};
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use edgion_algo::matcher::radix_hostname::{
    normalize_route_sni, radix_hostname_matches, RadixHostnameMatcher, RadixHostnamePattern, RadixHostnamePatternRef,
};
use edgion_resources::ListenerKey;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

/// One Route plus only its accepted attachments relevant to this pattern/key.
struct TlsRouteCandidate {
    unit: Arc<TlsRouteUnit>,
    attachment_indices: Vec<usize>,
}

/// Immutable per-physical-listener candidates, published by the outer ArcSwap.
pub struct TlsRouteTable {
    hosts: RadixHostnameMatcher<Vec<TlsRouteCandidate>>,
}

fn tls_route_precedence(a: &Arc<TlsRouteUnit>, b: &Arc<TlsRouteUnit>) -> std::cmp::Ordering {
    a.route
        .metadata
        .creation_timestamp
        .cmp(&b.route.metadata.creation_timestamp)
        .then_with(|| a.route.metadata.namespace.cmp(&b.route.metadata.namespace))
        .then_with(|| a.route.metadata.name.cmp(&b.route.metadata.name))
}

impl TlsRouteTable {
    fn from_hostname_result(
        result: Result<RadixHostnameMatcher<Vec<TlsRouteCandidate>>, impl Display>,
        pattern_count: usize,
        unit_count: usize,
    ) -> Self {
        match result {
            Ok(hosts) => Self { hosts },
            Err(error) => {
                tracing::error!(
                    component = "tls_route_hostname_index",
                    pattern_count,
                    unit_count,
                    err = %error,
                    "Failed to build the TLS Route hostname index; publishing an empty Listener table"
                );
                Self {
                    hosts: RadixHostnameMatcher::default(),
                }
            }
        }
    }

    pub fn new() -> Self {
        Self {
            hosts: RadixHostnameMatcher::default(),
        }
    }

    /// Cold diagnostic count of retained candidate entries across hostname
    /// groups. This is not a unique Route count: one Route may contribute an
    /// entry to multiple hostname groups and listener tables.
    pub(super) fn candidate_entry_count(&self) -> usize {
        self.hosts.groups().map(|(_, candidates)| candidates.len()).sum()
    }

    /// Existing L4 discovery cost remains explicit. Reuse both the tree and
    /// unchanged ordered candidate groups; never rebuild cached Route selectors.
    pub fn with_units(&self, listener_key: &ListenerKey, units: &HashMap<String, Arc<TlsRouteUnit>>) -> Self {
        let mut groups: HashMap<RadixHostnamePattern, Vec<TlsRouteCandidate>> = HashMap::new();
        for unit in units.values() {
            if unit.route.spec.validate_hostnames().is_err() {
                continue;
            }
            let records = unit.route.spec.resolved_listeners.as_deref().unwrap_or_default();
            for declared in unit.route.spec.hostnames.as_deref().unwrap_or_default() {
                let pattern =
                    RadixHostnamePatternRef::parse(declared).expect("the entire Route hostname list was validated");
                let attachment_indices: Vec<_> = unit
                    .accepted_attachments
                    .iter()
                    .enumerate()
                    .filter(|(_, accepted)| {
                        let attachment = &records[accepted.schema_index];
                        attachment.listener_key == *listener_key
                            && attachment.hostnames.iter().any(|effective| {
                                RadixHostnamePatternRef::parse(effective)
                                    .is_ok_and(|effective| pattern.intersection(effective).is_some())
                            })
                    })
                    .map(|(index, _)| index)
                    .collect();
                if attachment_indices.is_empty() {
                    continue;
                }
                let key = RadixHostnamePattern::parse(declared)
                    .expect("the same declared hostname already parsed successfully");
                let candidates = groups.entry(key).or_default();
                // Duplicate declared names never duplicate a Route or its WRR state.
                if !candidates.iter().any(|candidate| Arc::ptr_eq(&candidate.unit, unit)) {
                    candidates.push(TlsRouteCandidate {
                        unit: Arc::clone(unit),
                        attachment_indices,
                    });
                }
            }
        }
        let groups: HashMap<RadixHostnamePattern, Arc<Vec<TlsRouteCandidate>>> = groups
            .into_iter()
            .map(|(pattern, mut candidates)| {
                candidates.sort_by(|a, b| tls_route_precedence(&a.unit, &b.unit));
                let old = self.hosts.get(&pattern).filter(|old| {
                    old.len() == candidates.len()
                        && old
                            .iter()
                            .zip(&candidates)
                            .all(|(a, b)| Arc::ptr_eq(&a.unit, &b.unit) && a.attachment_indices == b.attachment_indices)
                });
                let value = old.cloned().unwrap_or_else(|| Arc::new(candidates));
                (pattern, value)
            })
            .collect();
        let pattern_count = groups.len();
        Self::from_hostname_result(self.hosts.with_groups(groups), pattern_count, units.len())
    }

    /// Hostname and attachment eligibility are conjunctive, before owner policy.
    pub fn select_route(&self, sni: &str, index: &GatewayInfoIndex) -> Option<(Arc<TlsRouteUnit>, Arc<GatewayInfo>)> {
        self.select_by(sni, |unit, parent, hostname| {
            check_gateway_listener_match(
                std::slice::from_ref(parent),
                index,
                hostname,
                unit.route.metadata.namespace.as_deref().unwrap_or("default"),
                "TLSRoute",
            )
        })
    }

    fn select_by<T>(
        &self,
        sni: &str,
        mut eligible: impl FnMut(&TlsRouteUnit, &ResolvedParentRef, &str) -> Option<T>,
    ) -> Option<(Arc<TlsRouteUnit>, T)> {
        let normalized = normalize_route_sni(sni).ok()?;
        let query = normalized.query();
        for group in self.hosts.matching_values(query) {
            for candidate in group {
                let unit = &candidate.unit;
                let records = unit.route.spec.resolved_listeners.as_deref().unwrap_or_default();
                for &index in &candidate.attachment_indices {
                    let accepted = &unit.accepted_attachments[index];
                    let attachment = &records[accepted.schema_index];
                    if !attachment
                        .hostnames
                        .iter()
                        .any(|hostname| radix_hostname_matches(query.as_str(), hostname))
                    {
                        continue;
                    }
                    if let Some(owner) = eligible(unit, &accepted.parent, query.as_str()) {
                        return Some((Arc::clone(unit), owner));
                    }
                }
            }
        }
        None
    }

    #[cfg(test)]
    fn from_units(listener_key: &ListenerKey, units: &HashMap<String, Arc<TlsRouteUnit>>) -> Self {
        Self::new().with_units(listener_key, units)
    }

    /// Unit-test view of hostname/attachment selection, without an owner inventory.
    #[cfg(test)]
    pub(crate) fn match_route(&self, sni: &str) -> Option<Arc<TlsRouteUnit>> {
        self.select_by(sni, |_, _, _| Some(())).map(|(unit, ())| unit)
    }
}

impl Default for TlsRouteTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/tls/gateway_tls_routes_test.rs"]
mod tests;
