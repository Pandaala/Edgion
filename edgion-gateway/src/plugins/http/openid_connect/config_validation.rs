use super::*;
use crate::plugins::runtime::PluginSessionHeaderExt;

impl OpenidConnect {
    pub(super) fn trim_forwarded_header(value: &str) -> &str {
        value.split(',').next().unwrap_or("").trim()
    }

    /// Scheme used to expand a path-form `redirectUri` / `postLogoutRedirectUri`.
    ///
    /// Derived from the listener's real TLS state, never from `X-Forwarded-Proto`:
    /// nothing strips or validates that header on this path, so trusting it would
    /// let an anonymous request downgrade the callback URL advertised to the IdP.
    /// This mirrors the `request_host` decision to ignore `X-Forwarded-Host`.
    /// Deployments terminating TLS in front of the gateway must configure an
    /// absolute URL instead. `https` stays the fallback when the connection state
    /// is unavailable.
    pub(super) fn request_scheme(session: &dyn PluginSession) -> &'static str {
        match session.downstream_is_tls() {
            Some(false) => "http",
            _ => "https",
        }
    }

    pub(super) fn request_host(session: &dyn PluginSession) -> Option<&str> {
        if let Some(v) = session.header_str("host") {
            let host = Self::trim_forwarded_header(v);
            if !host.is_empty() {
                return Some(host);
            }
        }
        None
    }

    pub(super) fn callback_path(&self) -> &str {
        &self.callback_path
    }

    pub(super) fn resolve_redirect_uri(&self, session: &dyn PluginSession) -> VerifyResult<String> {
        let configured = self.config.redirect_uri.as_deref().unwrap_or("/.edgion/oidc/callback");
        if configured.starts_with("https://") || configured.starts_with("http://") {
            return Ok(configured.to_string());
        }
        if !configured.starts_with('/') {
            return Err(OidcVerifyError::Auth(AuthFailure::InvalidRedirect));
        }

        let host = Self::request_host(session).ok_or(OidcVerifyError::Auth(AuthFailure::MissingRedirectHost))?;
        let scheme = Self::request_scheme(session);
        Ok(format!("{}://{}{}", scheme, host, configured))
    }

    pub(super) fn is_reserved_authorization_param(key: &str) -> bool {
        [
            "response_type",
            "client_id",
            "redirect_uri",
            "scope",
            "state",
            "nonce",
            "code_challenge",
            "code_challenge_method",
        ]
        .into_iter()
        .any(|reserved| key.eq_ignore_ascii_case(reserved))
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

        let mut available: HashSet<&str> = HashSet::new();
        if let Some(scope) = claims.get("scope").and_then(|v| v.as_str()) {
            for s in scope.split_whitespace() {
                available.insert(s);
            }
        }
        if let Some(scp) = claims.get("scp") {
            match scp {
                Value::String(s) => {
                    for item in s.split_whitespace() {
                        available.insert(item);
                    }
                }
                Value::Array(arr) => {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            available.insert(s);
                        }
                    }
                }
                _ => {}
            }
        }

        for required in required_scopes {
            if !available.contains(required.as_str()) {
                return Err(OidcVerifyError::Auth(AuthFailure::InsufficientScope));
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
                return Err(OidcVerifyError::Auth(AuthFailure::NotYetValid));
            }
        }

        let issuer_matches = match self.config.issuers.as_deref() {
            Some([]) => true,
            Some(issuers) => claims
                .get("iss")
                .and_then(|v| v.as_str())
                .is_some_and(|iss| issuers.iter().any(|issuer| issuer == iss)),
            None => default_issuer.is_none_or(|expected| claims.get("iss").and_then(|v| v.as_str()) == Some(expected)),
        };
        if !issuer_matches {
            return Err(OidcVerifyError::Auth(AuthFailure::InvalidIssuer));
        }

        if let Some(ref audiences) = self.config.audiences {
            if !audiences.is_empty() && !Self::claims_audience_matches(claims.get("aud"), audiences) {
                return Err(OidcVerifyError::Auth(AuthFailure::InvalidAudience));
            }
        }

        Ok(())
    }
}
