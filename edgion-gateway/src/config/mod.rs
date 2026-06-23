pub mod edgion_gateway;
pub mod gateway_class;

pub use edgion_gateway::{
    count_edgion_gateway_configs, create_edgion_gateway_config_handler, get_edgion_gateway_config_by_name,
    list_edgion_gateway_configs,
};
pub use gateway_class::{
    count_gateway_classes, create_gateway_class_handler, get_gateway_class_by_name, list_gateway_classes,
};
