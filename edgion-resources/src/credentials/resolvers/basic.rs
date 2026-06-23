use k8s_openapi::ByteString;
use std::collections::BTreeMap;

use super::super::{
    materials::BasicAuthMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

/// Shared resolver for Redis / Etcd / ES basic / Webhook basic auth.
///
/// Secret keys (camelCase, spec §5):
/// - `username` — optional; when absent, the client connects with the default ACL
/// - `password` — required
///
/// Accepted Secret types: `kubernetes.io/basic-auth` and `Opaque` (missing
/// type is also allowed and treated as `Opaque`).
#[derive(Debug, Default, Clone, Copy)]
pub struct BasicAuthResolver;

impl SecretMaterialResolver for BasicAuthResolver {
    type Material = BasicAuthMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &["password"]
    }

    fn allowed_secret_types(&self) -> &'static [&'static str] {
        &["kubernetes.io/basic-auth", "Opaque", ""]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let password_raw = data
            .get("password")
            .ok_or_else(|| ResolveError::MissingKeys(vec!["password"]))?;
        let password = std::str::from_utf8(&password_raw.0)
            .map_err(|e| ResolveError::DecodeError {
                key: "password",
                source: anyhow::anyhow!(e),
            })?
            .to_string();

        let username = match data.get("username") {
            Some(raw) => Some(
                std::str::from_utf8(&raw.0)
                    .map_err(|e| ResolveError::DecodeError {
                        key: "username",
                        source: anyhow::anyhow!(e),
                    })?
                    .to_string(),
            ),
            None => None,
        };

        Ok(BasicAuthMaterial { username, password })
    }
}

#[cfg(test)]
mod tests {
    use super::BasicAuthResolver;
    use crate::credentials::{ResolveError, SecretMaterialResolver};
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn mk(data: &[(&str, &[u8])]) -> BTreeMap<String, ByteString> {
        data.iter()
            .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
            .collect()
    }

    #[test]
    fn parse_full() {
        let data = mk(&[("username", b"alice"), ("password", b"secret")]);
        let m = BasicAuthResolver.parse(&data).unwrap();
        assert_eq!(m.username.as_deref(), Some("alice"));
        assert_eq!(m.password, "secret");
    }

    #[test]
    fn parse_password_only() {
        let data = mk(&[("password", b"secret")]);
        let m = BasicAuthResolver.parse(&data).unwrap();
        assert_eq!(m.username, None);
        assert_eq!(m.password, "secret");
    }

    #[test]
    fn parse_missing_password() {
        let data = mk(&[("username", b"alice")]);
        match BasicAuthResolver.parse(&data) {
            Err(ResolveError::MissingKeys(k)) => assert_eq!(k, vec!["password"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_extra_keys_ignored() {
        let data = mk(&[("password", b"p"), ("unrelated", b"x")]);
        let m = BasicAuthResolver.parse(&data).unwrap();
        assert_eq!(m.password, "p");
    }

    #[test]
    fn parse_invalid_utf8_password() {
        let data = mk(&[("password", &[0xff, 0xfe][..])]);
        match BasicAuthResolver.parse(&data) {
            Err(ResolveError::DecodeError { key: "password", .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn required_keys_order() {
        assert_eq!(BasicAuthResolver.required_keys(), &["password"]);
    }

    #[test]
    fn allows_basic_auth_and_opaque_types() {
        let allowed = BasicAuthResolver.allowed_secret_types();
        assert!(allowed.contains(&"kubernetes.io/basic-auth"));
        assert!(allowed.contains(&"Opaque"));
        assert!(allowed.contains(&""));
    }
}
