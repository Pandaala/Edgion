//! Shared marker helper for cross-namespace EdgionConfigData reference denial marking.
//!
//! Used by both EdgionPlugins and EdgionStreamPlugins handlers to set or clear
//! `ref_denied` on every ref-bearing arm before the resource is pushed to the store.

use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_config_data::{ConfigDataVisibility, ConfigEntry, EdgionConfigDataRef};

use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::is_edgion_config_data_ref_allowed;
use crate::conf_mgr::sync_runtime::resource_processor::HandlerContext;

/// Expected ConfigEntry variant for a given ref kind. Marking only applies to a
/// present, right-type target (a missing/wrong-type target is the NotFound path,
/// handled in Task 5's update_status, not a denial).
#[derive(Clone, Copy)]
pub(crate) enum ExpectedEntry {
    IpList,
    RegionRouteOverride,
    Selector,
}

impl ExpectedEntry {
    pub(crate) fn matches(self, data: &ConfigEntry) -> bool {
        matches!(
            (self, data),
            (ExpectedEntry::IpList, ConfigEntry::IpList(_))
                | (ExpectedEntry::RegionRouteOverride, ConfigEntry::RegionRouteOverride(_))
                | (ExpectedEntry::Selector, ConfigEntry::Selector(_))
        )
    }
}

/// Set or clear `ref_denied` on a single ref per the permission predicate.
///
/// - Same-namespace refs are always cleared (no denial possible).
/// - Cross-namespace refs whose target is missing or has the wrong ConfigEntry type
///   are also cleared — those are reported as NotFound in update_status, not a denial.
/// - Cross-namespace refs with a present, right-type target are allowed or denied
///   based on ReferenceGrant validation.
pub(crate) fn mark_one_ref(
    r: &mut EdgionConfigDataRef,
    from_ns: &str,
    from_kind: &str,
    expected: ExpectedEntry,
    ctx: &HandlerContext,
) {
    let to_ns = r.namespace.as_deref().unwrap_or(from_ns).to_string();
    if to_ns == from_ns {
        r.ref_denied = None;
        return;
    }
    let target = ctx.runtime().edgion_config_data_store.get(Some(&to_ns), &r.name);
    let right_type = target.as_ref().is_some_and(|t| expected.matches(&t.spec.data));
    if !right_type {
        // Missing / wrong-type -> reported as EdgionConfigDataNotFound in update_status, not denial.
        r.ref_denied = None;
        return;
    }
    let visibility = target
        .as_ref()
        .map(|t| t.spec.visibility)
        .unwrap_or(ConfigDataVisibility::Namespace);
    let allowed = is_edgion_config_data_ref_allowed(ctx.runtime(), from_ns, from_kind, &to_ns, &r.name, visibility);
    r.ref_denied = if allowed {
        None
    } else {
        Some(RefDenied {
            target_namespace: to_ns,
            target_name: r.name.clone(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        })
    };
}
