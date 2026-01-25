//! BackendTLSPolicy Handler
//!
//! Handles BackendTLSPolicy resources with Gateway API standard status management.

use crate::core::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, set_route_parent_conditions, HandlerContext, ProcessResult,
    ProcessorHandler,
};
use crate::types::prelude_resources::BackendTLSPolicy;
use crate::types::resources::backend_tls_policy::{BackendTLSPolicyStatus, PolicyAncestorStatus};
use crate::types::resources::common::ParentReference;

/// Controller name for status reporting
const CONTROLLER_NAME: &str = "edgion.io/gateway-controller";

/// BackendTLSPolicy handler
pub struct BackendTlsPolicyHandler;

impl BackendTlsPolicyHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BackendTlsPolicyHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<BackendTLSPolicy> for BackendTlsPolicyHandler {
    fn parse(&self, btp: BackendTLSPolicy, _ctx: &HandlerContext) -> ProcessResult<BackendTLSPolicy> {
        ProcessResult::Continue(btp)
    }

    fn update_status(&self, btp: &mut BackendTLSPolicy, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = btp.metadata.generation;

        // Initialize status if not present
        let status = btp
            .status
            .get_or_insert_with(|| BackendTLSPolicyStatus { ancestors: vec![] });

        // BackendTLSPolicy targets Services (via target_refs), but ancestors refer to Gateways
        // that have routes targeting those Services. For now, we create a synthetic ancestor
        // representing the policy's acceptance status.
        //
        // In a full implementation, this would be populated by analyzing which Gateways
        // have Routes that reference the targeted Services.

        // Create a synthetic ancestor for the policy itself
        let synthetic_ancestor = ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("BackendTLSPolicy".to_string()),
            namespace: btp.metadata.namespace.clone(),
            name: btp.metadata.name.clone().unwrap_or_default(),
            section_name: None,
            port: None,
        };

        // Find or create ancestor status
        let ancestor_status = status.ancestors.iter_mut().find(|as_| {
            as_.ancestor_ref.name == synthetic_ancestor.name
                && as_.ancestor_ref.namespace == synthetic_ancestor.namespace
        });

        if let Some(as_) = ancestor_status {
            // Update existing ancestor status
            set_route_parent_conditions(&mut as_.conditions, validation_errors, generation);
        } else {
            // Create new ancestor status
            let mut conditions = Vec::new();

            // Set Accepted condition
            if validation_errors.is_empty() {
                conditions.push(accepted_condition(generation));
            } else {
                conditions.push(condition_false(
                    condition_types::ACCEPTED,
                    "Invalid",
                    validation_errors.join("; "),
                    generation,
                ));
            }

            status.ancestors.push(PolicyAncestorStatus {
                ancestor_ref: synthetic_ancestor,
                controller_name: CONTROLLER_NAME.to_string(),
                conditions,
            });
        }
    }
}
