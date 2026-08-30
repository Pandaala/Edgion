//! Registry authentication: dockerconfigjson credential selection and
//! `WWW-Authenticate: Bearer` challenge parsing.

use base64::Engine as _;
use serde::Deserialize;

/// Basic credentials selected from a dockerconfigjson for one registry host.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistryAuth {
    pub username: String,
    pub password: String,
}

// Manual impl: the password must never appear in Debug output (a future
// `{:?}` on a session or auth value must not become a credential leak).
impl std::fmt::Debug for RegistryAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryAuth")
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

#[derive(Deserialize)]
struct DockerConfigJson {
    #[serde(default)]
    auths: std::collections::HashMap<String, DockerAuthEntry>,
}

#[derive(Deserialize)]
struct DockerAuthEntry {
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

/// Normalize a dockerconfigjson `auths` key to `host[:port]`: strip an
/// optional scheme, then anything from the first `/` on (covers the legacy
/// `https://index.docker.io/v1/` form).
fn normalize_auth_key(key: &str) -> String {
    let key = key
        .strip_prefix("https://")
        .or_else(|| key.strip_prefix("http://"))
        .unwrap_or(key);
    key.split('/').next().unwrap_or("").to_string()
}

/// Pick the credentials for `registry` (`host[:port]`) out of a raw
/// dockerconfigjson document. `Ok(None)` = no entry for this registry
/// (anonymous pull); `Err` = the document itself is malformed.
pub fn select_registry_auth(dockerconfigjson: &str, registry: &str) -> Result<Option<RegistryAuth>, String> {
    let parsed: DockerConfigJson =
        serde_json::from_str(dockerconfigjson).map_err(|e| format!("invalid dockerconfigjson: {e}"))?;
    // docker.io aliases: the API host is registry-1.docker.io, but config
    // entries traditionally key on docker.io / index.docker.io.
    let mut candidates: Vec<&str> = vec![registry];
    if registry == "registry-1.docker.io" {
        candidates.extend(["docker.io", "index.docker.io"]);
    }
    for (key, entry) in &parsed.auths {
        if !candidates.contains(&normalize_auth_key(key).as_str()) {
            continue;
        }
        if let Some(b64) = &entry.auth {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| format!("dockerconfigjson auth for '{key}' is not valid base64: {e}"))?;
            let text =
                String::from_utf8(decoded).map_err(|_| format!("dockerconfigjson auth for '{key}' is not UTF-8"))?;
            let (user, pass) = text
                .split_once(':')
                .ok_or_else(|| format!("dockerconfigjson auth for '{key}' is not 'user:password'"))?;
            return Ok(Some(RegistryAuth {
                username: user.to_string(),
                password: pass.to_string(),
            }));
        }
        if let (Some(u), Some(p)) = (&entry.username, &entry.password) {
            return Ok(Some(RegistryAuth {
                username: u.clone(),
                password: p.clone(),
            }));
        }
        return Err(format!(
            "dockerconfigjson entry for '{key}' has neither auth nor username/password"
        ));
    }
    Ok(None)
}

/// A parsed `WWW-Authenticate: Bearer` challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerChallenge {
    pub realm: String,
    pub service: Option<String>,
    pub scope: Option<String>,
}

/// Parse a `WWW-Authenticate` header value. Returns `None` unless it is a
/// Bearer challenge carrying a realm.
pub fn parse_bearer_challenge(header: &str) -> Option<BearerChallenge> {
    let rest = header.trim().strip_prefix("Bearer ")?;
    let mut realm = None;
    let mut service = None;
    let mut scope = None;
    for part in rest.split(',') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').to_string();
        match k.trim() {
            "realm" => realm = Some(v),
            "service" => service = Some(v),
            "scope" => scope = Some(v),
            _ => {}
        }
    }
    Some(BearerChallenge {
        realm: realm?,
        service,
        scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn selects_by_exact_host_and_legacy_url_key() {
        let json = format!(
            r#"{{"auths":{{"registry.internal:5000":{{"auth":"{}"}},"https://index.docker.io/v1/":{{"auth":"{}"}}}}}}"#,
            b64("alice:s3cret"),
            b64("bob:hub")
        );
        let a = select_registry_auth(&json, "registry.internal:5000").unwrap().unwrap();
        assert_eq!((a.username.as_str(), a.password.as_str()), ("alice", "s3cret"));
        // docker.io API host matches the legacy key.
        let b = select_registry_auth(&json, "registry-1.docker.io").unwrap().unwrap();
        assert_eq!(b.username, "bob");
        // Unknown registry: anonymous, not an error.
        assert!(select_registry_auth(&json, "other.example.com").unwrap().is_none());
    }

    #[test]
    fn username_password_fields_work_and_malformed_errors() {
        let json = r#"{"auths":{"r.io":{"username":"u","password":"p"}}}"#;
        let a = select_registry_auth(json, "r.io").unwrap().unwrap();
        assert_eq!((a.username.as_str(), a.password.as_str()), ("u", "p"));
        assert!(select_registry_auth("not-json", "r.io").is_err());
        let no_colon = format!(r#"{{"auths":{{"r.io":{{"auth":"{}"}}}}}}"#, b64("nocolon"));
        assert!(select_registry_auth(&no_colon, "r.io").is_err());
    }

    #[test]
    fn parses_bearer_challenge() {
        let c = parse_bearer_challenge(
            r#"Bearer realm="https://auth.example.com/token",service="registry.example.com",scope="repository:a/b:pull""#,
        )
        .unwrap();
        assert_eq!(c.realm, "https://auth.example.com/token");
        assert_eq!(c.service.as_deref(), Some("registry.example.com"));
        assert_eq!(c.scope.as_deref(), Some("repository:a/b:pull"));
        assert!(parse_bearer_challenge("Basic realm=\"x\"").is_none());
        assert!(parse_bearer_challenge("Bearer service=\"no-realm\"").is_none());
    }

    #[test]
    fn debug_output_redacts_password() {
        let auth = RegistryAuth {
            username: "alice".to_string(),
            password: "s3cret".to_string(),
        };
        let dbg = format!("{auth:?}");
        assert!(!dbg.contains("s3cret"), "password must not appear in Debug: {dbg}");
        assert!(dbg.contains("[redacted]"));
    }
}
