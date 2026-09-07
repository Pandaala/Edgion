//! Shared validation for authentication challenge realms.

/// Validate the unescaped realm before configuration is used or a challenge is built.
///
/// Quotes and backslashes are valid data and are escaped by the gateway's canonical
/// challenge writer. ASCII controls (including HTAB) and DEL are rejected rather
/// than stripped. Other UTF-8 bytes are preserved as HTTP quoted-string obs-text.
/// Existing plugin-specific empty-realm checks remain the caller's responsibility.
pub fn validate_auth_realm(realm: &str) -> Result<(), &'static str> {
    if realm.bytes().any(|byte| byte.is_ascii_control()) {
        return Err("realm must not contain ASCII control characters (including tabs and DEL)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_auth_realm;
    use crate::resources::edgion_plugins::{
        BasicAuthConfig, EdgionPlugin, HmacAuthConfig, JwtAuthConfig, KeyAuthConfig, LdapAuthConfig,
        OpenidConnectConfig,
    };

    fn realm_plugins(realm: &str) -> [EdgionPlugin; 6] {
        [
            EdgionPlugin::BasicAuth(BasicAuthConfig {
                realm: realm.into(),
                anonymous: Some("guest".into()),
                ..Default::default()
            }),
            EdgionPlugin::KeyAuth(KeyAuthConfig {
                realm: realm.into(),
                anonymous: Some("guest".into()),
                ..Default::default()
            }),
            EdgionPlugin::JwtAuth(JwtAuthConfig {
                realm: realm.into(),
                anonymous: Some("guest".into()),
                ..Default::default()
            }),
            EdgionPlugin::HmacAuth(HmacAuthConfig {
                realm: realm.into(),
                anonymous: Some("guest".into()),
                ..Default::default()
            }),
            EdgionPlugin::LdapAuth(LdapAuthConfig {
                realm: realm.into(),
                ldap_host: "ldap.example.com".into(),
                base_dn: "dc=example,dc=com".into(),
                attribute: "uid".into(),
                ..Default::default()
            }),
            EdgionPlugin::OpenidConnect(Box::new(OpenidConnectConfig {
                realm: realm.into(),
                discovery: "https://idp.example.com/.well-known/openid-configuration".into(),
                client_id: "test-client".into(),
                bearer_only: true,
                ..Default::default()
            })),
        ]
    }

    #[test]
    fn auth_realm_all_configs_accept_literal_quotes_backslashes_and_utf8() {
        for realm in ["API Gateway", "Partner \"API\"", "path\\", "a, b=c", "café €"] {
            assert_eq!(validate_auth_realm(realm), Ok(()));
            for plugin in realm_plugins(realm) {
                // Exercise the wire shape and enum dispatch used after conf_sync,
                // including HmacAuth's custom flat serialization.
                let wire = serde_json::to_value(&plugin).unwrap();
                assert_eq!(wire["config"]["realm"], realm);
                let decoded: EdgionPlugin = serde_json::from_value(wire).unwrap();
                assert_eq!(decoded.validation_error(), None, "{}", decoded.type_name());
            }
        }
    }

    #[test]
    fn auth_realm_all_configs_reject_every_ascii_control_without_echoing_the_value() {
        for byte in (0..=0x1f).chain(std::iter::once(0x7f)) {
            let realm = format!("private-realm{}suffix", char::from(byte));
            let expected = validate_auth_realm(&realm).unwrap_err();
            assert!(!expected.contains("private-realm"));
            for plugin in realm_plugins(&realm) {
                assert_eq!(
                    plugin.validation_error().as_deref(),
                    Some(expected),
                    "{}",
                    plugin.type_name()
                );
            }
        }
    }

    #[test]
    fn auth_realm_preserves_plugin_specific_empty_value_rules() {
        assert_eq!(validate_auth_realm(""), Ok(()));
        for plugin in realm_plugins("") {
            let accepts_empty = matches!(&plugin, EdgionPlugin::JwtAuth(_) | EdgionPlugin::LdapAuth(_));
            assert_eq!(
                plugin.validation_error().is_none(),
                accepts_empty,
                "{}",
                plugin.type_name()
            );
        }
    }
}
