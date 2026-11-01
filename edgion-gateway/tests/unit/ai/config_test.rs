use super::*;
#[test]
fn new_configuration_subtrees_reject_unknown_fields() {
    for yaml in [
        "conversion: {typo: 1}",
        "conversion: {chat_to_anthropic: {typo: 1}}",
        "conversion: {chat_to_anthropic: {unary: {typo: 1}}}",
        "typo: 1",
    ] {
        assert!(yaml_serde::from_str::<AiConfig>(yaml).is_err(), "{yaml}");
    }
    let config: AiConfig =
        yaml_serde::from_str("conversion: {chat_to_anthropic: {unary: {hold_timeout: 500ms}}}").unwrap();
    assert_eq!(
        UnaryLimits::resolve(&config.conversion.chat_to_anthropic.unary)
            .unwrap()
            .timeout,
        std::time::Duration::from_millis(500)
    );
}
