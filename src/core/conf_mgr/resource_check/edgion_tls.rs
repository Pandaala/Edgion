//! EdgionTls Resource Check
//!
//! Validates EdgionTls resources before apply.
//! Note: Gateway existence check is removed - controlled by K8s RBAC instead.

use super::ResourceCheckContext;
use crate::types::prelude_resources::EdgionTls;

/// Result of EdgionTls validation check
#[derive(Debug, Default)]
pub struct EdgionTlsCheckResult {
    /// If set, the resource should be skipped (not applied)
    /// Contains the reason for skipping
    pub skip_reason: Option<String>,

    /// Warnings that should be logged but don't prevent apply
    pub warnings: Vec<String>,
}

impl EdgionTlsCheckResult {
    /// Check if the resource should be skipped
    pub fn should_skip(&self) -> bool {
        self.skip_reason.is_some()
    }

    /// Check if there are any warnings
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

/// Check EdgionTls resource for validity
///
/// Note: Gateway existence is NOT checked here. Permission control is handled by K8s RBAC.
/// This function only provides warnings for potential issues.
///
/// # Arguments
/// * `ctx` - Resource check context
/// * `tls` - The EdgionTls resource to check
///
/// # Returns
/// `EdgionTlsCheckResult` with warnings if any (skip_reason is no longer used for Gateway checks)
pub fn check_edgion_tls(ctx: &ResourceCheckContext, tls: &EdgionTls) -> EdgionTlsCheckResult {
    let mut result = EdgionTlsCheckResult::default();

    // Warn if parent_refs is empty (structural issue)
    if tls.spec.parent_refs.as_ref().is_none_or(|p| p.is_empty()) {
        result
            .warnings
            .push("EdgionTls has no parent_refs, it may not be associated with any Gateway".to_string());
    }

    // Secret existence check (as warning only, since Secret might come later)
    let secret_namespace = tls
        .spec
        .secret_ref
        .namespace
        .as_ref()
        .or(tls.metadata.namespace.as_ref());

    if !ctx.secret_exists(secret_namespace.map(|s| s.as_str()), &tls.spec.secret_ref.name) {
        result.warnings.push(format!(
            "Secret '{}' not found, EdgionTls will be applied but TLS may not work until Secret is available",
            tls.spec.secret_ref.name
        ));
    }

    result
}
