use super::*;

#[cfg(test)]
pub(super) fn service_target_key(namespace: &str, name: &str) -> PolicyTargetKey {
    PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: namespace.to_string(),
        name: name.to_string(),
    }
}
