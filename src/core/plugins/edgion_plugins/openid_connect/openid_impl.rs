use super::*;

impl OpenidConnect {
    pub fn new(config: &OpenidConnectConfig, plugin_namespace: String) -> Self {
        Self {
            name: "OpenidConnect".to_string(),
            config: config.clone(),
            plugin_namespace,
            discovery_doc: Arc::new(RwLock::new(None)),
            discovery_refresh: Arc::new(Mutex::new(())),
            jwks_state: Arc::new(RwLock::new(JwksState::default())),
            jwks_refresh: Arc::new(Mutex::new(())),
            introspection_cache: Arc::new(RwLock::new(HashMap::new())),
            access_token_cache: Arc::new(RwLock::new(HashMap::new())),
            refresh_singleflight_locks: Arc::new(Mutex::new(HashMap::new())),
            refresh_singleflight_results: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) fn extract_bearer_token(&self, session: &dyn PluginSession) -> Option<String> {
        let header = session.header_value("authorization")?;
        let header = header.trim();
        if header.len() < 8 {
            return None;
        }
        let (scheme, token) = header.split_at(7);
        if !scheme.eq_ignore_ascii_case("bearer ") {
            return None;
        }
        let token = token.trim();
        if token.is_empty() {
            None
        } else {
            Some(token.to_string())
        }
    }

    pub(super) async fn send_unauthorized(&self, session: &mut dyn PluginSession, body: &str) -> OidcResult<()> {
        send_auth_error_response(session, 401, "Bearer", &self.config.realm, body).await
    }

    pub(super) async fn send_forbidden(&self, session: &mut dyn PluginSession, body: &str) -> OidcResult<()> {
        send_auth_error_response(session, 403, "Bearer", &self.config.realm, body).await
    }

    pub(super) async fn send_plain_error(
        &self,
        session: &mut dyn PluginSession,
        status: u16,
        body: &str,
    ) -> OidcResult<()> {
        let mut resp = ResponseHeader::build(status, None)?;
        resp.insert_header("Content-Type", "text/plain")?;
        session.write_response_header(Box::new(resp), false).await?;
        session
            .write_response_body(Some(Bytes::from(format!("{} {}", status, body))), true)
            .await?;
        session.shutdown().await;
        Ok(())
    }

    pub(super) async fn send_redirect_with_cookies(
        &self,
        session: &mut dyn PluginSession,
        location: &str,
        cookies: &[String],
    ) -> OidcResult<()> {
        let mut resp = ResponseHeader::build(302, None)?;
        resp.insert_header("Location", location)?;
        if let Some(first_cookie) = cookies.first() {
            resp.insert_header("Set-Cookie", first_cookie)?;
            for cookie in &cookies[1..] {
                resp.append_header("Set-Cookie", cookie)?;
            }
        }
        resp.insert_header("Cache-Control", "no-store")?;
        resp.insert_header("Pragma", "no-cache")?;
        resp.insert_header("Content-Length", "0")?;
        session.write_response_header(Box::new(resp), true).await?;
        Ok(())
    }

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
        if configured.starts_with('/') {
            return configured.to_string();
        }
        if let Ok(url) = Url::parse(configured) {
            return url.path().to_string();
        }
        "/.edgion/oidc/callback".to_string()
    }

    pub(super) fn resolve_redirect_uri(&self, session: &dyn PluginSession) -> VerifyResult<String> {
        let configured = self.config.redirect_uri.as_deref().unwrap_or("/.edgion/oidc/callback");
        if configured.starts_with("https://") || configured.starts_with("http://") {
            return Ok(configured.to_string());
        }
        if !configured.starts_with('/') {
            return Err((401, "Invalid redirectUri".to_string()));
        }

        let host = Self::request_host(session).ok_or((401, "Missing host header for redirect".to_string()))?;
        let scheme = Self::request_scheme(session);
        Ok(format!("{}://{}{}", scheme, host, configured))
    }

    pub(super) fn random_urlsafe(bytes_len: usize) -> String {
        let mut bytes = vec![0u8; bytes_len];
        rand::rng().fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
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

    pub(super) fn build_authorization_redirect(
        &self,
        session: &dyn PluginSession,
        discovery: &DiscoveryDocument,
    ) -> VerifyResult<(String, String)> {
        let authorization_endpoint = discovery
            .authorization_endpoint
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or((
                502,
                "OIDC discovery document missing authorization_endpoint".to_string(),
            ))?;

        let mut auth_url = Url::parse(authorization_endpoint)
            .map_err(|e| (502, format!("Invalid authorization endpoint URL: {}", e)))?;

        let redirect_uri = self.resolve_redirect_uri(session)?;
        let state = Self::random_urlsafe(24);
        let mut code_verifier: Option<String> = None;
        let mut code_challenge: Option<String> = None;
        let mut nonce: Option<String> = None;

        if self.config.use_pkce {
            let verifier = Self::random_urlsafe(64);
            let digest = Sha256::digest(verifier.as_bytes());
            let challenge = URL_SAFE_NO_PAD.encode(digest);
            code_verifier = Some(verifier);
            code_challenge = Some(challenge);
        }
        if self.config.use_nonce {
            nonce = Some(Self::random_urlsafe(24));
        }

        {
            let mut qp = auth_url.query_pairs_mut();
            qp.append_pair("response_type", "code");
            qp.append_pair("client_id", &self.config.client_id);
            qp.append_pair("redirect_uri", &redirect_uri);
            qp.append_pair("scope", &self.config.scope);
            qp.append_pair("state", &state);
            if let Some(ref nonce_value) = nonce {
                qp.append_pair("nonce", nonce_value);
            }
            if let Some(ref challenge) = code_challenge {
                qp.append_pair("code_challenge_method", "S256");
                qp.append_pair("code_challenge", challenge);
            }
            if let Some(ref params) = self.config.authorization_params {
                for (key, value) in params {
                    if Self::is_reserved_authorization_param(key) {
                        tracing::warn!(
                            plugin = "OpenidConnect",
                            key = key,
                            "Skipped reserved authorization parameter override"
                        );
                        continue;
                    }
                    qp.append_pair(key, value);
                }
            }
        }

        let original_url = match session.get_query() {
            Some(q) if !q.is_empty() => format!("{}?{}", session.get_path(), q),
            _ => session.get_path().to_string(),
        };
        let cookie_payload = AuthorizationStateCookie {
            state,
            original_url,
            code_verifier,
            nonce,
            created_at: Self::now_unix_secs(),
        };
        let session_secret = self
            .resolve_session_secret()
            .map_err(|e| (401, format!("Missing session secret for auth flow: {}", e)))?;
        let cookie_value = self
            .encode_signed_cookie_payload(&cookie_payload, &session_secret)
            .map_err(|e| (502, format!("Failed to encode OIDC state cookie: {}", e)))?;
        let cookie_suffix = self.build_cookie_attr_suffix(session, 300, &self.callback_path());
        let cookie = format!("{}={}{}", Self::state_cookie_name(), cookie_value, cookie_suffix);

        Ok((auth_url.to_string(), cookie))
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
                return Err((403, "Insufficient scope".to_string()));
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
                return Err((401, "Token expired".to_string()));
            }
        }

        if let Some(nbf) = claims.get("nbf").and_then(|v| v.as_u64()) {
            if now.saturating_add(leeway) < nbf {
                return Err((401, "Token not yet valid".to_string()));
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
                return Err((401, "Invalid token issuer".to_string()));
            }
        }

        if let Some(ref audiences) = self.config.audiences {
            if !audiences.is_empty() && !Self::claims_audience_matches(claims.get("aud"), audiences) {
                return Err((401, "Invalid token audience".to_string()));
            }
        }

        Ok(())
    }

    pub(super) fn resolve_client_secret(&self) -> OidcResult<String> {
        if let Some(ref resolved) = self.config.resolved_client_secret {
            if !resolved.is_empty() {
                return Ok(resolved.clone());
            }
        }

        let secret_ref = self
            .config
            .client_secret_ref
            .as_ref()
            .ok_or("clientSecretRef is required for introspection flow")?;

        let namespace = secret_ref.namespace.as_deref().unwrap_or(&self.plugin_namespace);
        let secret = get_secret(Some(namespace), &secret_ref.name)
            .ok_or_else(|| format!("Secret {}/{} not found", namespace, secret_ref.name))?;
        let data = secret
            .data
            .as_ref()
            .ok_or_else(|| format!("Secret {}/{} has no data", namespace, secret_ref.name))?;

        let value = data
            .get("clientSecret")
            .or_else(|| data.get("client_secret"))
            .or_else(|| data.get("secret"))
            .ok_or_else(|| {
                format!(
                    "Secret {}/{} missing key (clientSecret/client_secret/secret)",
                    namespace, secret_ref.name
                )
            })?;

        let client_secret = String::from_utf8(value.0.clone()).map_err(|e| {
            format!(
                "Secret {}/{} client secret is not valid UTF-8: {}",
                namespace, secret_ref.name, e
            )
        })?;

        if client_secret.trim().is_empty() {
            return Err(format!("Secret {}/{} client secret is empty", namespace, secret_ref.name).into());
        }

        Ok(client_secret)
    }

    pub(super) fn resolve_session_secret(&self) -> OidcResult<String> {
        if let Some(ref resolved) = self.config.resolved_session_secret {
            if !resolved.is_empty() {
                return Ok(resolved.clone());
            }
        }

        let secret_ref = self
            .config
            .session_secret_ref
            .as_ref()
            .ok_or("sessionSecretRef is required for auth flow")?;
        let namespace = secret_ref.namespace.as_deref().unwrap_or(&self.plugin_namespace);
        let secret = get_secret(Some(namespace), &secret_ref.name)
            .ok_or_else(|| format!("Secret {}/{} not found", namespace, secret_ref.name))?;
        let data = secret
            .data
            .as_ref()
            .ok_or_else(|| format!("Secret {}/{} has no data", namespace, secret_ref.name))?;

        let value = data
            .get("sessionSecret")
            .or_else(|| data.get("session_secret"))
            .or_else(|| data.get("secret"))
            .ok_or_else(|| {
                format!(
                    "Secret {}/{} missing key (sessionSecret/session_secret/secret)",
                    namespace, secret_ref.name
                )
            })?;
        let session_secret = String::from_utf8(value.0.clone()).map_err(|e| {
            format!(
                "Secret {}/{} session secret is not valid UTF-8: {}",
                namespace, secret_ref.name, e
            )
        })?;
        if session_secret.trim().is_empty() {
            return Err(format!("Secret {}/{} session secret is empty", namespace, secret_ref.name).into());
        }
        Ok(session_secret)
    }

    pub(super) fn session_cipher(secret: &str) -> OidcResult<Aes256Gcm> {
        if secret.as_bytes().len() < 32 {
            return Err("Session secret must be at least 32 bytes for AES-256-GCM".into());
        }
        let digest = Sha256::digest(secret.as_bytes());
        Aes256Gcm::new_from_slice(&digest).map_err(|e| format!("Failed to initialize session cipher: {}", e).into())
    }

    pub(super) fn encode_signed_cookie_payload<T: Serialize>(&self, value: &T, secret: &str) -> OidcResult<String> {
        let cipher = Self::session_cipher(secret)?;
        let payload = serde_json::to_vec(value)?;
        let mut nonce_bytes = [0u8; 12];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let encrypted = cipher
            .encrypt(nonce, payload.as_ref())
            .map_err(|e| format!("Failed to encrypt OIDC cookie payload: {}", e))?;
        let nonce_b64 = URL_SAFE_NO_PAD.encode(nonce_bytes);
        let encrypted_b64 = URL_SAFE_NO_PAD.encode(encrypted);
        Ok(format!("{}.{}", nonce_b64, encrypted_b64))
    }

    pub(super) fn decode_signed_cookie_payload<T: for<'de> Deserialize<'de>>(
        &self,
        value: &str,
        secret: &str,
    ) -> VerifyResult<T> {
        let cipher = Self::session_cipher(secret).map_err(|e| (502, format!("Failed to verify OIDC cookie: {}", e)))?;
        let (nonce_b64, encrypted_b64) = value
            .split_once('.')
            .ok_or((401, "Malformed OIDC cookie payload".to_string()))?;
        let nonce = URL_SAFE_NO_PAD
            .decode(nonce_b64)
            .map_err(|_| (401, "Malformed OIDC cookie payload".to_string()))?;
        if nonce.len() != 12 {
            return Err((401, "Malformed OIDC cookie payload".to_string()));
        }
        let encrypted = URL_SAFE_NO_PAD
            .decode(encrypted_b64)
            .map_err(|_| (401, "Malformed OIDC cookie payload".to_string()))?;
        let nonce = Nonce::from_slice(&nonce);
        let payload = cipher
            .decrypt(nonce, encrypted.as_ref())
            .map_err(|_| (401, "Invalid OIDC cookie signature".to_string()))?;
        serde_json::from_slice::<T>(&payload).map_err(|_| (401, "Malformed OIDC cookie payload".to_string()))
    }

    pub(super) fn session_cookie_name(&self) -> &str {
        &self.config.session_cookie_name
    }

    pub(super) fn state_cookie_name() -> &'static str {
        "edgion_oidc_state"
    }

    pub(super) fn cookie_same_site_value(&self) -> &'static str {
        let value = self.config.session_cookie_same_site.as_str();
        if value.eq_ignore_ascii_case("strict") {
            "Strict"
        } else if value.eq_ignore_ascii_case("none") {
            "None"
        } else {
            "Lax"
        }
    }

    pub(super) fn build_cookie_attr_suffix(&self, _session: &dyn PluginSession, max_age: u64, path: &str) -> String {
        let secure_attr = if self.config.session_cookie_secure {
            "; Secure"
        } else {
            ""
        };
        let http_only_attr = if self.config.session_cookie_http_only {
            "; HttpOnly"
        } else {
            ""
        };
        let same_site = self.cookie_same_site_value();
        format!(
            "; Path={}; SameSite={}; Max-Age={}{}{}",
            path, same_site, max_age, http_only_attr, secure_attr
        )
    }

    pub(super) fn http_client(&self) -> &'static reqwest::Client {
        get_http_client_with_ssl_verify(self.config.ssl_verify)
    }

    pub(super) async fn verify_token_via_introspection(&self, token: &str) -> VerifyResult<Value> {
        let discovery = self
            .get_or_fetch_discovery()
            .await
            .map_err(|e| (502, format!("Failed to fetch OIDC discovery document: {}", e)))?;
        let endpoint = self
            .config
            .introspection_endpoint
            .clone()
            .filter(|v| !v.is_empty())
            .or_else(|| discovery.introspection_endpoint.clone().filter(|v| !v.is_empty()))
            .ok_or((502, "OIDC introspection endpoint is not configured".to_string()))?;

        if let Some(cached_claims) = self.get_cached_introspection_claims(token).await {
            self.validate_claims_from_value(&cached_claims, Some(discovery.issuer.as_str()))?;
            self.validate_required_scopes(&cached_claims)?;
            return Ok(cached_claims);
        }

        let client_secret = self
            .resolve_client_secret()
            .map_err(|e| (401, format!("Missing client secret for introspection: {}", e)))?;

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

        let resp = request
            .send()
            .await
            .map_err(|e| (502, format!("Introspection request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err((
                502,
                format!("Introspection request failed with status {}", resp.status()),
            ));
        }

        let claims: Value = resp
            .json()
            .await
            .map_err(|e| (502, format!("Invalid introspection response: {}", e)))?;
        if !claims.get("active").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Err((401, "Inactive token".to_string()));
        }

        self.validate_claims_from_value(&claims, Some(discovery.issuer.as_str()))?;
        self.validate_required_scopes(&claims)?;
        self.cache_introspection_claims(token, &claims).await;
        Ok(claims)
    }

    pub(super) fn is_header_value_safe(value: &str) -> bool {
        !value.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0')
    }

    pub(super) fn try_set_header_with_limits(
        &self,
        session: &mut dyn PluginSession,
        header_name: &str,
        header_value: &str,
        total_added_bytes: &mut usize,
    ) {
        let max_value = self.config.max_header_value_bytes as usize;
        let max_total = self.config.max_total_header_bytes as usize;

        if !Self::is_header_value_safe(header_value) {
            tracing::warn!(header = header_name, "Skipped unsafe OIDC header value");
            return;
        }
        if header_value.len() > max_value {
            tracing::warn!(
                header = header_name,
                len = header_value.len(),
                max = max_value,
                "Skipped OIDC header: value too large"
            );
            return;
        }

        let add = header_name.len() + header_value.len();
        if total_added_bytes.saturating_add(add) > max_total {
            tracing::warn!(max = max_total, "Stopped OIDC header mapping: total size limit reached");
            return;
        }

        *total_added_bytes = total_added_bytes.saturating_add(add);
        let _ = session.set_request_header(header_name, header_value);
    }

    pub(super) fn apply_upstream_headers(
        &self,
        session: &mut dyn PluginSession,
        access_token: &str,
        id_token: Option<&str>,
        userinfo_json: Option<&str>,
        claims: &Value,
    ) {
        let mut total_added: usize = 0;
        let max_total = self.config.max_total_header_bytes as usize;
        let max_value = self.config.max_header_value_bytes as usize;

        if self.config.set_access_token_header {
            self.try_set_header_with_limits(session, "X-Access-Token", access_token, &mut total_added);
        }

        if self.config.set_id_token_header {
            if let Some(id_token) = id_token.filter(|v| !v.trim().is_empty()) {
                self.try_set_header_with_limits(session, "X-ID-Token", id_token, &mut total_added);
            }
        }

        if self.config.set_userinfo_header {
            if let Some(userinfo) = userinfo_json {
                self.try_set_header_with_limits(session, "X-Userinfo", userinfo, &mut total_added);
            } else if let Ok(userinfo) = serde_json::to_string(claims) {
                self.try_set_header_with_limits(session, "X-Userinfo", &userinfo, &mut total_added);
            }
        }

        if self.config.access_token_in_authorization_header {
            let value = format!("Bearer {}", access_token);
            self.try_set_header_with_limits(session, "Authorization", &value, &mut total_added);
        }

        if self.config.store_claims_in_ctx {
            if let Ok(claims_json) = serde_json::to_string(claims) {
                let _ = session.set_ctx_var("oidc_claims", &claims_json);
            }
        }

        if let Some(ref mapping) = self.config.claims_to_headers {
            let remaining_total = max_total.saturating_sub(total_added);
            if remaining_total > 0 {
                set_common_claims_headers(session, claims, mapping, max_value, remaining_total);
            }
        }
    }

    pub(super) fn min_refresh_duration(&self) -> Duration {
        Duration::from_secs(self.config.jwks_min_refresh_interval)
    }

    pub(super) fn jwks_ttl_duration(&self) -> Duration {
        Duration::from_secs(self.config.jwks_cache_ttl)
    }

    pub(super) fn can_refresh(last_refresh_at: Option<Instant>, now: Instant, min_refresh: Duration) -> bool {
        match last_refresh_at {
            Some(last) => now.saturating_duration_since(last) >= min_refresh,
            None => true,
        }
    }

    pub(super) async fn get_or_fetch_discovery(&self) -> OidcResult<DiscoveryDocument> {
        if let Some(doc) = self.discovery_doc.read().await.as_ref() {
            return Ok(doc.clone());
        }

        let _lock = self.discovery_refresh.lock().await;
        if let Some(doc) = self.discovery_doc.read().await.as_ref() {
            return Ok(doc.clone());
        }

        let client = self.http_client();
        let resp = client
            .get(&self.config.discovery)
            .timeout(Duration::from_secs(self.config.timeout))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("OIDC discovery request failed with status {}", resp.status()).into());
        }

        let doc: DiscoveryDocument = resp.json().await?;
        if doc.issuer.is_empty() {
            return Err("OIDC discovery document missing issuer".into());
        }
        if doc.jwks_uri.is_empty() {
            return Err("OIDC discovery document missing jwks_uri".into());
        }

        *self.discovery_doc.write().await = Some(doc.clone());
        Ok(doc)
    }

    pub(super) async fn fetch_userinfo_json(&self, access_token: &str) -> Option<String> {
        if !self.config.set_userinfo_header {
            return None;
        }
        let discovery = match self.get_or_fetch_discovery().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(plugin = "OpenidConnect", error = %e, "Failed to load discovery for userinfo");
                return None;
            }
        };
        let userinfo_endpoint = match discovery.userinfo_endpoint.as_deref().filter(|v| !v.is_empty()) {
            Some(v) => v,
            None => return None,
        };

        let client = self.http_client();
        let resp = match client
            .get(userinfo_endpoint)
            .timeout(Duration::from_secs(self.config.timeout))
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(plugin = "OpenidConnect", error = %e, "Userinfo request failed");
                return None;
            }
        };
        if !resp.status().is_success() {
            tracing::warn!(
                plugin = "OpenidConnect",
                status = resp.status().as_u16(),
                "Userinfo request returned non-success status"
            );
            return None;
        }

        let value: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    plugin = "OpenidConnect",
                    error = %e,
                    "Failed to parse userinfo response as JSON"
                );
                return None;
            }
        };
        serde_json::to_string(&value).ok()
    }

    pub(super) async fn get_or_fetch_jwks(&self, force_refresh: bool) -> OidcResult<JwkSet> {
        let now = Instant::now();
        let min_refresh = self.min_refresh_duration();
        let ttl = self.jwks_ttl_duration();

        {
            let state = self.jwks_state.read().await;
            if let Some(ref set) = state.set {
                if !force_refresh && state.expires_at.is_some_and(|exp| now < exp) {
                    return Ok(set.clone());
                }
                if force_refresh && !Self::can_refresh(state.last_refresh_at, now, min_refresh) {
                    return Ok(set.clone());
                }
            }
        }

        let _guard = self.jwks_refresh.lock().await;
        let now = Instant::now();

        {
            let state = self.jwks_state.read().await;
            if let Some(ref set) = state.set {
                if !force_refresh && state.expires_at.is_some_and(|exp| now < exp) {
                    return Ok(set.clone());
                }
                if force_refresh && !Self::can_refresh(state.last_refresh_at, now, min_refresh) {
                    return Ok(set.clone());
                }
            } else if !Self::can_refresh(state.last_refresh_at, now, min_refresh) {
                return Err("JWKS refresh blocked by minimum refresh interval".into());
            }
        }

        let discovery = self.get_or_fetch_discovery().await?;
        let fetch_result: OidcResult<JwkSet> = async {
            let client = self.http_client();
            let resp = client
                .get(&discovery.jwks_uri)
                .timeout(Duration::from_secs(self.config.timeout))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(format!("JWKS request failed with status {}", resp.status()).into());
            }
            let jwks: JwkSet = resp.json().await?;
            if jwks.keys.is_empty() {
                return Err("JWKS endpoint returned empty keys".into());
            }
            Ok(jwks)
        }
        .await;

        let mut state = self.jwks_state.write().await;
        state.last_refresh_at = Some(now);

        match fetch_result {
            Ok(jwks) => {
                state.set = Some(jwks.clone());
                state.expires_at = Some(now + ttl);
                Ok(jwks)
            }
            Err(err) => {
                if let Some(ref cached) = state.set {
                    tracing::warn!(error = %err, "JWKS refresh failed, using stale cache");
                    Ok(cached.clone())
                } else {
                    Err(err)
                }
            }
        }
    }

    pub(super) async fn verify_jwt_token_internal(
        &self,
        token: &str,
        validate_scopes: bool,
        audience_override: Option<&[String]>,
        validate_exp: bool,
    ) -> VerifyResult<Value> {
        let header = decode_header(token).map_err(|_| (401, "Invalid token format".to_string()))?;
        let token_alg = header.alg;

        let (expected_alg, allowed_algs) = resolve_algorithm_policy(
            self.config.token_signing_alg.as_deref(),
            self.config.allowed_signing_algs.as_deref(),
        )?;
        validate_token_alg(token_alg, expected_alg, &allowed_algs)?;

        let mut jwks = self
            .get_or_fetch_jwks(false)
            .await
            .map_err(|e| (502, format!("Failed to fetch JWKS: {}", e)))?;

        let jwk = match select_jwk(&header, &jwks, token_alg) {
            Ok(jwk) => jwk,
            Err(JwkSelectError::KidNotFound) => {
                // kid miss: force one refresh (singleflight + min refresh interval in cache layer)
                jwks = self
                    .get_or_fetch_jwks(true)
                    .await
                    .map_err(|e| (502, format!("Failed to refresh JWKS on kid miss: {}", e)))?;
                select_jwk(&header, &jwks, token_alg)
                    .map_err(|_| (401, "No matching JWK found for token kid".to_string()))?
            }
            Err(JwkSelectError::NoSuitableKey) => {
                return Err((401, "No suitable JWK for token algorithm".to_string()));
            }
        };

        let decoding_key =
            DecodingKey::from_jwk(&jwk).map_err(|e| (401, format!("Failed to build decoding key: {}", e)))?;
        let discovery = self
            .get_or_fetch_discovery()
            .await
            .map_err(|e| (502, format!("Failed to fetch OIDC discovery document: {}", e)))?;

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

        let token_data = decode::<Claims>(token, &decoding_key, &validation).map_err(map_jwt_decode_error)?;
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

    pub(super) fn is_refresh_wait_timeout(err: &(u16, String)) -> bool {
        err.0 == 502 && err.1 == "Token refresh singleflight wait timeout"
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

    pub(super) fn should_auto_fallback_to_introspection(jwt_error: &(u16, String)) -> bool {
        let (status, message) = jwt_error;
        *status == 401 && message == "Invalid token format"
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

    pub(super) fn sanitize_redirect_target(original_url: &str) -> String {
        if original_url.starts_with('/') {
            original_url.to_string()
        } else {
            "/".to_string()
        }
    }

    pub(super) fn logout_redirect_target(&self) -> String {
        match self.config.post_logout_redirect_uri.as_deref() {
            Some(v) if !v.is_empty() => {
                if v.starts_with('/') || v.starts_with("http://") || v.starts_with("https://") {
                    v.to_string()
                } else {
                    "/".to_string()
                }
            }
            _ => "/".to_string(),
        }
    }

    pub(super) fn resolve_post_logout_redirect_uri_for_idp(&self, session: &dyn PluginSession) -> Option<String> {
        let target = self.logout_redirect_target();
        if target.starts_with("https://") || target.starts_with("http://") {
            return Some(target);
        }
        if !target.starts_with('/') {
            return None;
        }
        let host = Self::request_host(session)?;
        let scheme = Self::request_scheme(session);
        Some(format!("{}://{}{}", scheme, host, target))
    }

    pub(super) async fn build_logout_redirect_target(
        &self,
        fallback_target: String,
        post_logout_redirect_uri: Option<String>,
        id_token_hint: Option<String>,
    ) -> String {
        let end_session_endpoint = {
            let cached = self.discovery_doc.read().await;
            cached
                .as_ref()
                .and_then(|d| d.end_session_endpoint.clone())
                .filter(|v| !v.is_empty())
        };
        let Some(end_session_endpoint) = end_session_endpoint else {
            return fallback_target;
        };

        let mut end_session_url = match Url::parse(&end_session_endpoint) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    plugin = "OpenidConnect",
                    error = %e,
                    "Invalid end_session_endpoint URL; falling back to local logout redirect target"
                );
                return fallback_target;
            }
        };
        if let Some(post_logout_redirect_uri) = post_logout_redirect_uri {
            end_session_url
                .query_pairs_mut()
                .append_pair("post_logout_redirect_uri", &post_logout_redirect_uri);
        }
        if let Some(id_token_hint) = id_token_hint.filter(|v| !v.is_empty()) {
            end_session_url
                .query_pairs_mut()
                .append_pair("id_token_hint", &id_token_hint);
        }
        end_session_url.to_string()
    }

    pub(super) async fn cached_discovery_doc(&self) -> Option<DiscoveryDocument> {
        let cached = self.discovery_doc.read().await;
        cached.clone()
    }

    pub(super) fn refresh_lock_key(raw_cookie_value: &str) -> String {
        let digest = Sha256::digest(raw_cookie_value.as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
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
        let now = Instant::now();
        let mut cache = self.introspection_cache.write().await;
        let entry = cache.get(&key)?;
        if now < entry.expires_at {
            return Some(entry.claims.clone());
        }
        cache.remove(&key);
        None
    }

    pub(super) async fn cache_introspection_claims(&self, token: &str, claims: &Value) {
        if self.config.introspection_cache_ttl == 0 {
            return;
        }
        let key = Self::introspection_cache_key(token);
        let now = Instant::now();
        let entry = IntrospectionCacheEntry {
            claims: claims.clone(),
            expires_at: now + Duration::from_secs(self.config.introspection_cache_ttl),
        };
        let mut cache = self.introspection_cache.write().await;
        cache.retain(|_, v| now < v.expires_at);
        cache.insert(key, entry);
    }

    pub(super) fn generate_session_ref() -> String {
        let mut bytes = [0u8; 16];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub(super) async fn cache_access_token(&self, session_ref: &str, token: &str, expires_at: Option<u64>) {
        if session_ref.is_empty() || token.is_empty() {
            return;
        }
        let now = Self::now_unix_secs();
        let mut cache = self.access_token_cache.write().await;
        cache.retain(|_, entry| {
            !entry
                .expires_at
                .is_some_and(|exp| now > exp.saturating_add(self.config.clock_skew_seconds))
        });
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(
            session_ref.to_string(),
            AccessTokenCacheEntry {
                token: token.to_string(),
                expires_at,
            },
        );
    }

    pub(super) async fn get_cached_access_token(&self, session_ref: &str) -> Option<String> {
        if session_ref.is_empty() {
            return None;
        }
        let now = Self::now_unix_secs();
        let mut cache = self.access_token_cache.write().await;
        let entry = cache.get(session_ref)?;
        if entry
            .expires_at
            .is_some_and(|exp| now > exp.saturating_add(self.config.clock_skew_seconds))
        {
            cache.remove(session_ref);
            return None;
        }
        Some(entry.token.clone())
    }

    pub(super) async fn remove_cached_access_token(&self, session_ref: &str) {
        if session_ref.is_empty() {
            return;
        }
        let mut cache = self.access_token_cache.write().await;
        cache.remove(session_ref);
    }

    pub(super) async fn resolve_access_token_from_session(&self, session_cookie: &OidcSessionCookie) -> Option<String> {
        if !session_cookie.access_token.trim().is_empty() {
            return Some(session_cookie.access_token.clone());
        }
        self.get_cached_access_token(&session_cookie.session_ref).await
    }

    pub(super) fn extract_session_cookie_from_request(
        &self,
        session: &dyn PluginSession,
    ) -> VerifyResult<Option<(OidcSessionCookie, String)>> {
        let Some(cookie_value) = session.get_cookie(self.session_cookie_name()) else {
            return Ok(None);
        };
        let session_secret = self
            .resolve_session_secret()
            .map_err(|e| (401, format!("Missing session secret: {}", e)))?;
        let payload: OidcSessionCookie = self.decode_signed_cookie_payload(&cookie_value, &session_secret)?;
        Ok(Some((payload, cookie_value)))
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
            Err(e) => {
                tracing::warn!(plugin = "OpenidConnect", error = %e, "Failed to resolve session secret");
                return;
            }
        };
        let value = match self.encode_signed_cookie_payload(&persisted_payload, &session_secret) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(plugin = "OpenidConnect", error = %e, "Failed to encode session cookie");
                return;
            }
        };
        let cookie = format!(
            "{}={}{}",
            self.session_cookie_name(),
            value,
            self.build_cookie_attr_suffix(session, self.config.session_lifetime, "/")
        );
        if let Err(e) = self.ensure_session_cookie_size_limit(&cookie) {
            tracing::warn!(
                plugin = "OpenidConnect",
                error = %e,
                "Skipped updating session cookie due to size limit"
            );
            return;
        }
        let _ = session.set_response_header("Set-Cookie", &cookie);
    }

    pub(super) fn ensure_session_cookie_size_limit(&self, cookie_header_value: &str) -> Result<(), String> {
        let max = self.config.max_session_cookie_bytes as usize;
        if cookie_header_value.len() > max {
            return Err(format!(
                "Session cookie exceeds maxSessionCookieBytes: {} > {}",
                cookie_header_value.len(),
                max
            ));
        }
        Ok(())
    }

    pub(super) async fn exchange_code_for_token(
        &self,
        discovery: &DiscoveryDocument,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> VerifyResult<TokenEndpointResponse> {
        let token_endpoint = discovery
            .token_endpoint
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or((502, "OIDC discovery document missing token_endpoint".to_string()))?;
        let client_secret = self
            .resolve_client_secret()
            .map_err(|e| (401, format!("Missing client secret for token exchange: {}", e)))?;

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

        let resp = request
            .send()
            .await
            .map_err(|e| (502, format!("Token exchange request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err((
                502,
                format!("Token exchange request failed with status {}", resp.status()),
            ));
        }
        resp.json::<TokenEndpointResponse>()
            .await
            .map_err(|e| (502, format!("Invalid token endpoint response: {}", e)))
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
            None => {
                tracing::warn!(
                    plugin = "OpenidConnect",
                    "No cached discovery document for token revocation; skip token revocation"
                );
                return;
            }
        };
        let Some(revocation_endpoint) = discovery.revocation_endpoint.as_deref().filter(|v| !v.is_empty()) else {
            tracing::warn!(
                plugin = "OpenidConnect",
                "Discovery document missing revocation_endpoint; skip token revocation"
            );
            return;
        };

        let client_secret = match self.resolve_client_secret() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    plugin = "OpenidConnect",
                    error = %e,
                    "Missing client secret for token revocation"
                );
                return;
            }
        };

        if let Some(token) = refresh_token {
            if let Err(e) = self
                .revoke_token(
                    revocation_endpoint,
                    client_secret.as_str(),
                    token,
                    Some("refresh_token"),
                )
                .await
            {
                tracing::warn!(
                    plugin = "OpenidConnect",
                    error = %e,
                    token_type = "refresh_token",
                    "OIDC token revocation failed"
                );
            }
        }

        if let Some(token) = access_token {
            if let Err(e) = self
                .revoke_token(revocation_endpoint, client_secret.as_str(), token, Some("access_token"))
                .await
            {
                tracing::warn!(
                    plugin = "OpenidConnect",
                    error = %e,
                    token_type = "access_token",
                    "OIDC token revocation failed"
                );
            }
        }
    }

    pub(super) async fn refresh_access_token(
        &self,
        discovery: &DiscoveryDocument,
        refresh_token: &str,
    ) -> VerifyResult<TokenEndpointResponse> {
        let token_endpoint = discovery
            .token_endpoint
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or((502, "OIDC discovery document missing token_endpoint".to_string()))?;
        let client_secret = self
            .resolve_client_secret()
            .map_err(|e| (401, format!("Missing client secret for token refresh: {}", e)))?;

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

        let resp = request
            .send()
            .await
            .map_err(|e| (502, format!("Token refresh request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err((
                502,
                format!("Token refresh request failed with status {}", resp.status()),
            ));
        }
        resp.json::<TokenEndpointResponse>()
            .await
            .map_err(|e| (502, format!("Invalid token refresh response: {}", e)))
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
        let refresh_token = session_cookie
            .refresh_token
            .as_deref()
            .ok_or((401, "Missing refresh token".to_string()))?;
        let discovery = self
            .get_or_fetch_discovery()
            .await
            .map_err(|e| (502, format!("Failed to fetch OIDC discovery document: {}", e)))?;
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
        let mut locks = self.refresh_singleflight_locks.lock().await;
        if locks.len() > 4096 {
            locks.clear();
        }
        locks
            .entry(lock_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub(super) async fn try_get_recent_refresh_result(&self, lock_key: &str) -> Option<OidcSessionCookie> {
        let mut results = self.refresh_singleflight_results.lock().await;
        let entry = results.get(lock_key)?.clone();
        if entry.at.elapsed() > Duration::from_secs(5) {
            results.remove(lock_key);
            return None;
        }
        Some(entry.payload.clone())
    }

    pub(super) async fn put_refresh_result(&self, lock_key: &str, payload: OidcSessionCookie) {
        let mut results = self.refresh_singleflight_results.lock().await;
        if results.len() > 4096 {
            results.clear();
        }
        results.insert(
            lock_key.to_string(),
            RefreshSingleflightResult {
                payload,
                at: Instant::now(),
            },
        );
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
                return Err((502, "Token refresh singleflight wait timeout".to_string()));
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

    pub(super) async fn validate_id_token_nonce(&self, id_token: &str, expected_nonce: &str) -> VerifyResult<()> {
        let id_token_audience = vec![self.config.client_id.clone()];
        let claims = self
            .verify_jwt_token_internal(id_token, false, Some(&id_token_audience), true)
            .await?;
        let actual_nonce = claims.get("nonce").and_then(|v| v.as_str());
        if actual_nonce != Some(expected_nonce) {
            return Err((401, "Invalid ID token nonce".to_string()));
        }
        Ok(())
    }

    pub(super) async fn handle_callback(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        code: &str,
        state_param: &str,
    ) -> PluginRunningResult {
        let session_secret = match self.resolve_session_secret() {
            Ok(v) => v,
            Err(e) => {
                let _ = self
                    .send_unauthorized(session, &format!("Unauthorized - Missing session secret: {}", e))
                    .await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        let Some(raw_state_cookie) = session.get_cookie(Self::state_cookie_name()) else {
            let _ = self
                .send_unauthorized(session, "Unauthorized - Missing OIDC state cookie")
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        };
        let state_cookie: AuthorizationStateCookie =
            match self.decode_signed_cookie_payload(&raw_state_cookie, &session_secret) {
                Ok(v) => v,
                Err((status, message)) => {
                    if status == 502 {
                        let _ = self.send_plain_error(session, 502, &message).await;
                    } else {
                        let _ = self.send_unauthorized(session, &message).await;
                    }
                    return PluginRunningResult::ErrTerminateRequest;
                }
            };

        let now = Self::now_unix_secs();
        if now.saturating_sub(state_cookie.created_at) > 300 {
            let _ = self
                .send_unauthorized(session, "Unauthorized - OIDC state expired")
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        }
        if state_cookie.state != state_param {
            let _ = self.send_plain_error(session, 400, "Invalid OIDC callback state").await;
            return PluginRunningResult::ErrTerminateRequest;
        }

        let discovery = match self.get_or_fetch_discovery().await {
            Ok(d) => d,
            Err(e) => {
                let _ = self
                    .send_plain_error(session, 502, &format!("Failed to fetch OIDC discovery document: {}", e))
                    .await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        let redirect_uri = match self.resolve_redirect_uri(session) {
            Ok(v) => v,
            Err((status, message)) => {
                if status == 502 {
                    let _ = self.send_plain_error(session, 502, &message).await;
                } else {
                    let _ = self.send_unauthorized(session, &message).await;
                }
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        let token_response = match self
            .exchange_code_for_token(&discovery, code, &redirect_uri, state_cookie.code_verifier.as_deref())
            .await
        {
            Ok(v) => v,
            Err((status, message)) => {
                if status == 502 {
                    let _ = self.send_plain_error(session, 502, &message).await;
                } else {
                    let _ = self.send_unauthorized(session, &message).await;
                }
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        if self.config.use_nonce {
            let expected_nonce = match state_cookie.nonce.as_deref() {
                Some(v) if !v.is_empty() => v,
                _ => {
                    let _ = self.send_plain_error(session, 400, "Invalid OIDC callback nonce").await;
                    return PluginRunningResult::ErrTerminateRequest;
                }
            };
            let id_token = match token_response.id_token.as_deref() {
                Some(v) if !v.is_empty() => v,
                _ => {
                    let _ = self.send_unauthorized(session, "Unauthorized - Missing ID token").await;
                    return PluginRunningResult::ErrTerminateRequest;
                }
            };
            if let Err((status, message)) = self.validate_id_token_nonce(id_token, expected_nonce).await {
                if status == 502 {
                    let _ = self.send_plain_error(session, 502, &message).await;
                } else {
                    let _ = self.send_unauthorized(session, &message).await;
                }
                return PluginRunningResult::ErrTerminateRequest;
            }
        }

        let _claims = match self.verify_token(&token_response.access_token).await {
            Ok(v) => v,
            Err((status, message)) => {
                if status == 502 {
                    let _ = self.send_plain_error(session, 502, &message).await;
                } else {
                    let _ = self.send_unauthorized(session, &message).await;
                }
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        let expires_at = token_response.expires_in.map(|ttl| now.saturating_add(ttl));
        let should_persist_id_token = self.should_persist_id_token(&discovery);
        let mut session_cookie_payload = OidcSessionCookie {
            session_ref: Self::generate_session_ref(),
            access_token: token_response.access_token,
            created_at: now,
            expires_at,
            id_token: if should_persist_id_token {
                token_response.id_token.filter(|v| !v.is_empty())
            } else {
                None
            },
            refresh_token: token_response.refresh_token,
        };
        self.cache_access_token(
            &session_cookie_payload.session_ref,
            &session_cookie_payload.access_token,
            session_cookie_payload.expires_at,
        )
        .await;
        session_cookie_payload.access_token.clear();

        let session_cookie_value = match self.encode_signed_cookie_payload(&session_cookie_payload, &session_secret) {
            Ok(v) => v,
            Err(e) => {
                let _ = self
                    .send_plain_error(session, 502, &format!("Failed to encode session cookie: {}", e))
                    .await;
                return PluginRunningResult::ErrTerminateRequest;
            }
        };

        let session_cookie = format!(
            "{}={}{}",
            self.session_cookie_name(),
            session_cookie_value,
            self.build_cookie_attr_suffix(session, self.config.session_lifetime, "/")
        );
        if let Err(e) = self.ensure_session_cookie_size_limit(&session_cookie) {
            tracing::warn!(
                plugin = "OpenidConnect",
                error = %e,
                "OIDC callback aborted due to oversized session cookie"
            );
            let _ = self
                .send_plain_error(session, 500, "Session cookie exceeds configured size limit")
                .await;
            return PluginRunningResult::ErrTerminateRequest;
        }
        let clear_state_cookie = format!(
            "{}={}{}",
            Self::state_cookie_name(),
            "",
            self.build_cookie_attr_suffix(session, 0, &self.callback_path())
        );
        let redirect_target = Self::sanitize_redirect_target(&state_cookie.original_url);
        let cookies = vec![session_cookie, clear_state_cookie];
        let _ = self
            .send_redirect_with_cookies(session, &redirect_target, &cookies)
            .await;

        plugin_log.push("OIDC callback exchanged code and established session; ");
        PluginRunningResult::ErrTerminateRequest
    }
}
