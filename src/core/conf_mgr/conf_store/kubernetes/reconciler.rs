use crate::core::conf_mgr::StatusStore;
use crate::core::conf_sync::ConfigServer;
use crate::types::resources::common::Condition;
use kube::ResourceExt;
use std::sync::Arc;
use tokio::time::{interval, Duration};

const RECONCILE_INTERVAL: u64 = 5; // seconds

pub struct StatusReconciler {
    config_server: Arc<ConfigServer>,
    status_store: Arc<dyn StatusStore>,
    gateway_class_name: String,
}

impl StatusReconciler {
    pub fn new(
        config_server: Arc<ConfigServer>,
        status_store: Arc<dyn StatusStore>,
        gateway_class_name: String,
    ) -> Self {
        Self {
            config_server,
            status_store,
            gateway_class_name,
        }
    }

    pub async fn run(&self) {
        let mut interval = interval(Duration::from_secs(RECONCILE_INTERVAL));

        loop {
            interval.tick().await;
            self.reconcile_gateways().await;
            self.reconcile_http_routes().await;
        }
    }

    async fn reconcile_gateways(&self) {
        use crate::types::resources::gateway::{GatewayStatus, GatewayStatusAddress, ListenerStatus};
        use chrono::Utc;

        let gateways = self.config_server.gateways.list_owned();

        for gateway in gateways.data {
            // Check if gateway class matches
            if gateway.spec.gateway_class_name != self.gateway_class_name {
                continue;
            }

            let mut status = GatewayStatus {
                addresses: Some(vec![GatewayStatusAddress {
                    address_type: Some("IPAddress".to_string()),
                    value: "0.0.0.0".to_string(), // Placeholder: in real world this should watch LoadBalancer Service
                }]),
                conditions: Some(vec![
                    Condition {
                        type_: "Programmed".to_string(),
                        status: "True".to_string(),
                        reason: "Programmed".to_string(),
                        message: "Gateway programmed by Edgion controller".to_string(),
                        last_transition_time: Utc::now().to_rfc3339(),
                        observed_generation: gateway.metadata.generation,
                    },
                    Condition {
                        type_: "Accepted".to_string(),
                        status: "True".to_string(),
                        reason: "Accepted".to_string(),
                        message: "Gateway accepted by Edgion controller".to_string(),
                        last_transition_time: Utc::now().to_rfc3339(),
                        observed_generation: gateway.metadata.generation,
                    },
                ]),
                listeners: Some(vec![]),
            };

            if let Some(listeners) = &gateway.spec.listeners {
                let mut listener_statuses = Vec::new();
                for listener in listeners {
                    listener_statuses.push(ListenerStatus {
                        name: listener.name.clone(),
                        supported_kinds: vec![], // TODO: populate supported kinds based on protocols
                        attached_routes: 0,      // TODO: calculate attached routes
                        conditions: vec![
                            Condition {
                                type_: "Accepted".to_string(),
                                status: "True".to_string(),
                                reason: "Accepted".to_string(),
                                message: "Listener accepted".to_string(),
                                last_transition_time: Utc::now().to_rfc3339(),
                                observed_generation: gateway.metadata.generation,
                            },
                            Condition {
                                type_: "Programmed".to_string(),
                                status: "True".to_string(),
                                reason: "Programmed".to_string(),
                                message: "Listener programmed".to_string(),
                                last_transition_time: Utc::now().to_rfc3339(),
                                observed_generation: gateway.metadata.generation,
                            },
                        ],
                    });
                }
                status.listeners = Some(listener_statuses);
            }

            // Only update if status implies it's not already set or changed (simple check for now)
            // Ideally we check if existing status is equivalent to reduce churn.
            // For now, we update periodically which is inefficient but fits "slow implementation".
            // Optimization: check generation in existing status.

            if let Err(e) = self
                .status_store
                .update_gateway_status(
                    gateway.namespace().as_deref().unwrap_or("default"),
                    gateway.name_any().as_str(),
                    status,
                )
                .await
            {
                tracing::error!("Failed to update gateway status: {}", e);
            }
        }
    }

    async fn reconcile_http_routes(&self) {
        use crate::types::resources::http_route::{HTTPRouteStatus, RouteParentStatus};
        use chrono::Utc;

        let routes = self.config_server.routes.list_owned();

        for route in routes.data {
            let mut parents_status = Vec::new();

            // Iterate over parent refs to find those that match our gateways
            if let Some(parent_refs) = &route.spec.parent_refs {
                for parent_ref in parent_refs {
                    // Logic to check if this parentRef points to a Gateway we manage.
                    // Simplified: We assume if the parent is a Gateway in the same namespace (or allowed by ReferenceGrant)
                    // and that Gateway is managed by us, then we should report status.

                    // For now, simpler: blindly report status for all parentRefs assuming they are waiting for us,
                    // IF the parent kind is Gateway.

                    let kind = parent_ref.kind.as_deref().unwrap_or("Gateway");
                    let group = parent_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");

                    if kind == "Gateway" && group == "gateway.networking.k8s.io" {
                        // In a real implementation we would look up the Gateway and check its controller.
                        // Here we assume we are the controller.

                        parents_status.push(RouteParentStatus {
                            parent_ref: parent_ref.clone(),
                            controller_name: "edgion.io/gateway-controller".to_string(),
                            conditions: vec![
                                Condition {
                                    type_: "Accepted".to_string(),
                                    status: "True".to_string(),
                                    reason: "Accepted".to_string(),
                                    message: "Route accepted".to_string(),
                                    last_transition_time: Utc::now().to_rfc3339(),
                                    observed_generation: route.metadata.generation,
                                },
                                Condition {
                                    type_: "ResolvedRefs".to_string(),
                                    status: "True".to_string(),
                                    reason: "ResolvedRefs".to_string(),
                                    message: "All references resolved".to_string(),
                                    last_transition_time: Utc::now().to_rfc3339(),
                                    observed_generation: route.metadata.generation,
                                },
                            ],
                        });
                    }
                }
            }

            if !parents_status.is_empty() {
                let status = HTTPRouteStatus {
                    parents: parents_status,
                };

                if let Err(e) = self
                    .status_store
                    .update_http_route_status(
                        route.namespace().as_deref().unwrap_or("default"),
                        route.name_any().as_str(),
                        status,
                    )
                    .await
                {
                    tracing::error!("Failed to update http route status: {}", e);
                }
            }
        }
    }
}
