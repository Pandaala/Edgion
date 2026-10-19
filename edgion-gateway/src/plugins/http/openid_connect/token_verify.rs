use super::*;

impl OpenidConnect {
    /// `_session` is unused here (introspection never touches the JWKS
    /// provider), but the signature must line up with `verify_jwt_token`
    /// since `verify_token` dispatches to either branch depending on
    /// `verification_mode`.
    pub(super) async fn verify_token_via_introspection(
        &self,
        _session: &mut dyn PluginSession,
        token: &str,
    ) -> VerifyResult<Value> {
        let discovery = self.get_or_fetch_discovery().await.map_err(OidcVerifyError::from)?;
        let endpoint = self
            .config
            .introspection_endpoint
            .clone()
            .filter(|v| !v.is_empty())
            .or_else(|| discovery.introspection_endpoint.clone().filter(|v| !v.is_empty()))
            .ok_or(OidcVerifyError::dependency(
                OidcOperation::Introspection,
                OidcDependencyFailure::MissingEndpoint,
            ))?;

        if let Some(cached_claims) = self.get_cached_introspection_claims(token).await {
            self.validate_claims_from_value(&cached_claims, Some(discovery.issuer.as_str()))?;
            self.validate_required_scopes(&cached_claims)?;
            return Ok(cached_claims);
        }

        Self::admit_idp_url(&endpoint, OidcOperation::Introspection)?;
        let client = self.http_client();
        let base_request = client
            .post(&endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .header("Accept", "application/json");

        let request = {
            let client_secret = self.client_secret().ok_or_else(Self::credential_failure)?;
            match self.config.introspection_endpoint_auth_method {
                EndpointAuthMethod::ClientSecretBasic => base_request
                    .basic_auth(self.config.client_id.as_str(), Some(client_secret))
                    .form(&[("token", token)]),
                EndpointAuthMethod::ClientSecretPost => base_request.form(&[
                    ("token", token),
                    ("client_id", self.config.client_id.as_str()),
                    ("client_secret", client_secret),
                ]),
            }
        };

        let resp = request
            .send()
            .await
            .map_err(|error| OidcDependencyError::transport(OidcOperation::Introspection, &error, "openid_connect"))?;
        if !resp.status().is_success() {
            return Err(OidcVerifyError::dependency(
                OidcOperation::Introspection,
                OidcDependencyFailure::HttpStatus,
            ));
        }

        let claims: Value = super::token_exchange::read_auth_response(resp, OidcOperation::Introspection).await?;
        if !claims.get("active").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Err(OidcVerifyError::Auth(AuthFailure::InactiveToken));
        }

        self.validate_claims_from_value(&claims, Some(discovery.issuer.as_str()))?;
        self.validate_required_scopes(&claims)?;
        self.cache_introspection_claims(token, &claims).await;
        Ok(claims)
    }

    pub(super) async fn verify_jwt_token_internal(
        &self,
        session: &mut dyn PluginSession,
        token: &str,
        validate_scopes: bool,
        audience_override: Option<&[String]>,
        validate_exp: bool,
    ) -> VerifyResult<Value> {
        let header = decode_header(token).map_err(|_| OidcVerifyError::InvalidFormat)?;
        let token_alg = header.alg;

        let policy = self
            .signing_policy
            .as_ref()
            .map_err(|failure| OidcVerifyError::Auth(*failure))?;
        validate_token_alg(token_alg, policy.expected_alg, &policy.allowed_algs)?;

        let build_decoding_key = |jwk: &jsonwebtoken::jwk::Jwk| {
            DecodingKey::from_jwk(jwk).map_err(|_| OidcVerifyError::Auth(AuthFailure::InvalidKey))
        };
        let decoding_key = {
            let jwks = self
                .get_or_fetch_jwks(session, false)
                .await
                .map_err(OidcVerifyError::from)?;

            match select_jwk(&header, &jwks, token_alg) {
                Ok(jwk) => build_decoding_key(jwk)?,
                Err(JwkSelectError::KidNotFound) => {
                    // kid miss: force one refresh (singleflight + min refresh interval in cache layer)
                    let refreshed_jwks = self
                        .get_or_fetch_jwks(session, true)
                        .await
                        .map_err(OidcVerifyError::from)?;
                    let jwk = select_jwk(&header, &refreshed_jwks, token_alg)
                        .map_err(|_| OidcVerifyError::Auth(AuthFailure::MissingKey))?;
                    build_decoding_key(jwk)?
                }
                Err(JwkSelectError::NoSuitableKey) => {
                    return Err(OidcVerifyError::Auth(AuthFailure::NoSuitableKey));
                }
            }
        };
        let discovery = self.get_or_fetch_discovery().await.map_err(OidcVerifyError::from)?;

        let mut validation = Validation::new(token_alg);
        validation.leeway = self.config.clock_skew_seconds;
        validation.validate_exp = validate_exp;
        validation.validate_nbf = true;
        validation.set_required_spec_claims(&["exp"]);

        if let Some(ref issuers) = self.config.issuers {
            if !issuers.is_empty() {
                validation.set_issuer(issuers);
            }
        } else if !discovery.issuer.is_empty() {
            validation.set_issuer(&[discovery.issuer.as_str()]);
        }

        if let Some(override_audiences) = audience_override {
            if !override_audiences.is_empty() {
                validation.set_audience(override_audiences);
            } else {
                validation.validate_aud = false;
            }
        } else if let Some(ref audiences) = self.config.audiences {
            if !audiences.is_empty() {
                validation.set_audience(audiences);
            } else {
                validation.validate_aud = false;
            }
        } else {
            validation.validate_aud = false;
        }

        let token_data = decode::<Claims>(token, &decoding_key, &validation).map_err(map_jwt_decode_error_typed)?;
        let claims = token_data.claims.into_value();

        if validate_scopes {
            self.validate_required_scopes(&claims)?;
        }
        Ok(claims)
    }

    pub(super) async fn verify_jwt_token(&self, session: &mut dyn PluginSession, token: &str) -> VerifyResult<Value> {
        self.verify_jwt_token_internal(session, token, true, None, true).await
    }

    pub(super) async fn verify_jwt_token_allow_expired(
        &self,
        session: &mut dyn PluginSession,
        token: &str,
    ) -> VerifyResult<Value> {
        self.verify_jwt_token_internal(session, token, true, None, false).await
    }

    pub(super) fn is_refresh_wait_timeout(err: &OidcVerifyError) -> bool {
        matches!(err, OidcVerifyError::RefreshSingleflightTimeout)
    }

    pub(super) fn is_expired_within_leeway(&self, claims: &Value) -> bool {
        let Some(exp) = claims.get("exp").and_then(|v| v.as_u64()) else {
            return false;
        };
        if self.config.access_token_expires_leeway == 0 {
            return false;
        }
        let now = Self::now_unix_secs();
        now > exp && now <= exp.saturating_add(self.config.access_token_expires_leeway)
    }

    pub(super) fn should_auto_fallback_to_introspection(jwt_error: &OidcVerifyError) -> bool {
        matches!(jwt_error, OidcVerifyError::InvalidFormat)
    }

    pub(super) async fn verify_token(&self, session: &mut dyn PluginSession, token: &str) -> VerifyResult<Value> {
        match self.config.verification_mode {
            VerificationMode::JwksOnly => self.verify_jwt_token(session, token).await,
            VerificationMode::IntrospectionOnly => self.verify_token_via_introspection(session, token).await,
            VerificationMode::Auto => {
                if self.config.use_jwks && Self::looks_like_jwt(token) {
                    match self.verify_jwt_token(session, token).await {
                        Ok(claims) => Ok(claims),
                        Err(jwt_error) if Self::should_auto_fallback_to_introspection(&jwt_error) => {
                            match self.verify_token_via_introspection(session, token).await {
                                Ok(claims) => Ok(claims),
                                Err(err) => Err(jwt_error.with_introspection_failure(err)),
                            }
                        }
                        Err(err) => Err(err),
                    }
                } else {
                    self.verify_token_via_introspection(session, token).await
                }
            }
        }
    }

    pub(super) fn introspection_cache_key(raw_token: &str) -> String {
        let digest = Sha256::digest(raw_token.as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
    }

    pub(super) async fn get_cached_introspection_claims(&self, token: &str) -> Option<Value> {
        if self.config.introspection_cache_ttl == 0 {
            return None;
        }
        let key = Self::introspection_cache_key(token);
        self.introspection_cache.get(&key).0.map(|entry| entry.claims)
    }

    pub(super) async fn cache_introspection_claims(&self, token: &str, claims: &Value) {
        if self.config.introspection_cache_ttl == 0 || claims.get("active") != Some(&Value::Bool(true)) {
            return;
        }
        let key = Self::introspection_cache_key(token);
        let ttl =
            Self::bounded_introspection_cache_ttl(claims, self.config.introspection_cache_ttl, Self::now_unix_secs());
        self.introspection_cache
            .insert(key, IntrospectionCacheEntry { claims: claims.clone() }, ttl);
        // Non-retention is a normal cache miss; the current request still uses
        // the authoritative introspection result.
    }

    pub(super) fn bounded_introspection_cache_ttl(claims: &Value, configured_ttl_secs: u64, now: u64) -> Duration {
        let configured_ttl = Duration::from_secs(configured_ttl_secs);
        claims
            .get("exp")
            .and_then(Value::as_u64)
            .map_or(configured_ttl, |expires_at| {
                configured_ttl.min(Duration::from_secs(expires_at.saturating_sub(now)))
            })
    }

    pub(super) async fn validate_id_token_nonce(
        &self,
        session: &mut dyn PluginSession,
        id_token: &str,
        expected_nonce: &str,
    ) -> VerifyResult<()> {
        let id_token_audience = vec![self.config.client_id.clone()];
        let claims = self
            .verify_jwt_token_internal(session, id_token, false, Some(&id_token_audience), true)
            .await?;
        let actual_nonce = claims.get("nonce").and_then(|v| v.as_str());
        if actual_nonce != Some(expected_nonce) {
            return Err(OidcVerifyError::Auth(AuthFailure::InvalidNonce));
        }
        Ok(())
    }
}
