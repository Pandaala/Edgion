//! EdgionTls Resource Check
//!
//! Validates EdgionTls resources before apply.

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
/// Validates:
/// - Referenced Gateway exists in cache
///
/// # Arguments
/// * `ctx` - Resource check context
/// * `tls` - The EdgionTls resource to check
///
/// # Returns
/// `EdgionTlsCheckResult` with skip_reason if validation fails, or warnings if any
pub fn check_edgion_tls(ctx: &ResourceCheckContext, tls: &EdgionTls) -> EdgionTlsCheckResult {
    let mut result = EdgionTlsCheckResult::default();

    // Check if EdgionTls references a Gateway that exists
    if let Some(parent_refs) = &tls.spec.parent_refs {
        if let Some(first_ref) = parent_refs.first() {
            let gateway_namespace = first_ref.namespace.as_ref().or(tls.metadata.namespace.as_ref());
            let gateway_name = &first_ref.name;

            if !ctx.gateway_exists(gateway_namespace.map(|s| s.as_str()), gateway_name) {
                let ns_display = gateway_namespace
                    .map(|s| s.as_str())
                    .unwrap_or("default");
                result.skip_reason = Some(format!(
                    "EdgionTls references Gateway that does not exist: {}/{}",
                    ns_display, gateway_name
                ));
                return result;
            }
        } else {
            result.skip_reason = Some("EdgionTls has empty parent_refs".to_string());
            return result;
        }
    } else {
        result.skip_reason = Some("EdgionTls has no parent_refs".to_string());
        return result;
    }

    // Add more validation checks here as needed
    // e.g., Secret existence check (as warning, since Secret might come later)
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
