use super::*;

impl OpenidConnect {
    pub(super) fn trim_forwarded_header(value: &str) -> &str {
        value.split(',').next().unwrap_or("").trim()
    }

    pub(super) fn request_scheme(session: &dyn PluginSession) -> String {
        if let Some(v) = session.header_value("x-forwarded-proto") {
            let proto = Self::trim_forwarded_header(&v);
            if !proto.is_empty() {
                return proto.to_ascii_lowercase();
            }
        }
        "https".to_string()
    }

    pub(super) fn request_host(session: &dyn PluginSession) -> Option<String> {
        if let Some(v) = session.header_value("x-forwarded-host") {
            let host = Self::trim_forwarded_header(&v);
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }
        if let Some(v) = session.header_value("host") {
            let host = Self::trim_forwarded_header(&v);
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }
        None
    }

    pub(super) fn callback_path(&self) -> String {
        let configured = self
            .config
            .redirect_uri
            .as_deref()
            .filter(|v| !v.is_empty())
            .unwrap_or("/.edgion/oidc/callback");
        let raw_path = if configured.starts_with('/') {
            configured.to_string()
        } else if let Ok(url) = Url::parse(configured) {
            url.path().to_string()
        } else {
            "/.edgion/oidc/callback".to_string()
        };
        // Normalize the path so config-side and request-side inhabit the same
        // normalized space. See §3.5 of the URI normalization spec.
        let path_cfg = crate::core::gateway::cli::config::get_path_normalization_config();
        normalize_config_path(&raw_path, path_cfg)
    }

    pub(super) fn resolve_redirect_uri(&self, session: &dyn PluginSession) -> VerifyResult<String> {
        let configured = self.config.redirect_uri.as_deref().unwrap_or("/.edgion/oidc/callback");
        if configured.starts_with("https://") || configured.starts_with("http://") {
            return Ok(configured.to_string());
        }
        if !configured.starts_with('/') {
            return Err(OidcVerifyError::Other {
                status: 401,
                message: "Invalid redirectUri".to_string(),
            });
        }

        let host = Self::request_host(session).ok_or(OidcVerifyError::Other {
            status: 401,
            message: "Missing host header for redirect".to_string(),
        })?;
        let scheme = Self::request_scheme(session);
        Ok(format!("{}://{}{}", scheme, host, configured))
    }

    pub(super) fn is_reserved_authorization_param(key: &str) -> bool {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "response_type"
                | "client_id"
                | "redirect_uri"
                | "scope"
                | "state"
                | "nonce"
                | "code_challenge"
                | "code_challenge_method"
        )
    }

    pub(super) fn validate_required_scopes(&self, claims: &Value) -> VerifyResult<()> {
        if !self.config.bearer_only {
            return Ok(());
        }
        let Some(required_scopes) = self.config.required_scopes.as_ref() else {
            return Ok(());
        };
        if required_scopes.is_empty() {
            return Ok(());
        }

        let mut available = HashSet::new();
        if let Some(scope) = claims.get("scope").and_then(|v| v.as_str()) {
            for s in scope.split_whitespace() {
                available.insert(s.to_string());
            }
        }
        if let Some(scp) = claims.get("scp") {
            match scp {
                Value::String(s) => {
                    for item in s.split_whitespace() {
                        available.insert(item.to_string());
                    }
                }
                Value::Array(arr) => {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            available.insert(s.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        for required in required_scopes {
            if !available.contains(required) {
                return Err(OidcVerifyError::Other {
                    status: 403,
                    message: "Insufficient scope".to_string(),
                });
            }
        }

        Ok(())
    }

    pub(super) fn looks_like_jwt(token: &str) -> bool {
        decode_header(token).is_ok()
    }

    pub(super) fn now_unix_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    pub(super) fn claims_audience_matches(aud: Option<&Value>, allowed: &[String]) -> bool {
        if allowed.is_empty() {
            return true;
        }
        let Some(aud) = aud else {
            return false;
        };

        match aud {
            Value::String(v) => allowed.iter().any(|a| a == v),
            Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .any(|v| allowed.iter().any(|a| a == v)),
            _ => false,
        }
    }

    pub(super) fn validate_claims_from_value(&self, claims: &Value, default_issuer: Option<&str>) -> VerifyResult<()> {
        let now = Self::now_unix_secs();
        let leeway = self.config.clock_skew_seconds;

        if let Some(exp) = claims.get("exp").and_then(|v| v.as_u64()) {
            if now > exp.saturating_add(leeway) {
                return Err(OidcVerifyError::Expired);
            }
        }

        if let Some(nbf) = claims.get("nbf").and_then(|v| v.as_u64()) {
            if now.saturating_add(leeway) < nbf {
                return Err(OidcVerifyError::Other {
                    status: 401,
                    message: "Token not yet valid".to_string(),
                });
            }
        }

        let expected_issuers: Option<Vec<&str>> = if let Some(ref issuers) = self.config.issuers {
            if issuers.is_empty() {
                None
            } else {
                Some(issuers.iter().map(String::as_str).collect())
            }
        } else {
            default_issuer.map(|i| vec![i])
        };

        if let Some(issuers) = expected_issuers {
            let iss = claims.get("iss").and_then(|v| v.as_str());
            if !iss.is_some_and(|v| issuers.contains(&v)) {
                return Err(OidcVerifyError::Other {
                    status: 401,
                    message: "Invalid token issuer".to_string(),
                });
            }
        }

        if let Some(ref audiences) = self.config.audiences {
            if !audiences.is_empty() && !Self::claims_audience_matches(claims.get("aud"), audiences) {
                return Err(OidcVerifyError::Other {
                    status: 401,
                    message: "Invalid token audience".to_string(),
                });
            }
        }

        Ok(())
    }
}
