use super::*;

#[test]
fn every_outbound_surface_validates_secret_references() {
    for template in [
        "${secretRef}",
        "${secretRef:token:key}",
        "${secretRef:ns/token:key}",
        "${header:X-Id}",
    ] {
        let valid = matches!(template, "${secretRef:ns/token:key}" | "${header:X-Id}");
        for value in [
            serde_json::json!({"path": {"template": template}}),
            serde_json::json!({"method": {"template": template}}),
            serde_json::json!({"args": {"custom": [{"name": "token", "template": template}]}}),
            serde_json::json!({"headers": {"custom": [{"name": "X-Token", "template": template}]}}),
            serde_json::json!({"cookies": {"custom": [{"name": "token", "template": template}]}}),
            serde_json::json!({"body": {"type": "raw", "template": template}}),
            serde_json::json!({"body": {"type": "json", "template": format!("{{\"token\":\"{template}\"}}")}}),
            serde_json::json!({"body": {"type": "form", "fields": [{"name": "token", "template": template}]}}),
        ] {
            let spec: OutboundRequestSpec = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(spec.validation_error().is_none(), valid, "{value}");
        }
    }
}
