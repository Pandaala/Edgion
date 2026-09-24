use super::*;

#[test]
fn default_config_is_valid() {
    assert!(PluginValidation::validation_error(&DirectEndpointConfig::default()).is_none());
}
