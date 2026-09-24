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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/ref_grant/edgion_config_data_grant_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../tests/support/conf_mgr/sync_runtime/resource_processor/ref_grant/edgion_config_data_grant.rs"]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::GATEWAY_GROUP;
