//! Kubernetes Status Handler
//!
//! Handles resource status updates for Kubernetes mode.
//! Status is updated via the K8s API using Server-Side Apply on the status subresource.
//!
//! ## Supported Resources
//!
//! - Gateway -> GatewayStatus
//! - HTTPRoute -> HTTPRouteStatus
//! - Other resources with status subresources
//!
//! ## Usage
//!
//! The status handler is called after resource processing in the worker loop,
//! before `workqueue.done()` is called.

use crate::types::resources::common::Condition;
use chrono::Utc;
use kube::api::{Patch, PatchParams};
use kube::{Api, Client};
use serde::Serialize;
use std::sync::Arc;

/// Kubernetes status handler
pub struct KubernetesStatusHandler {
    client: Client,
}

impl KubernetesStatusHandler {
    /// Create a new KubernetesStatusHandler
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Get the K8s client
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Create a Ready condition
    pub fn ready_condition() -> Condition {
        Condition {
            type_: "Ready".to_string(),
            status: "True".to_string(),
            reason: "ProcessingComplete".to_string(),
            message: "Resource processed successfully".to_string(),
            last_transition_time: Utc::now().to_rfc3339(),
            observed_generation: None,
        }
    }

    /// Create an Error condition
    pub fn error_condition(reason: &str, message: &str) -> Condition {
        Condition {
            type_: "Ready".to_string(),
            status: "False".to_string(),
            reason: reason.to_string(),
            message: message.to_string(),
            last_transition_time: Utc::now().to_rfc3339(),
            observed_generation: None,
        }
    }

    /// Update status for a namespaced resource using Server-Side Apply
    ///
    /// # Arguments
    /// * `namespace` - Resource namespace
    /// * `name` - Resource name
    /// * `status` - Status object to apply
    pub async fn update_status<R, S>(&self, namespace: &str, name: &str, status: S) -> Result<(), kube::Error>
    where
        R: kube::Resource<Scope = kube::core::NamespaceResourceScope>
            + Clone
            + std::fmt::Debug
            + serde::de::DeserializeOwned
            + Serialize,
        R::DynamicType: Default,
        S: Serialize,
    {
        let api: Api<R> = Api::namespaced(self.client.clone(), namespace);

        // Build status patch
        let patch = serde_json::json!({
            "status": status
        });

        let params = PatchParams::apply("edgion-controller").force();

        api.patch_status(name, &params, &Patch::Apply(&patch)).await?;

        tracing::debug!(
            component = "k8s_status",
            kind = std::any::type_name::<R>(),
            namespace = namespace,
            name = name,
            "Updated resource status"
        );

        Ok(())
    }

    /// Update status for a cluster-scoped resource using Server-Side Apply
    pub async fn update_cluster_status<R, S>(&self, name: &str, status: S) -> Result<(), kube::Error>
    where
        R: kube::Resource<Scope = kube::core::ClusterResourceScope>
            + Clone
            + std::fmt::Debug
            + serde::de::DeserializeOwned
            + Serialize,
        R::DynamicType: Default,
        S: Serialize,
    {
        let api: Api<R> = Api::all(self.client.clone());

        // Build status patch
        let patch = serde_json::json!({
            "status": status
        });

        let params = PatchParams::apply("edgion-controller").force();

        api.patch_status(name, &params, &Patch::Apply(&patch)).await?;

        tracing::debug!(
            component = "k8s_status",
            kind = std::any::type_name::<R>(),
            name = name,
            "Updated cluster resource status"
        );

        Ok(())
    }
}

/// Wrapper for sharing status handler across tasks
pub type SharedStatusHandler = Arc<KubernetesStatusHandler>;

/// Create a shared status handler
pub fn create_shared_handler(client: Client) -> SharedStatusHandler {
    Arc::new(KubernetesStatusHandler::new(client))
}
