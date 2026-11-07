use super::*;
use crate::resources::edgion_stream_plugins::EdgionStreamPlugin;

#[test]
fn accepts_bounds_and_rejects_out_of_range_values() {
    for max_connections in [1, 2, MAX_CONNECTION_LIMIT] {
        assert_eq!(ConnectionLimitConfig { max_connections }.validation_error(), None);
    }
    for max_connections in [0, MAX_CONNECTION_LIMIT + 1] {
        let error = ConnectionLimitConfig { max_connections }.validation_error().unwrap();
        assert!(
            error.contains("'maxConnections' must be between 1 and 10000000"),
            "{error}"
        );
    }
}

#[test]
fn deserializes_as_stage_one_variant_with_camel_case_config() {
    let plugin: EdgionStreamPlugin =
        yaml_serde::from_str("type: ConnectionLimit\nconfig:\n  maxConnections: 25\n").unwrap();
    let EdgionStreamPlugin::ConnectionLimit(config) = &plugin else {
        panic!("expected ConnectionLimit, got {}", plugin.type_name());
    };
    assert_eq!(config.max_connections, 25);
    assert_eq!(plugin.type_name(), "ConnectionLimit");
}

#[test]
fn missing_max_connections_does_not_deserialize() {
    assert!(yaml_serde::from_str::<ConnectionLimitConfig>("{}").is_err());
}
