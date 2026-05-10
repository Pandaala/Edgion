use super::*;

impl OpenidConnect {
    pub(super) fn refresh_lock_key(raw_cookie_value: &str) -> String {
        let digest = Sha256::digest(raw_cookie_value.as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
    }

    pub(super) async fn cache_access_token(&self, session_ref: &str, token: &str, expires_at: Option<u64>) {
        if session_ref.is_empty() || token.is_empty() {
            return;
        }
        let clock_skew = self.config.clock_skew_seconds;
        // TTL = token exp - now - clock_skew, capped at 1 hour, fallback 5 min
        let ttl = expires_at
            .and_then(|exp| exp.checked_sub(Self::now_unix_secs()))
            .and_then(|secs| secs.checked_sub(clock_skew))
            .map(|secs| Duration::from_secs(secs).min(Duration::from_secs(3600)))
            .filter(|d| !d.is_zero())
            .unwrap_or(Duration::from_secs(300));
        self.access_token_cache.insert(
            session_ref.to_string(),
            AccessTokenCacheEntry {
                token: token.to_string(),
                _expires_at: expires_at,
            },
            Some(ttl),
        );
        // The old `retain` and `len > 4096` emergency-clear are removed.
        // LocalLruCache provides LRU eviction at configured capacity.
    }

    pub(super) async fn get_cached_access_token(&self, session_ref: &str) -> Option<String> {
        if session_ref.is_empty() {
            return None;
        }
        // Expiry (including clock_skew) was baked into TTL at insert time — no manual check needed.
        self.access_token_cache.get(&session_ref.to_string()).map(|e| e.token)
    }

    pub(super) async fn remove_cached_access_token(&self, session_ref: &str) {
        if session_ref.is_empty() {
            return;
        }
        self.access_token_cache.remove(&session_ref.to_string());
    }

    pub(super) async fn resolve_access_token_from_session(&self, session_cookie: &OidcSessionCookie) -> Option<String> {
        if !session_cookie.access_token.trim().is_empty() {
            return Some(session_cookie.access_token.clone());
        }
        self.get_cached_access_token(&session_cookie.session_ref).await
    }

    pub(super) fn should_preemptive_refresh(&self, payload: &OidcSessionCookie) -> bool {
        if !self.config.renew_access_token_on_expiry {
            return false;
        }
        if payload.refresh_token.as_deref().is_none() {
            return false;
        }
        if payload.access_token.trim().is_empty() {
            return true;
        }
        let Some(exp) = payload.expires_at else {
            return false;
        };
        let now = Self::now_unix_secs();
        now.saturating_add(self.config.access_token_expires_leeway) >= exp
    }

    pub(super) async fn maybe_set_session_cookie_header(
        &self,
        session: &mut dyn PluginSession,
        payload: &OidcSessionCookie,
    ) {
        let mut persisted_payload = payload.clone();
        if persisted_payload.session_ref.is_empty() {
            persisted_payload.session_ref = Self::generate_session_ref();
        }
        if !persisted_payload.access_token.trim().is_empty() {
            self.cache_access_token(
                &persisted_payload.session_ref,
                &persisted_payload.access_token,
                persisted_payload.expires_at,
            )
            .await;
            persisted_payload.access_token.clear();
        }

        let session_secret = match self.resolve_session_secret() {
            Ok(v) => v,
            Err(_) => return,
        };
        let value = match self.encode_signed_cookie_payload(&persisted_payload, &session_secret) {
            Ok(v) => v,
            Err(_) => return,
        };
        let cookie = format!(
            "{}={}{}",
            self.session_cookie_name(),
            value,
            self.build_cookie_attr_suffix(session, self.config.session_lifetime, "/")
        );
        if self.ensure_session_cookie_size_limit(&cookie).is_err() {
            return;
        }
        let _ = session.set_response_header("Set-Cookie", &cookie);
    }

    pub(super) async fn exchange_code_for_token(
        &self,
        discovery: &DiscoveryDocument,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> VerifyResult<TokenEndpointResponse> {
        let token_endpoint =
            discovery
                .token_endpoint
                .as_deref()
                .filter(|v| !v.is_empty())
                .ok_or(OidcVerifyError::Other {
                    status: 502,
                    message: "OIDC discovery document missing token_endpoint".to_string(),
                })?;
        let client_secret = self.resolve_client_secret().map_err(|e| OidcVerifyError::Other {
            status: 401,
            message: format!("Missing client secret for token exchange: {}", e),
        })?;

        let mut params: Vec<(&str, String)> = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", redirect_uri.to_string()),
            ("client_id", self.config.client_id.clone()),
        ];
        if let Some(verifier) = code_verifier {
            params.push(("code_verifier", verifier.to_string()));
        }

        let client = self.http_client();
        let base_request = client
            .post(token_endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .header("Accept", "application/json");

        let request = match self.config.token_endpoint_auth_method {
            EndpointAuthMethod::ClientSecretBasic => base_request
                .basic_auth(self.config.client_id.as_str(), Some(client_secret))
                .form(&params),
            EndpointAuthMethod::ClientSecretPost => {
                params.push(("client_secret", client_secret));
                base_request.form(&params)
            }
        };

        let resp = request.send().await.map_err(|e| OidcVerifyError::Other {
            status: 502,
            message: format!("Token exchange request failed: {}", e),
        })?;
        if !resp.status().is_success() {
            return Err(OidcVerifyError::Other {
                status: 502,
                message: format!("Token exchange request failed with status {}", resp.status()),
            });
        }
        resp.json::<TokenEndpointResponse>()
            .await
            .map_err(|e| OidcVerifyError::Other {
                status: 502,
                message: format!("Invalid token endpoint response: {}", e),
            })
    }

    pub(super) async fn revoke_token(
        &self,
        endpoint: &str,
        client_secret: &str,
        token: &str,
        token_type_hint: Option<&str>,
    ) -> OidcResult<()> {
        let client = self.http_client();
        let base_request = client
            .post(endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .header("Accept", "application/json");

        let request = match self.config.token_endpoint_auth_method {
            EndpointAuthMethod::ClientSecretBasic => {
                let mut form: Vec<(&str, String)> = vec![("token", token.to_string())];
                if let Some(hint) = token_type_hint {
                    form.push(("token_type_hint", hint.to_string()));
                }
                base_request
                    .basic_auth(self.config.client_id.as_str(), Some(client_secret))
                    .form(&form)
            }
            EndpointAuthMethod::ClientSecretPost => {
                let mut form: Vec<(&str, String)> = vec![
                    ("token", token.to_string()),
                    ("client_id", self.config.client_id.clone()),
                    ("client_secret", client_secret.to_string()),
                ];
                if let Some(hint) = token_type_hint {
                    form.push(("token_type_hint", hint.to_string()));
                }
                base_request.form(&form)
            }
        };

        let resp = request.send().await?;
        if !resp.status().is_success() {
            return Err(format!("Revocation request failed with status {}", resp.status()).into());
        }
        Ok(())
    }

    pub(super) async fn maybe_revoke_tokens_on_logout(&self, session_cookie: Option<OidcSessionCookie>) {
        if !self.config.revoke_tokens_on_logout {
            return;
        }

        let Some(session_cookie) = session_cookie else {
            return;
        };

        let refresh_token = session_cookie.refresh_token.as_deref().filter(|v| !v.trim().is_empty());
        let cached_access_token = self.get_cached_access_token(&session_cookie.session_ref).await;
        let access_token = if session_cookie.access_token.trim().is_empty() {
            cached_access_token.as_deref()
        } else {
            Some(session_cookie.access_token.as_str())
        };
        if refresh_token.is_none() && access_token.is_none() {
            return;
        }

        let discovery = match self.cached_discovery_doc().await {
            Some(v) => v,
            None => return,
        };
        let Some(revocation_endpoint) = discovery.revocation_endpoint.as_deref().filter(|v| !v.is_empty()) else {
            return;
        };

        let client_secret = match self.resolve_client_secret() {
            Ok(v) => v,
            Err(_) => return,
        };

        if let Some(token) = refresh_token {
            let _ = self
                .revoke_token(
                    revocation_endpoint,
                    client_secret.as_str(),
                    token,
                    Some("refresh_token"),
                )
                .await;
        }

        if let Some(token) = access_token {
            let _ = self
                .revoke_token(revocation_endpoint, client_secret.as_str(), token, Some("access_token"))
                .await;
        }
    }

    pub(super) async fn refresh_access_token(
        &self,
        discovery: &DiscoveryDocument,
        refresh_token: &str,
    ) -> VerifyResult<TokenEndpointResponse> {
        let token_endpoint =
            discovery
                .token_endpoint
                .as_deref()
                .filter(|v| !v.is_empty())
                .ok_or(OidcVerifyError::Other {
                    status: 502,
                    message: "OIDC discovery document missing token_endpoint".to_string(),
                })?;
        let client_secret = self.resolve_client_secret().map_err(|e| OidcVerifyError::Other {
            status: 401,
            message: format!("Missing client secret for token refresh: {}", e),
        })?;

        let mut params: Vec<(&str, String)> = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token.to_string()),
            ("client_id", self.config.client_id.clone()),
        ];

        let client = self.http_client();
        let base_request = client
            .post(token_endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .header("Accept", "application/json");

        let request = match self.config.token_endpoint_auth_method {
            EndpointAuthMethod::ClientSecretBasic => base_request
                .basic_auth(self.config.client_id.as_str(), Some(client_secret))
                .form(&params),
            EndpointAuthMethod::ClientSecretPost => {
                params.push(("client_secret", client_secret));
                base_request.form(&params)
            }
        };

        let resp = request.send().await.map_err(|e| OidcVerifyError::Other {
            status: 502,
            message: format!("Token refresh request failed: {}", e),
        })?;
        if !resp.status().is_success() {
            return Err(OidcVerifyError::Other {
                status: 502,
                message: format!("Token refresh request failed with status {}", resp.status()),
            });
        }
        resp.json::<TokenEndpointResponse>()
            .await
            .map_err(|e| OidcVerifyError::Other {
                status: 502,
                message: format!("Invalid token refresh response: {}", e),
            })
    }

    pub(super) fn should_persist_id_token(&self, discovery: &DiscoveryDocument) -> bool {
        self.config.set_id_token_header || discovery.end_session_endpoint.as_deref().is_some_and(|v| !v.is_empty())
    }

    pub(super) async fn try_refresh_session_token(
        &self,
        session_cookie: &mut OidcSessionCookie,
    ) -> VerifyResult<String> {
        if session_cookie.session_ref.is_empty() {
            session_cookie.session_ref = Self::generate_session_ref();
        }
        let refresh_token = session_cookie.refresh_token.as_deref().ok_or(OidcVerifyError::Other {
            status: 401,
            message: "Missing refresh token".to_string(),
        })?;
        let discovery = self
            .get_or_fetch_discovery()
            .await
            .map_err(|e| OidcVerifyError::Other {
                status: 502,
                message: format!("Failed to fetch OIDC discovery document: {}", e),
            })?;
        let token_response = self.refresh_access_token(&discovery, refresh_token).await?;

        let now = Self::now_unix_secs();
        let should_persist_id_token = self.should_persist_id_token(&discovery);
        session_cookie.access_token = token_response.access_token.clone();
        session_cookie.expires_at = token_response.expires_in.map(|ttl| now.saturating_add(ttl));
        if should_persist_id_token {
            let new_id_token = token_response.id_token.as_deref().filter(|v| !v.is_empty());
            if let Some(new_id_token) = new_id_token {
                session_cookie.id_token = Some(new_id_token.to_string());
            }
        }
        if token_response.refresh_token.is_some() {
            session_cookie.refresh_token = token_response.refresh_token;
        }
        Ok(session_cookie.access_token.clone())
    }

    pub(super) async fn get_or_create_refresh_lock(&self, lock_key: &str) -> Arc<Mutex<()>> {
        // ShardedLruCache has no entry/get_or_insert_with API, so this is best-effort
        // get-or-insert: under a true concurrent miss two callers can each insert their
        // own lock and the last writer wins. The leader's lock is then orphaned and the
        // two refreshes proceed in parallel — the second waiter's `try_get_recent_refresh_result`
        // double-check after acquiring its own lock still de-dupes via the results cache,
        // and worst case is one extra refresh round-trip for that key.
        if let Some(existing) = self.refresh_singleflight_locks.get(&lock_key.to_string()) {
            return existing;
        }
        let new_lock = Arc::new(Mutex::new(()));
        self.refresh_singleflight_locks
            .insert(lock_key.to_string(), new_lock.clone(), None);
        new_lock
    }

    pub(super) async fn try_get_recent_refresh_result(&self, lock_key: &str) -> Option<OidcSessionCookie> {
        // Freshness window is owned by the cache TTL (see `put_refresh_result`); LRU
        // lazy-expires entries past the window on read, no manual elapsed-check needed.
        self.refresh_singleflight_results.get(&lock_key.to_string())
    }

    pub(super) async fn put_refresh_result(&self, lock_key: &str, payload: OidcSessionCookie) {
        self.refresh_singleflight_results
            .insert(lock_key.to_string(), payload, Some(Duration::from_secs(5)));
    }

    pub(super) async fn try_refresh_session_token_singleflight(
        &self,
        lock_key: &str,
        session_cookie: &mut OidcSessionCookie,
    ) -> VerifyResult<String> {
        let lock = self.get_or_create_refresh_lock(lock_key).await;
        let wait_timeout = Duration::from_secs(self.config.timeout);
        let _guard = match tokio::time::timeout(wait_timeout, lock.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                if let Some(cached_payload) = self.try_get_recent_refresh_result(lock_key).await {
                    *session_cookie = cached_payload;
                    return Ok(session_cookie.access_token.clone());
                }
                return Err(OidcVerifyError::RefreshSingleflightTimeout);
            }
        };

        if let Some(cached_payload) = self.try_get_recent_refresh_result(lock_key).await {
            *session_cookie = cached_payload;
            return Ok(session_cookie.access_token.clone());
        }

        let token = self.try_refresh_session_token(session_cookie).await?;
        self.put_refresh_result(lock_key, session_cookie.clone()).await;
        Ok(token)
    }
}
