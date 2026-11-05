//! Shared marker helper for cross-namespace EdgionConfigData reference denial marking.
//!
//! Used by both EdgionPlugins and EdgionStreamPlugins handlers to set or clear
//! `ref_denied` on every ref-bearing arm before the resource is pushed to the store.

use crate::conf_mgr::sync_runtime::resource_processor::status_utils::ResolvedRefsError;
use edgion_resources::refs::config_data_fields::{ConfigDataField, ConfigDataRole};
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::EdgionConfigData;

use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::decide_edgion_config_data_ref;
use crate::conf_mgr::sync_runtime::resource_processor::HandlerContext;

/// Expected ConfigEntry variant for a given ref kind. Marking only applies to a
/// present, right-type target (a missing/wrong-type target is the NotFound path,
/// handled in Task 5's update_status, not a denial).
#[derive(Clone, Copy)]
pub(crate) enum ExpectedEntry {
    IpList,
}
impl ExpectedEntry {
    pub(crate) fn matches(self, data: &ConfigEntry) -> bool {
        matches!(data, ConfigEntry::IpList(_))
    }
}

/// One current target decision shared by marking, readonly status and URL expansion.
/// Invalid identities return before store access or permission queries.
pub(crate) struct ConfigDataResolution {
    pub target: Option<EdgionConfigData>,
    pub denied: Option<RefDenied>,
    pub error: Option<ResolvedRefsError>,
    pub failure: &'static str,
}

pub(crate) fn resolve_field(field: &ConfigDataField<'_>, ctx: &HandlerContext) -> ConfigDataResolution {
    let query = match field.query() {
        Ok(query) => query,
        Err(error) => {
            return ConfigDataResolution {
                target: None,
                denied: None,
                error: Some(ResolvedRefsError::InvalidReference { error }),
                failure: "has an invalid reference",
            }
        }
    };
    let target_namespace = query.target().scope().namespace().expect("namespaced target");
    let target_name = query.target().name();
    let unavailable = || ResolvedRefsError::EdgionConfigDataNotFound {
        namespace: target_namespace.to_string(),
        name: target_name.to_string(),
        expected_type: field.role().expected_type().to_string(),
    };
    let Some(target) = ctx
        .runtime()
        .edgion_config_data_store
        .get(Some(target_namespace), target_name)
    else {
        return ConfigDataResolution {
            target: None,
            denied: None,
            error: Some(unavailable()),
            failure: "is unavailable",
        };
    };
    if !field.role().matches(&target.spec.data) {
        return ConfigDataResolution {
            target: None,
            denied: None,
            error: Some(unavailable()),
            failure: "has the wrong type",
        };
    }
    let allowed = decide_edgion_config_data_ref(
        crate::config::is_reference_grant_validation_enabled(),
        query.source().scope() == query.target().scope(),
        target.spec.visibility,
        ctx.runtime().ref_grant_store.check_query(query),
    );
    if !allowed {
        let from_kind = match query.source().group_kind().kind() {
            "EdgionPlugins" => "EdgionPlugins",
            "EdgionStreamPlugins" => "EdgionStreamPlugins",
            _ => unreachable!("fixed source role"),
        };
        return ConfigDataResolution {
            target: None,
            failure: "is not permitted",
            denied: Some(RefDenied {
                target_namespace: target_namespace.to_string(),
                target_name: target_name.to_string(),
                reason: Some("NoMatchingReferenceGrant".into()),
            }),
            error: Some(ResolvedRefsError::RefNotPermitted {
                from_namespace: query
                    .source()
                    .scope()
                    .namespace()
                    .expect("namespaced source")
                    .to_string(),
                from_kind,
                target_namespace: target_namespace.to_string(),
                target_name: target_name.to_string(),
            }),
        };
    }
    if !target.spec.enable {
        let error = if field.role() == ConfigDataRole::ClusterRegion {
            ResolvedRefsError::EdgionConfigDataInvalid {
                namespace: target_namespace.to_string(),
                name: target_name.to_string(),
                detail: "ClusterRegionSetting is disabled".into(),
            }
        } else {
            unavailable()
        };
        return ConfigDataResolution {
            target: None,
            denied: None,
            error: Some(error),
            failure: "is disabled",
        };
    }
    ConfigDataResolution {
        target: Some(target),
        denied: None,
        error: None,
        failure: "",
    }
}

/// Pure marking needs no material read for a valid same-namespace field.
pub(crate) fn denial_marker(field: &ConfigDataField<'_>, ctx: &HandlerContext) -> Option<RefDenied> {
    let query = field.query().ok()?;
    if query.source().scope() == query.target().scope() {
        return None;
    }
    resolve_field(field, ctx).denied
}
