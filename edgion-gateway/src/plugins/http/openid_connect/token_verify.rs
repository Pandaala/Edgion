use super::*;

impl OpenidConnect {
    pub(super) async fn verify_token_via_introspection(&self, token: &str) -> VerifyResult<Value> {
        let discovery = self
            .get_or_fetch_discovery()
            .await
            .map_err(|_| OidcVerifyError::Other {
                status: 502,
                message: "Bad Gateway".to_string(),
            })?;
        let endpoint = self
            .config
            .introspection_endpoint
            .clone()
            .filter(|v| !v.is_empty())
            .or_else(|| discovery.introspection_endpoint.clone().filter(|v| !v.is_empty()))
            .ok_or(OidcVerifyError::Other {
                status: 502,
                message: "OIDC introspection endpoint is not configured".to_string(),
            })?;

        if let Some(cached_claims) = self.get_cached_introspection_claims(token).await {
            self.validate_claims_from_value(&cached_claims, Some(discovery.issuer.as_str()))?;
            self.validate_required_scopes(&cached_claims)?;
            return Ok(cached_claims);
        }

        let client_secret = self.resolve_client_secret().map_err(|e| OidcVerifyError::Other {
            status: 401,
            message: format!("Missing client secret for introspection: {}", e),
        })?;

        let client = self.http_client();
        let base_request = client
            .post(&endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .header("Accept", "application/json");

        let request = match self.config.introspection_endpoint_auth_method {
            EndpointAuthMethod::ClientSecretBasic => base_request
                .basic_auth(self.config.client_id.as_str(), Some(client_secret))
                .form(&[("token", token)]),
            EndpointAuthMethod::ClientSecretPost => base_request.form(&[
                ("token", token),
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", client_secret.as_str()),
            ]),
        };

        let resp = request.send().await.map_err(|e| {
            super::token_exchange::note_oidc_san_mismatch(&e, "introspection");
            OidcVerifyError::Other {
                status: 502,
                message: "Bad Gateway".to_string(),
            }
        })?;
        if !resp.status().is_success() {
            return Err(OidcVerifyError::Other {
                status: 502,
                message: format!("Introspection request failed with status {}", resp.status()),
            });
        }

        let claims: Value = resp.json().await.map_err(|e| OidcVerifyError::Other {
            status: 502,
            message: format!("Invalid introspection response: {}", e),
        })?;
        if !claims.get("active").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Err(OidcVerifyError::Other {
                status: 401,
                message: "Inactive token".to_string(),
            });
        }

        self.validate_claims_from_value(&claims, Some(discovery.issuer.as_str()))?;
        self.validate_required_scopes(&claims)?;
        self.cache_introspection_claims(token, &claims).await;
        Ok(claims)
    }

    pub(super) async fn verify_jwt_token_internal(
        &self,
        token: &str,
        validate_scopes: bool,
        audience_override: Option<&[String]>,
        validate_exp: bool,
    ) -> VerifyResult<Value> {
        let header = decode_header(token).map_err(|_| OidcVerifyError::InvalidFormat)?;
        let token_alg = header.alg;

        let (expected_alg, allowed_algs) = resolve_algorithm_policy(
            self.config.token_signing_alg.as_deref(),
            self.config.allowed_signing_algs.as_deref(),
        )
        .map_err(|(status, message)| OidcVerifyError::Other { status, message })?;
        validate_token_alg(token_alg, expected_alg, &allowed_algs)
            .map_err(|(status, message)| OidcVerifyError::Other { status, message })?;

        let mut jwks = self
            .get_or_fetch_jwks(false)
            .await
            .map_err(|e| OidcVerifyError::JwksFetch(e.to_string()))?;

        let jwk = match select_jwk(&header, &jwks, token_alg) {
            Ok(jwk) => jwk,
            Err(JwkSelectError::KidNotFound) => {
                // kid miss: force one refresh (singleflight + min refresh interval in cache layer)
                jwks = self
                    .get_or_fetch_jwks(true)
                    .await
                    .map_err(|e| OidcVerifyError::JwksFetch(format!("refresh on kid miss: {}", e)))?;
                select_jwk(&header, &jwks, token_alg).map_err(|_| OidcVerifyError::Other {
                    status: 401,
                    message: "No matching JWK found for token kid".to_string(),
                })?
            }
            Err(JwkSelectError::NoSuitableKey) => {
                return Err(OidcVerifyError::Other {
                    status: 401,
                    message: "No suitable JWK for token algorithm".to_string(),
                });
            }
        };

        let decoding_key = DecodingKey::from_jwk(&jwk).map_err(|e| OidcVerifyError::Other {
            status: 401,
            message: format!("Failed to build decoding key: {}", e),
        })?;
        let discovery = self
            .get_or_fetch_discovery()
            .await
            .map_err(|_| OidcVerifyError::Other {
                status: 502,
                message: "Bad Gateway".to_string(),
            })?;

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
        let claims = token_data.claims.to_value();

        if validate_scopes {
            self.validate_required_scopes(&claims)?;
        }
        Ok(claims)
    }

    pub(super) async fn verify_jwt_token(&self, token: &str) -> VerifyResult<Value> {
        self.verify_jwt_token_internal(token, true, None, true).await
    }

    pub(super) async fn verify_jwt_token_allow_expired(&self, token: &str) -> VerifyResult<Value> {
        self.verify_jwt_token_internal(token, true, None, false).await
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

    pub(super) async fn verify_token(&self, token: &str) -> VerifyResult<Value> {
        match self.config.verification_mode {
            VerificationMode::JwksOnly => self.verify_jwt_token(token).await,
            VerificationMode::IntrospectionOnly => self.verify_token_via_introspection(token).await,
            VerificationMode::Auto => {
                if self.config.use_jwks && Self::looks_like_jwt(token) {
                    match self.verify_jwt_token(token).await {
                        Ok(claims) => Ok(claims),
                        Err(jwt_error) if Self::should_auto_fallback_to_introspection(&jwt_error) => {
                            match self.verify_token_via_introspection(token).await {
                                Ok(claims) => Ok(claims),
                                Err(_) => Err(jwt_error),
                            }
                        }
                        Err(err) => Err(err),
                    }
                } else {
                    self.verify_token_via_introspection(token).await
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
        self.introspection_cache.get(&key).map(|e| e.claims)
    }

    pub(super) async fn cache_introspection_claims(&self, token: &str, claims: &Value) {
        if self.config.introspection_cache_ttl == 0 {
            return;
        }
        let key = Self::introspection_cache_key(token);
        let config_ttl = Duration::from_secs(self.config.introspection_cache_ttl);
        // Cap TTL at token's remaining lifetime
        let token_remaining = claims["exp"]
            .as_u64()
            .and_then(|exp| exp.checked_sub(Self::now_unix_secs()))
            .map(Duration::from_secs);
        let ttl = match token_remaining {
            Some(remaining) => config_ttl.min(remaining),
            None => config_ttl,
        };
        self.introspection_cache
            .insert(key, IntrospectionCacheEntry { claims: claims.clone() }, Some(ttl));
        // No retain/len check needed — LocalLruCache handles eviction automatically
    }

    pub(super) async fn validate_id_token_nonce(&self, id_token: &str, expected_nonce: &str) -> VerifyResult<()> {
        let id_token_audience = vec![self.config.client_id.clone()];
        let claims = self
            .verify_jwt_token_internal(id_token, false, Some(&id_token_audience), true)
            .await?;
        let actual_nonce = claims.get("nonce").and_then(|v| v.as_str());
        if actual_nonce != Some(expected_nonce) {
            return Err(OidcVerifyError::Other {
                status: 401,
                message: "Invalid ID token nonce".to_string(),
            });
        }
        Ok(())
    }
}
