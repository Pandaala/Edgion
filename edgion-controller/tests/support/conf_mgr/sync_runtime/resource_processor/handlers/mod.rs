use super::*;

#[cfg(test)]
pub(super) fn resolve_policy_ancestor_gateways(
    policy_ns: &str,
    target_service_names: &[&str],
    policy_key: &str,
    ctx: &HandlerContext,
) -> BTreeMap<String, BTreeSet<String>> {
    let targets: Vec<PolicyTargetKey> = target_service_names
        .iter()
        .map(|name| PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: policy_ns.to_string(),
            name: (*name).to_string(),
        })
        .collect();
    resolve_policy_ancestor_gateways_for_targets(&targets, policy_key, ctx)
        .into_iter()
        .map(|(gateway, targets)| {
            let service_keys = targets
                .into_iter()
                .map(|target| format!("{}/{}", target.namespace, target.name))
                .collect();
            (gateway, service_keys)
        })
        .collect()
}
