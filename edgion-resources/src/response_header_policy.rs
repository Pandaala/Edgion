//! Policy for ordinary configured and extension-produced response mutations.
//!
//! Body and transport owners adjust framing through their private response path.
//! This policy does not restrict request headers or validate header syntax.

use crate::gwapi_types::is_http_framing_or_hop_by_hop_header_name;

/// Whether an ordinary response mutation would change a protocol-owned field.
pub fn is_protected_response_header(name: &str) -> bool {
    is_http_framing_or_hop_by_hop_header_name(name)
        || name.eq_ignore_ascii_case("proxy-connection")
        || name.starts_with(':')
}

/// Apply the same policy to writes, removals, and both ends of a rename.
pub fn validate_response_header_mutation(name: &str) -> Result<(), &'static str> {
    if is_protected_response_header(name) {
        Err("response header mutation targets a protected protocol field")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gwapi_types::HTTP_HOP_BY_HOP_HEADER_NAMES;
    use crate::resources::edgion_plugins::EdgionPlugin;
    use serde_json::json;

    #[test]
    fn response_header_policy_config_consumers_reject_all_protected_fields() {
        for name in
            HTTP_HOP_BY_HOP_HEADER_NAMES
                .iter()
                .copied()
                .chain(["Content-Length", "Proxy-Connection", ":status"])
        {
            let name = name.to_ascii_uppercase();
            let mut configs = vec![
                json!({"type":"Mock", "config":{"headers":{&name:"value"}}}),
                json!({"type":"FaultInjection", "config":{"abort":{"statusCode":503,"headers":[{"name":name,"value":"value"}]}}}),
                json!({"type":"ForwardAuth", "config":{"target":{"name":"auth","port":8080},"decision":{"clientHeaders":[name]}}}),
                json!({"type":"RegionRoute", "config":{"hashCalc":{"writeBack":[{"type":"responseHeader","name":name}]}}}),
            ];
            for action in ["set", "add", "remove"] {
                let values = if action == "remove" {
                    json!([name])
                } else {
                    json!([{"name":name,"value":"value"}])
                };
                configs.push(json!({"type":"ResponseHeaderModifier","config":{action:values}}));
                configs.push(json!({"type":"ResponseRewrite","config":{"headers":{action:values}}}));
            }
            for (from, to) in [(&*name, "x-ordinary"), ("x-ordinary", &*name)] {
                configs.push(json!({"type":"ResponseRewrite","config":{"headers":{"rename":[{"from":from,"to":to}]}}}));
            }
            for config in configs {
                let plugin: EdgionPlugin = serde_json::from_value(config.clone()).unwrap();
                assert!(plugin.validation_error().is_some(), "accepted {config}");
            }
        }
    }

    #[test]
    fn response_header_policy_config_consumers_preserve_ordinary_headers() {
        for name in ["Content-Encoding", "Set-Cookie", "WWW-Authenticate", "X-Business"] {
            for config in [
                json!({"type":"Mock", "config":{"headers":{name:"value"}}}),
                json!({"type":"FaultInjection", "config":{"abort":{"statusCode":503,"headers":[{"name":name,"value":"value"}]}}}),
                json!({"type":"ForwardAuth", "config":{"target":{"name":"auth","port":8080},"decision":{"clientHeaders":[name]}}}),
                json!({"type":"RegionRoute", "config":{"hashCalc":{"writeBack":[{"type":"responseHeader","name":name}]}}}),
                json!({"type":"ResponseHeaderModifier","config":{"set":[{"name":name,"value":"value"}]}}),
                json!({"type":"ResponseRewrite","config":{"headers":{"rename":[{"from":name,"to":"x-ordinary"}]}}}),
            ] {
                let plugin: EdgionPlugin = serde_json::from_value(config.clone()).unwrap();
                assert_eq!(plugin.validation_error(), None, "rejected {config}");
            }
        }
    }

    #[test]
    fn request_id_destination_policy_covers_every_protected_response_field() {
        use crate::resources::edgion_plugins::valid_request_id_header;

        for name in crate::gwapi_types::HTTP_HOP_BY_HOP_HEADER_NAMES.iter().copied().chain([
            "Content-Length",
            "Proxy-Connection",
            ":status",
        ]) {
            assert!(!valid_request_id_header(&name.to_ascii_uppercase()), "{name}");
        }
        assert!(valid_request_id_header("X-Request-Id"));
    }

    #[test]
    fn response_header_policy_preserves_body_metadata_and_business_headers() {
        for name in [
            "Content-Type",
            "Content-Encoding",
            "Set-Cookie",
            "Location",
            "Retry-After",
            "grpc-status",
        ] {
            assert_eq!(validate_response_header_mutation(name), Ok(()));
        }
        for name in crate::gwapi_types::HTTP_HOP_BY_HOP_HEADER_NAMES.iter().copied().chain([
            "Content-Length",
            "Proxy-Connection",
            ":status",
        ]) {
            assert!(validate_response_header_mutation(name).is_err());
            assert_eq!(
                validate_response_header_mutation(name),
                validate_response_header_mutation(&name.to_ascii_uppercase())
            );
        }
    }
}
