use super::{validate_anonymous_header_text, validate_auth_realm, INVALID_ANONYMOUS_HEADER_TEXT};
use crate::resources::edgion_plugins::{
    BasicAuthConfig, EdgionPlugin, HmacAuthConfig, JwtAuthConfig, KeyAuthConfig, LdapAuthConfig, OpenidConnectConfig,
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

#[test]
fn anonymous_header_text_accepts_header_values_only() {
    assert_eq!(validate_anonymous_header_text(None), Ok(()));
    for valid in ["guest", "", "caf\u{e9}"] {
        assert_eq!(validate_anonymous_header_text(Some(valid)), Ok(()), "{valid:?}");
    }
    for invalid in ["a\u{1}b", "a\u{7f}b", "a\rb", "a\nb", "a\0b"] {
        assert_eq!(
            validate_anonymous_header_text(Some(invalid)),
            Err(INVALID_ANONYMOUS_HEADER_TEXT),
            "{invalid:?}"
        );
    }
}
