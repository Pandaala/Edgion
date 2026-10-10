//! Cross-namespace permission decision for EdgionConfigData references.

use edgion_resources::resources::edgion_config_data::ConfigDataVisibility;

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

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/ref_grant/edgion_config_data_grant_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../tests/support/conf_mgr/sync_runtime/resource_processor/ref_grant/edgion_config_data_grant.rs"]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::GATEWAY_GROUP;
