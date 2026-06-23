//! Cross-namespace permission decision for EdgionConfigData references.

use edgion_resources::resources::edgion_config_data::ConfigDataVisibility;

use crate::conf_mgr::ControllerRuntime;
use crate::config::is_reference_grant_validation_enabled;

/// API group shared by EdgionPlugins / EdgionStreamPlugins / EdgionConfigData.
pub const EDGION_GROUP: &str = "edgion.io";

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
    let grant_allows = runtime.ref_grant_store.check_reference_allowed(
        from_ns,
        EDGION_GROUP,
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
}
