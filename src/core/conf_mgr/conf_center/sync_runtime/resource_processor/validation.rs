//! Resource Validation
//!
//! Provides validation logic for resources before they are applied.
//! This module consolidates validation functions that were previously in resource_check.

use crate::core::conf_sync::ConfigServer;
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
/// * `config_server` - ConfigServer to check resource dependencies
/// * `tls` - The EdgionTls resource to check
///
/// # Returns
/// `EdgionTlsCheckResult` with warnings if any (skip_reason is no longer used for Gateway checks)
pub fn check_edgion_tls(config_server: &ConfigServer, tls: &EdgionTls) -> EdgionTlsCheckResult {
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

    if !secret_exists(config_server, secret_namespace.map(|s| s.as_str()), &tls.spec.secret_ref.name) {
        result.warnings.push(format!(
            "Secret '{}' not found, EdgionTls will be applied but TLS may not work until Secret is available",
            tls.spec.secret_ref.name
        ));
    }

    result
}

/// Check if a Secret exists in the cache
///
/// # Arguments
/// * `config_server` - ConfigServer to check
/// * `namespace` - The namespace to check
/// * `name` - The Secret name
fn secret_exists(config_server: &ConfigServer, namespace: Option<&str>, name: &str) -> bool {
    let secrets = config_server.secrets.list_owned();
    secrets.data.iter().any(|s| {
        let s_name_matches = s.metadata.name.as_deref() == Some(name);
        let s_namespace_matches = match (namespace, s.metadata.namespace.as_deref()) {
            (Some(ns), Some(s_ns)) => ns == s_ns,
            (None, None) => true,
            (None, Some(_)) => true,
            _ => false,
        };
        s_name_matches && s_namespace_matches
    })
}
