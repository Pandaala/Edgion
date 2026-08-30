use super::*;
use edgion_common::secret::get_secret;

impl OpenidConnect {
    pub(super) fn random_urlsafe(bytes_len: usize) -> String {
        let mut bytes = vec![0u8; bytes_len];
        rand::rng().fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub(super) fn resolve_client_secret(&self) -> OidcResult<String> {
        if let Some(ref resolved) = self.config.resolved_oidc_client_secret {
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
        if secret.len() < 32 {
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
        let cipher = Self::session_cipher(secret).map_err(|e| OidcVerifyError::Other {
            status: 502,
            message: format!("Failed to verify OIDC cookie: {}", e),
        })?;
        let (nonce_b64, encrypted_b64) = value.split_once('.').ok_or(OidcVerifyError::Other {
            status: 401,
            message: "Malformed OIDC cookie payload".to_string(),
        })?;
        let nonce = URL_SAFE_NO_PAD.decode(nonce_b64).map_err(|_| OidcVerifyError::Other {
            status: 401,
            message: "Malformed OIDC cookie payload".to_string(),
        })?;
        if nonce.len() != 12 {
            return Err(OidcVerifyError::Other {
                status: 401,
                message: "Malformed OIDC cookie payload".to_string(),
            });
        }
        let encrypted = URL_SAFE_NO_PAD
            .decode(encrypted_b64)
            .map_err(|_| OidcVerifyError::Other {
                status: 401,
                message: "Malformed OIDC cookie payload".to_string(),
            })?;
        let nonce = Nonce::from_slice(&nonce);
        let payload = cipher
            .decrypt(nonce, encrypted.as_ref())
            .map_err(|_| OidcVerifyError::Other {
                status: 401,
                message: "Invalid OIDC cookie signature".to_string(),
            })?;
        serde_json::from_slice::<T>(&payload).map_err(|_| OidcVerifyError::Other {
            status: 401,
            message: "Malformed OIDC cookie payload".to_string(),
        })
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

    pub(super) fn generate_session_ref() -> String {
        let mut bytes = [0u8; 16];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub(super) fn extract_session_cookie_from_request(
        &self,
        session: &dyn PluginSession,
    ) -> VerifyResult<Option<(OidcSessionCookie, String)>> {
        let Some(cookie_value) = session.get_cookie(self.session_cookie_name()) else {
            return Ok(None);
        };
        let session_secret = self.resolve_session_secret().map_err(|e| OidcVerifyError::Other {
            status: 401,
            message: format!("Missing session secret: {}", e),
        })?;
        let payload: OidcSessionCookie = self.decode_signed_cookie_payload(&cookie_value, &session_secret)?;
        Ok(Some((payload, cookie_value)))
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
}
