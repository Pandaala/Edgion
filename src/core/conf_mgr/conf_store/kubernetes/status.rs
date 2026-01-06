use crate::types::resources::gateway::{Gateway, GatewayStatus};
use crate::types::resources::http_route::{HTTPRoute, HTTPRouteStatus};
use anyhow::Result;
use kube::api::{Patch, PatchParams};
use kube::{Api, Client};
use serde_json::json;

/// StatusManager handles status updates for Kubernetes resources
pub struct StatusManager {
    client: Client,
    field_manager: String,
}

impl StatusManager {
    pub fn new(client: Client, field_manager: String) -> Self {
        Self { client, field_manager }
    }

    /// Update Gateway status using Server-Side Apply
    pub async fn update_gateway_status(&self, ns: &str, name: &str, status: GatewayStatus) -> Result<()> {
        let api: Api<Gateway> = Api::namespaced(self.client.clone(), ns);

        let patch = Patch::Apply(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "Gateway",
            "metadata": {
                "name": name,
                "namespace": ns,
            },
            "status": status
        }));

        let params = PatchParams::apply(&self.field_manager).force();
        api.patch_status(name, &params, &patch).await?;

        Ok(())
    }

    /// Update HTTPRoute status
    /// Note: This replaces the entire status. For shared resources,
    /// we should be careful to only update our controller's entry in parents list.
    /// Standard implementation requires managing the specific entry in parents.
    pub async fn update_http_route_status_full(&self, ns: &str, name: &str, status: HTTPRouteStatus) -> Result<()> {
        let api: Api<HTTPRoute> = Api::namespaced(self.client.clone(), ns);

        let patch = Patch::Apply(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "name": name,
                "namespace": ns,
            },
            "status": status
        }));

        let params = PatchParams::apply(&self.field_manager).force();
        api.patch_status(name, &params, &patch).await?;

        Ok(())
    }
}
