use crate::types::prelude_resources::*;

fn sample_gateway_class(name: &str, version: u64) -> GatewayClass {
    let mut gc = GatewayClass::new(
        name,
        GatewayClassSpec {
            controller_name: "edgion.dev/controller".to_string(),
            description: None,
            parameters_ref: None,
        },
    );
    gc.metadata.resource_version = Some(version.to_string());
    gc
}

// All watch-related tests have been removed as they use deprecated APIs.
// GatewayClass/EdgionGatewayConfig/Gateway are now managed via base_conf
// and should be accessed via apply_base_conf() and base_conf.read().unwrap()
// instead of apply_resource_change() and list/watch APIs.
