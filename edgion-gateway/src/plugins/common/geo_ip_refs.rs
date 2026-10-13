//! Cold compiled CIDR rule grid; every decision uses one shared ConfigData view.
use crate::plugins::runtime::conditions::ip_refs::{compile_geo_leaf, BoundIpPredicate, IpConditionCompiler};
use crate::plugins::runtime::edgion_config_data::EdgionConfigDataSnapshot;
use crate::plugins::runtime::PluginSession;
use edgion_resources::refs::ip_list_fields::BoundGeoIpEntry;
use edgion_resources::refs::ReferenceError;
use edgion_resources::resources::edgion_plugins::plugin_configs::{GeoIpLocationConfig, GeoRuleKey, GeoRuleSide};
use std::net::IpAddr;
use std::sync::Arc;

type RuleGrid = Vec<Vec<Option<Arc<BoundIpPredicate>>>>;
pub(crate) struct CompiledGeoIpRefs {
    deny: RuleGrid,
    allow: RuleGrid,
}
impl CompiledGeoIpRefs {
    fn empty(config: &GeoIpLocationConfig) -> Self {
        let grid = |groups: &[edgion_resources::resources::edgion_plugins::plugin_configs::GeoRuleGroup]| {
            groups
                .iter()
                .map(|group| group.rules.iter().map(|_| None).collect())
                .collect()
        };
        Self {
            deny: grid(&config.deny_rule_groups),
            allow: grid(&config.allow_rule_groups),
        }
    }
    pub(crate) fn from_http(
        config: &GeoIpLocationConfig,
        compiler: &IpConditionCompiler<'_>,
    ) -> Result<Self, ReferenceError> {
        let mut result = Self::empty(config);
        for (grid, groups) in [
            (&mut result.deny, &config.deny_rule_groups),
            (&mut result.allow, &config.allow_rule_groups),
        ] {
            for (rows, group) in grid.iter_mut().zip(groups) {
                for (slot, rule) in rows.iter_mut().zip(&group.rules) {
                    if rule.key == GeoRuleKey::Cidr {
                        *slot = Some(compiler.bind(&rule.cidr).ok_or(ReferenceError::StaleBinding)?);
                    }
                }
            }
        }
        Ok(result)
    }
    pub(crate) fn from_stream(entry: BoundGeoIpEntry<'_>) -> Result<Self, ReferenceError> {
        let mut result = Self::empty(entry.config());
        for leaf in entry.leaves() {
            if leaf.config().ip_refs.is_empty() && !leaf.is_supported() {
                continue;
            }
            let (deny, group, rule) = leaf.coordinates();
            let predicate = compile_geo_leaf(leaf)?;
            (if deny { &mut result.deny } else { &mut result.allow })[group][rule] = Some(predicate);
        }
        Ok(result)
    }
    fn predicates(&self) -> impl Iterator<Item = &Arc<BoundIpPredicate>> {
        self.deny.iter().chain(&self.allow).flatten().flatten()
    }
    pub(crate) fn has_refs(&self) -> bool {
        self.predicates().next().is_some()
    }
    pub(crate) fn available(&self, session: &dyn PluginSession) -> bool {
        self.predicates().all(|predicate| predicate.available(session))
    }
    pub(crate) fn available_in(&self, view: &EdgionConfigDataSnapshot) -> bool {
        self.predicates().all(|predicate| predicate.available_in(view))
    }
    fn predicate(&self, side: GeoRuleSide, group: usize, rule: usize) -> Option<&BoundIpPredicate> {
        (match side {
            GeoRuleSide::Deny => &self.deny,
            GeoRuleSide::Allow => &self.allow,
        })
        .get(group)?
        .get(rule)?
        .as_deref()
    }
    pub(crate) fn contains(
        &self,
        session: &dyn PluginSession,
        side: GeoRuleSide,
        group: usize,
        rule: usize,
        ip: IpAddr,
    ) -> bool {
        self.predicate(side, group, rule)
            .is_some_and(|predicate| predicate.contains(session, ip))
    }
    pub(crate) fn contains_in(
        &self,
        view: &EdgionConfigDataSnapshot,
        side: GeoRuleSide,
        group: usize,
        rule: usize,
        ip: IpAddr,
    ) -> bool {
        self.predicate(side, group, rule)
            .is_some_and(|predicate| predicate.contains_in(view, ip))
    }
    /// Call only after authored validation and successful actual-owner binding.
    /// This private runtime copy is never admitted or used to mint authority.
    pub(crate) fn sanitize(config: &mut GeoIpLocationConfig) {
        for rule in config
            .deny_rule_groups
            .iter_mut()
            .chain(&mut config.allow_rule_groups)
            .flat_map(|group| &mut group.rules)
        {
            if rule.key == GeoRuleKey::Cidr {
                rule.cidr = Default::default();
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/common/geo_ip_refs_test.rs"]
mod tests;
