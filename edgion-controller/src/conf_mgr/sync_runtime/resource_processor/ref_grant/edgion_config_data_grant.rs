//! Cross-namespace permission decision for EdgionConfigData references.

use edgion_resources::resources::edgion_config_data::ConfigDataVisibility;

use crate::conf_mgr::ControllerRuntime;
use crate::config::is_reference_grant_validation_enabled;

/// API group shared by EdgionPlugins / EdgionStreamPlugins / EdgionConfigData.
pub const EDGION_GROUP: &str = "edgion.io";
#[cfg(test)]
pub const GATEWAY_GROUP: &str = "gateway.networking.k8s.io";

/// Pure permission decision. `grant_allows` is the result of
/// `ReferenceGrantStore::check_reference_allowed`. Order: flag off → allow (no-op);
/// same-ns → allow; visibility Cluster → allow; else require a grant.
pub(crate) fn decide_edgion_config_data_ref(
    validation_enabled: bool,
    same_namespace: bool,
    visibility: ConfigDataVisibility,
    grant_allows: bool,
) -> bool {
    if !validation_enabled {
        return true;
    }
    if same_namespace {
        return true;
    }
    if visibility == ConfigDataVisibility::Cluster {
        return true;
    }
    grant_allows
}

/// Returns true iff the referrer may reference the target EdgionConfigData.
/// `from_kind` is `"EdgionPlugins"` or `"EdgionStreamPlugins"`. `visibility` is the
/// target's `spec.visibility` (caller reads it from the store; defaults to `Namespace`).
pub fn is_edgion_config_data_ref_allowed(
    runtime: &ControllerRuntime,
    from_ns: &str,
    from_kind: &str,
    to_ns: &str,
    to_name: &str,
    visibility: ConfigDataVisibility,
) -> bool {
    is_edgion_config_data_ref_allowed_from(runtime, from_ns, EDGION_GROUP, from_kind, to_ns, to_name, visibility)
}

/// Returns true iff an explicitly identified source group/kind may reference
/// the target EdgionConfigData.
#[allow(clippy::too_many_arguments)]
pub fn is_edgion_config_data_ref_allowed_from(
    runtime: &ControllerRuntime,
    from_ns: &str,
    from_group: &str,
    from_kind: &str,
    to_ns: &str,
    to_name: &str,
    visibility: ConfigDataVisibility,
) -> bool {
    let grant_allows = runtime.ref_grant_store.check_reference_allowed(
        from_ns,
        from_group,
        from_kind,
        to_ns,
        EDGION_GROUP,
        "EdgionConfigData",
        Some(to_name),
    );
    decide_edgion_config_data_ref(
        is_reference_grant_validation_enabled(),
        from_ns == to_ns,
        visibility,
        grant_allows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::{ReferenceGrant, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::sync::Arc;
    use ConfigDataVisibility::*;

    #[test]
    fn truth_table() {
        // flag off → always allowed (no-op), regardless of everything else
        assert!(decide_edgion_config_data_ref(false, false, Namespace, false));
        // same namespace → always allowed
        assert!(decide_edgion_config_data_ref(true, true, Namespace, false));
        // cross-ns, visibility Cluster → allowed without grant
        assert!(decide_edgion_config_data_ref(true, false, Cluster, false));
        // cross-ns, Namespace, grant allows → allowed
        assert!(decide_edgion_config_data_ref(true, false, Namespace, true));
        // cross-ns, Namespace, no grant → DENIED
        assert!(!decide_edgion_config_data_ref(true, false, Namespace, false));
    }

    fn grant(from_group: &str, from_kind: &str) -> ReferenceGrant {
        ReferenceGrant {
            metadata: ObjectMeta {
                namespace: Some("target".to_string()),
                name: Some("allow-policy".to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantSpec {
                from: vec![ReferenceGrantFrom {
                    group: from_group.to_string(),
                    kind: from_kind.to_string(),
                    namespace: "source".to_string(),
                }],
                to: vec![ReferenceGrantTo {
                    group: EDGION_GROUP.to_string(),
                    kind: "EdgionConfigData".to_string(),
                    name: Some("policy".to_string()),
                }],
            },
        }
    }

    #[test]
    fn gateway_grant_requires_exact_gateway_api_group() {
        let runtime = ControllerRuntime::default();
        runtime.ref_grant_store.upsert(
            "target/allow-policy".to_string(),
            Arc::new(grant(GATEWAY_GROUP, "Gateway")),
        );

        assert!(is_edgion_config_data_ref_allowed_from(
            &runtime,
            "source",
            GATEWAY_GROUP,
            "Gateway",
            "target",
            "policy",
            Namespace,
        ));
        assert!(!is_edgion_config_data_ref_allowed_from(
            &runtime,
            "source",
            EDGION_GROUP,
            "Gateway",
            "target",
            "policy",
            Namespace,
        ));
    }

    #[test]
    fn existing_plugin_helper_keeps_edgion_group() {
        let runtime = ControllerRuntime::default();
        runtime.ref_grant_store.upsert(
            "target/allow-policy".to_string(),
            Arc::new(grant(EDGION_GROUP, "EdgionPlugins")),
        );

        assert!(is_edgion_config_data_ref_allowed(
            &runtime,
            "source",
            "EdgionPlugins",
            "target",
            "policy",
            Namespace,
        ));
    }
}
