use super::*;
use crate::resources::http_route::HTTPRouteExternalAuth;
use crate::resources::link_sys::webhook::WebhookServiceConfig;
use serde_json::json;

#[test]
fn shared_custom_service_validation_reaches_webhook_and_internal_edgion_auth() {
    for group in [None, Some(""), Some("core"), Some("other.io"), Some("CORE")] {
        for namespace in [None, Some("shared"), Some("")] {
            let mut target = json!({"name":"auth","port":8080});
            if let Some(group) = group {
                target["group"] = json!(group);
            }
            if let Some(namespace) = namespace {
                target["namespace"] = json!(namespace);
            }
            let input = json!({"target":target,"decision":{}});
            let webhook: WebhookServiceConfig = serde_json::from_value(input.clone()).unwrap();
            let route: HTTPRouteExternalAuth = serde_json::from_value(input).unwrap();
            assert!(matches!(route, HTTPRouteExternalAuth::Edgion(_)));
            let valid = group.is_none_or(|group| group.is_empty() || group == "core") && namespace != Some("");
            assert_eq!(webhook.get_validation_error().is_none(), valid);
            assert_eq!(route.validation_error().is_none(), valid);
        }
    }
    for input in [
        json!({"backendRef":{"name":"auth","port":8080},"protocol":"HTTP"}),
        json!({"backendRef":{"name":"auth","port":8080},"target":{"name":"auth","port":8080}}),
    ] {
        let native: HTTPRouteExternalAuth = serde_json::from_value(input).unwrap();
        assert!(matches!(native, HTTPRouteExternalAuth::Upstream(_)));
        assert!(native.validation_error().is_some());
        assert!(native.edgion_config().is_none());
    }
}

#[test]
fn custom_target_url_exclusion_and_secure_scheme_validation_remain_separate() {
    for target in [
        json!({"url":"https://example.com","name":"auth","port":8080}),
        json!({"url":"https://example.com","group":"core"}),
    ] {
        let target: WebhookTarget = serde_json::from_value(target).unwrap();
        assert!(target.kind().is_err());
    }
    for (url, enabled, valid) in [
        ("http://example.com", true, false),
        ("https://example.com", false, true),
        ("https://example.com", true, true),
    ] {
        let conn: ExternalHttpConnection =
            serde_json::from_value(json!({"target":{"url":url},"tls":{"enabled":enabled}})).unwrap();
        assert_eq!(validate_connection_target(&conn).is_none(), valid);
    }
}
