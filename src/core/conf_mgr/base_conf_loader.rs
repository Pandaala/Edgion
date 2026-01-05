use crate::core::conf_mgr::ConfStore;
use crate::types::{EdgionGatewayConfig, Gateway, GatewayBaseConf, GatewayClass, ResourceKind};
use anyhow::{anyhow, Context, Result};
use kube::ResourceExt;
use std::sync::Arc;

/// Load base configuration (GatewayClass, EdgionGatewayConfig, Gateway) from store
pub async fn load_base_conf_from_store(store: Arc<dyn ConfStore>, gateway_class_name: &str) -> Result<GatewayBaseConf> {
    tracing::info!(
        component = "conf_mgr",
        event = "load_base_start",
        gateway_class_name = gateway_class_name,
        "Loading base configuration from store"
    );

    // Step 1: List all resources from store
    let all_resources = store
        .list_all()
        .await
        .context("Failed to list all resources from store")?;

    // Step 2: Collect base configuration resources
    let mut gateway_classes: Vec<GatewayClass> = Vec::new();
    let mut edgion_gateway_configs: Vec<EdgionGatewayConfig> = Vec::new();
    let mut gateways: Vec<Gateway> = Vec::new();

    for resource in all_resources {
        let resource_kind = ResourceKind::from_content(&resource.content);

        match resource_kind {
            Some(ResourceKind::GatewayClass) => match serde_yaml::from_str::<GatewayClass>(&resource.content) {
                Ok(gc) => {
                    tracing::debug!(
                        component = "conf_mgr",
                        name = ?gc.name_any(),
                        "Found GatewayClass"
                    );
                    gateway_classes.push(gc);
                }
                Err(e) => {
                    tracing::warn!(
                        component = "conf_mgr",
                        name = resource.name,
                        error = %e,
                        "Failed to parse GatewayClass"
                    );
                }
            },
            Some(ResourceKind::EdgionGatewayConfig) => {
                match serde_yaml::from_str::<EdgionGatewayConfig>(&resource.content) {
                    Ok(egwc) => {
                        tracing::debug!(
                            component = "conf_mgr",
                            name = ?egwc.name_any(),
                            "Found EdgionGatewayConfig"
                        );
                        edgion_gateway_configs.push(egwc);
                    }
                    Err(e) => {
                        tracing::warn!(
                            component = "conf_mgr",
                            name = resource.name,
                            error = %e,
                            "Failed to parse EdgionGatewayConfig"
                        );
                    }
                }
            }
            Some(ResourceKind::Gateway) => match serde_yaml::from_str::<Gateway>(&resource.content) {
                Ok(gw) => {
                    tracing::debug!(
                        component = "conf_mgr",
                        name = ?gw.name_any(),
                        namespace = ?gw.namespace(),
                        gateway_class_name = %gw.spec.gateway_class_name,
                        "Found Gateway"
                    );
                    gateways.push(gw);
                }
                Err(e) => {
                    tracing::warn!(
                        component = "conf_mgr",
                        name = resource.name,
                        error = %e,
                        "Failed to parse Gateway"
                    );
                }
            },
            _ => {
                // Skip other resource types
            }
        }
    }

    tracing::info!(
        component = "conf_mgr",
        gateway_classes = gateway_classes.len(),
        edgion_gateway_configs = edgion_gateway_configs.len(),
        gateways = gateways.len(),
        "Collected base configuration resources"
    );

    // Log all collected Gateways for debugging
    if !gateways.is_empty() {
        tracing::info!(component = "conf_mgr", "All collected Gateways:");
        for gw in &gateways {
            tracing::info!(
                component = "conf_mgr",
                gateway_name = ?gw.name_any(),
                namespace = ?gw.namespace(),
                gateway_class_name = %gw.spec.gateway_class_name,
                "Gateway details"
            );
        }
    } else {
        tracing::warn!(component = "conf_mgr", "No Gateways were collected from store");
    }

    // Step 3: Find the matching GatewayClass by name
    let gateway_class = gateway_classes
        .into_iter()
        .find(|gc| gc.name_any() == gateway_class_name)
        .ok_or_else(|| anyhow!("GatewayClass '{}' not found in store", gateway_class_name))?;

    tracing::info!(
        component = "conf_mgr",
        name = ?gateway_class.name_any(),
        "Found matching GatewayClass"
    );

    // Step 4: Extract EdgionGatewayConfig name from GatewayClass parameters_ref
    let egwc_name = gateway_class
        .spec
        .parameters_ref
        .as_ref()
        .and_then(|params_ref| {
            if params_ref.kind == "EdgionGatewayConfig" {
                Some(params_ref.name.clone())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            anyhow!(
                "GatewayClass '{}' does not reference an EdgionGatewayConfig via parameters_ref",
                gateway_class_name
            )
        })?;

    tracing::info!(
        component = "conf_mgr",
        egwc_name = %egwc_name,
        "Found EdgionGatewayConfig reference from GatewayClass"
    );

    // Step 5: Find the matching EdgionGatewayConfig
    let edgion_gateway_config = edgion_gateway_configs
        .into_iter()
        .find(|egwc| egwc.name_any() == egwc_name)
        .ok_or_else(|| anyhow!("EdgionGatewayConfig '{}' not found in store", egwc_name))?;

    tracing::info!(
        component = "conf_mgr",
        name = ?edgion_gateway_config.name_any(),
        "Found matching EdgionGatewayConfig"
    );

    // Step 6: Filter Gateways that reference this GatewayClass
    let matching_gateways: Vec<Gateway> = gateways
        .into_iter()
        .filter(|gw| {
            let matches = gw.spec.gateway_class_name == gateway_class_name;
            if matches {
                tracing::debug!(
                    component = "conf_mgr",
                    gateway_name = ?gw.name_any(),
                    "Gateway matches GatewayClass '{}'",
                    gateway_class_name
                );
            }
            matches
        })
        .collect();

    if matching_gateways.is_empty() {
        return Err(anyhow!(
            "No Gateways found that reference GatewayClass '{}'",
            gateway_class_name
        ));
    }

    tracing::info!(
        component = "conf_mgr",
        count = matching_gateways.len(),
        "Found matching Gateways"
    );

    // Step 7: Construct GatewayBaseConf
    let base_conf = GatewayBaseConf::new(gateway_class, edgion_gateway_config, matching_gateways);

    tracing::info!(
        component = "conf_mgr",
        event = "load_base_complete",
        "Base configuration loaded successfully"
    );

    Ok(base_conf)
}
