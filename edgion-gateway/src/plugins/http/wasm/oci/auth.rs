//! Registry authentication: dockerconfigjson credential selection and
//! `WWW-Authenticate: Bearer` challenge parsing.

use base64::Engine as _;
use serde::Deserialize;

/// Bound work and retained values derived from a remote authentication
/// challenge. Registry challenges are normally a few hundred bytes.
const MAX_BEARER_CHALLENGE_BYTES: usize = 16 * 1024;
const MAX_BEARER_CHALLENGE_PARAMS: usize = 32;

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
fn normalize_auth_key(key: &str) -> &str {
    let key = key
        .strip_prefix("https://")
        .or_else(|| key.strip_prefix("http://"))
        .unwrap_or(key);
    key.split('/').next().unwrap_or("")
}

/// Return an explicit selection rank for a matching dockerconfigjson key.
/// The API host wins over Docker Hub aliases, and a literal `host[:port]`
/// key wins over URL-shaped variants for the same host. The final key tie
/// break makes selection independent of `HashMap` iteration order.
fn auth_key_rank<'a>(key: &'a str, registry: &str) -> Option<(usize, bool, &'a str)> {
    let normalized = normalize_auth_key(key);
    let candidate_rank = if normalized == registry {
        0
    } else if registry == "registry-1.docker.io" && normalized == "docker.io" {
        1
    } else if registry == "registry-1.docker.io" && normalized == "index.docker.io" {
        2
    } else {
        return None;
    };
    Some((candidate_rank, key != normalized, key))
}

/// Pick the credentials for `registry` (`host[:port]`) out of a raw
/// dockerconfigjson document. `Ok(None)` = no entry for this registry
/// (anonymous pull); `Err` = the document itself is malformed.
pub fn select_registry_auth(dockerconfigjson: &str, registry: &str) -> Result<Option<RegistryAuth>, String> {
    let parsed: DockerConfigJson =
        serde_json::from_str(dockerconfigjson).map_err(|e| format!("invalid dockerconfigjson: {e}"))?;
    // A dockerconfigjson map has no meaningful iteration order. Select by an
    // explicit rank so an exact registry entry always wins over Docker Hub
    // aliases and URL-shaped legacy keys.
    let selected = parsed
        .auths
        .iter()
        .filter_map(|(key, entry)| auth_key_rank(key, registry).map(|rank| (rank, key, entry)))
        .min_by(|(left, _, _), (right, _, _)| left.cmp(right));
    if let Some((_, key, entry)) = selected {
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

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

fn skip_ows(bytes: &[u8], cursor: &mut usize) {
    while bytes.get(*cursor).is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        *cursor += 1;
    }
}

fn parse_token<'a>(input: &'a str, cursor: &mut usize) -> Option<&'a str> {
    let bytes = input.as_bytes();
    let start = *cursor;
    while bytes.get(*cursor).is_some_and(|byte| is_token_byte(*byte)) {
        *cursor += 1;
    }
    (*cursor > start).then(|| &input[start..*cursor])
}

fn is_quoted_text(character: char) -> bool {
    character == '\t' || character == ' ' || matches!(character, '!' | '#'..='[' | ']'..='~') || !character.is_ascii()
}

fn is_quoted_pair(character: char) -> bool {
    character == '\t' || matches!(character, ' '..='~') || !character.is_ascii()
}

fn parse_quoted_value(input: &str, cursor: &mut usize, capture: bool) -> Option<Option<String>> {
    debug_assert_eq!(input.as_bytes().get(*cursor), Some(&b'"'));
    *cursor += 1;
    let mut value = capture.then(String::new);
    loop {
        let character = input.get(*cursor..)?.chars().next()?;
        *cursor += character.len_utf8();
        match character {
            '"' => return Some(value),
            '\\' => {
                let escaped = input.get(*cursor..)?.chars().next()?;
                if !is_quoted_pair(escaped) {
                    return None;
                }
                *cursor += escaped.len_utf8();
                if let Some(value) = &mut value {
                    value.push(escaped);
                }
            }
            character if is_quoted_text(character) => {
                if let Some(value) = &mut value {
                    value.push(character);
                }
            }
            _ => return None,
        }
    }
}

fn parse_parameter_value(input: &str, cursor: &mut usize, capture: bool) -> Option<Option<String>> {
    if input.as_bytes().get(*cursor) == Some(&b'"') {
        return parse_quoted_value(input, cursor, capture);
    }
    let token = parse_token(input, cursor)?;
    Some(capture.then(|| token.to_string()))
}

/// Parse one `WWW-Authenticate: Bearer` challenge. Returns `None` for another
/// scheme, malformed syntax, ambiguous duplicate parameters, an absent realm,
/// or an input above the fixed work bounds.
pub fn parse_bearer_challenge(header: &str) -> Option<BearerChallenge> {
    if header.is_empty() || header.len() > MAX_BEARER_CHALLENGE_BYTES {
        return None;
    }

    let bytes = header.as_bytes();
    let mut cursor = 0;
    skip_ows(bytes, &mut cursor);
    let scheme = parse_token(header, &mut cursor)?;
    if !scheme.eq_ignore_ascii_case("Bearer") || !bytes.get(cursor).is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        return None;
    }
    skip_ows(bytes, &mut cursor);
    if cursor == bytes.len() {
        return None;
    }

    let mut realm = None;
    let mut service = None;
    let mut scope = None;
    let mut parameter_count = 0;
    loop {
        parameter_count += 1;
        if parameter_count > MAX_BEARER_CHALLENGE_PARAMS {
            return None;
        }

        let name = parse_token(header, &mut cursor)?;
        skip_ows(bytes, &mut cursor);
        if bytes.get(cursor) != Some(&b'=') {
            return None;
        }
        cursor += 1;
        skip_ows(bytes, &mut cursor);

        let target = if name.eq_ignore_ascii_case("realm") {
            &mut realm
        } else if name.eq_ignore_ascii_case("service") {
            &mut service
        } else if name.eq_ignore_ascii_case("scope") {
            &mut scope
        } else {
            let _ = parse_parameter_value(header, &mut cursor, false)?;
            skip_ows(bytes, &mut cursor);
            if cursor == bytes.len() {
                break;
            }
            if bytes.get(cursor) != Some(&b',') {
                return None;
            }
            cursor += 1;
            skip_ows(bytes, &mut cursor);
            if cursor == bytes.len() {
                return None;
            }
            continue;
        };
        if target.is_some() {
            return None;
        }
        *target = parse_parameter_value(header, &mut cursor, true)?;

        skip_ows(bytes, &mut cursor);
        if cursor == bytes.len() {
            break;
        }
        if bytes.get(cursor) != Some(&b',') {
            return None;
        }
        cursor += 1;
        skip_ows(bytes, &mut cursor);
        if cursor == bytes.len() {
            return None;
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
    fn credential_selection_has_explicit_exact_and_docker_hub_precedence() {
        let cases = [
            r#"{"auths":{"https://r.io/v1/":{"username":"url","password":"url-pass"},"r.io":{"username":"exact","password":"exact-pass"}}}"#,
            r#"{"auths":{"r.io":{"username":"exact","password":"exact-pass"},"https://r.io/v1/":{"username":"url","password":"url-pass"}}}"#,
        ];
        for json in cases {
            let selected = select_registry_auth(json, "r.io").unwrap().unwrap();
            assert_eq!(
                (selected.username.as_str(), selected.password.as_str()),
                ("exact", "exact-pass")
            );
        }

        let docker = r#"{"auths":{"index.docker.io":{"username":"index","password":"index-pass"},"docker.io":{"username":"docker","password":"docker-pass"},"registry-1.docker.io":{"username":"api","password":"api-pass"}}}"#;
        let selected = select_registry_auth(docker, "registry-1.docker.io").unwrap().unwrap();
        assert_eq!(
            (selected.username.as_str(), selected.password.as_str()),
            ("api", "api-pass")
        );

        let aliases = r#"{"auths":{"index.docker.io":{"username":"index","password":"index-pass"},"docker.io":{"username":"docker","password":"docker-pass"}}}"#;
        let selected = select_registry_auth(aliases, "registry-1.docker.io").unwrap().unwrap();
        assert_eq!(
            (selected.username.as_str(), selected.password.as_str()),
            ("docker", "docker-pass")
        );

        let decorated_cases = [
            r#"{"auths":{"https://r.io/a":{"username":"https","password":"https-pass"},"http://r.io/z":{"username":"http","password":"http-pass"}}}"#,
            r#"{"auths":{"http://r.io/z":{"username":"http","password":"http-pass"},"https://r.io/a":{"username":"https","password":"https-pass"}}}"#,
        ];
        for json in decorated_cases {
            let selected = select_registry_auth(json, "r.io").unwrap().unwrap();
            assert_eq!(
                (selected.username.as_str(), selected.password.as_str()),
                ("http", "http-pass")
            );
        }
    }

    #[test]
    fn malformed_higher_priority_credentials_fail_closed() {
        let json = r#"{"auths":{"r.io":{"auth":"not-base64!"},"https://r.io/v1/":{"username":"fallback","password":"fallback-pass"}}}"#;
        assert!(select_registry_auth(json, "r.io").is_err());
    }

    #[test]
    fn username_password_fields_work_and_malformed_errors() {
        let json = r#"{"auths":{"r.io":{"username":"u","password":"p"}}}"#;
        let a = select_registry_auth(json, "r.io").unwrap().unwrap();
        assert_eq!((a.username.as_str(), a.password.as_str()), ("u", "p"));

        let auth_wins = format!(
            r#"{{"auths":{{"r.io":{{"auth":"{}","username":"fallback","password":"fallback-pass"}}}}}}"#,
            b64("encoded:encoded-pass")
        );
        let selected = select_registry_auth(&auth_wins, "r.io").unwrap().unwrap();
        assert_eq!(
            (selected.username.as_str(), selected.password.as_str()),
            ("encoded", "encoded-pass")
        );

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
    fn bearer_parser_handles_case_tokens_quoted_commas_and_pairs() {
        let challenge = parse_bearer_challenge(
            "  bEaReR\tReAlM = \"https://auth.example/token?a=one,two\\\"three\\\\four\", SERVICE=registry.example, Scope=\"repository:a/b:pull\", extension=\"x,y\"  ",
        )
        .unwrap();
        assert_eq!(challenge.realm, "https://auth.example/token?a=one,two\"three\\four");
        assert_eq!(challenge.service.as_deref(), Some("registry.example"));
        assert_eq!(challenge.scope.as_deref(), Some("repository:a/b:pull"));
    }

    #[test]
    fn bearer_parser_rejects_malformed_ambiguous_and_oversized_inputs() {
        for header in [
            r#"Bearer realm="unterminated"#,
            r#"Bearer realm="dangling\""#,
            r#"Bearer realm="https://auth", realm="https://other""#,
            r#"Bearer realm="https://auth", SERVICE=one, service=two"#,
            r#"Bearer realm="https://auth" trailing=value"#,
            r#"Bearer realm="https://auth","#,
            r#"Bearer realm="https://auth", bad"#,
        ] {
            assert!(
                parse_bearer_challenge(header).is_none(),
                "accepted malformed header: {header}"
            );
        }

        let too_many = format!(
            "Bearer realm=\"https://auth.example\",{}",
            (0..MAX_BEARER_CHALLENGE_PARAMS)
                .map(|index| format!("x{index}=value"))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(parse_bearer_challenge(&too_many).is_none());
        assert!(
            parse_bearer_challenge(&format!("Bearer realm=\"{}\"", "x".repeat(MAX_BEARER_CHALLENGE_BYTES))).is_none()
        );
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
