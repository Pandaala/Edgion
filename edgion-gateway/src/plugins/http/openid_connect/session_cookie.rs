use std::fmt::Write as _;

use super::*;
use crate::plugins::http::common::auth_common::strip_cookies;
use edgion_common::secret::get_secret;
use edgion_resources::resources::gateway::SecretObjectReference;

impl<S: AsRef<str>> OidcSessionCookie<S> {
    /// Return only the remaining absolute session lifetime, never token lifetime.
    pub(super) fn remaining_lifetime(&self, now: u64) -> VerifyResult<u64> {
        if self.created_at == 0 || self.created_at > now || self.session_expires_at <= self.created_at {
            return Err(OpenidConnect::credential_failure());
        }
        self.session_expires_at
            .checked_sub(now)
            .filter(|remaining| *remaining > 0)
            .ok_or_else(OpenidConnect::credential_failure)
    }
}

impl OpenidConnect {
    pub(super) fn session_now(&self) -> u64 {
        #[cfg(test)]
        if let Some(clock) = &self.session_clock {
            return clock.load(std::sync::atomic::Ordering::SeqCst);
        }
        Self::now_unix_secs()
    }

    pub(super) fn session_deadline(created_at: u64, lifetime: u64) -> VerifyResult<u64> {
        created_at
            .checked_add(lifetime)
            .filter(|deadline| created_at > 0 && *deadline > created_at)
            .ok_or_else(Self::credential_failure)
    }

    pub(super) fn random_urlsafe<const N: usize>() -> String {
        let mut bytes = [0u8; N];
        rand::rng().fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn resolve_secret_once(
        resolved: Option<String>,
        secret_ref: Option<&SecretObjectReference>,
        plugin_namespace: &str,
        keys: &[&str],
    ) -> Option<String> {
        if let Some(resolved) = resolved.filter(|value| !value.is_empty()) {
            return Some(resolved);
        }

        let secret_ref = secret_ref?;
        let namespace = secret_ref.namespace.as_deref().unwrap_or(plugin_namespace);
        let mut secret = get_secret(Some(namespace), &secret_ref.name)?;
        let mut data = secret.data.take()?;
        let value = keys.iter().find_map(|key| data.remove(*key))?;
        let value = String::from_utf8(value.0).ok()?;
        (!value.trim().is_empty()).then_some(value)
    }

    pub(super) fn prepare_secrets(config: &mut OpenidConnectConfig, plugin_namespace: &str) -> PreparedOidcSecrets {
        let client_secret = Self::resolve_secret_once(
            config.resolved_oidc_client_secret.take(),
            config.client_secret_ref.as_ref(),
            plugin_namespace,
            &["clientSecret", "client_secret", "secret"],
        )
        .map_or(PreparedValue::Unavailable, PreparedValue::Ready);

        let session_cipher = Self::resolve_secret_once(
            config.resolved_session_secret.take(),
            config.session_secret_ref.as_ref(),
            plugin_namespace,
            &["sessionSecret", "session_secret", "secret"],
        )
        .and_then(|secret| Self::session_cipher_from_secret(&secret))
        .map_or(PreparedValue::Unavailable, PreparedValue::Ready);

        PreparedOidcSecrets {
            client_secret,
            session_cipher,
        }
    }

    fn session_cipher_from_secret(secret: &str) -> Option<Aes256Gcm> {
        if secret.len() < 32 {
            return None;
        }
        let digest = Sha256::digest(secret.as_bytes());
        Aes256Gcm::new_from_slice(&digest).ok()
    }

    pub(super) fn client_secret(&self) -> Option<&str> {
        match &self.prepared_secrets.client_secret {
            PreparedValue::Ready(value) => Some(value),
            PreparedValue::Unavailable => None,
        }
    }

    pub(super) fn session_cipher(&self) -> Option<&Aes256Gcm> {
        match &self.prepared_secrets.session_cipher {
            PreparedValue::Ready(cipher) => Some(cipher),
            PreparedValue::Unavailable => None,
        }
    }

    pub(super) fn credential_failure() -> OidcVerifyError {
        OidcVerifyError::Auth(AuthFailure::Credentials)
    }

    pub(super) fn encode_signed_cookie_payload<T: Serialize>(value: &T, cipher: &Aes256Gcm) -> Result<String, ()> {
        let payload = serde_json::to_vec(value).map_err(|_| ())?;
        let mut nonce_bytes = [0u8; 12];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let encrypted = cipher.encrypt(nonce, payload.as_ref()).map_err(|_| ())?;
        drop(payload);
        Self::encode_cookie_frame(&nonce_bytes, &encrypted)
    }

    pub(super) fn encode_cookie_frame(nonce: &[u8; 12], encrypted: &[u8]) -> Result<String, ()> {
        let nonce_len = base64::encoded_len(nonce.len(), false).ok_or(())?;
        let encrypted_len = base64::encoded_len(encrypted.len(), false).ok_or(())?;
        let len = nonce_len
            .checked_add(1)
            .and_then(|len| len.checked_add(encrypted_len))
            .ok_or(())?;
        let mut output = vec![0; len];
        let (nonce_output, tail) = output.split_at_mut(nonce_len);
        let (separator, encrypted_output) = tail.split_at_mut(1);
        separator[0] = b'.';
        URL_SAFE_NO_PAD.encode_slice(nonce, nonce_output).map_err(|_| ())?;
        URL_SAFE_NO_PAD
            .encode_slice(encrypted, encrypted_output)
            .map_err(|_| ())?;
        String::from_utf8(output).map_err(|_| ())
    }

    pub(super) fn decode_signed_cookie_payload<T: for<'de> Deserialize<'de>>(
        value: &str,
        cipher: &Aes256Gcm,
    ) -> VerifyResult<T> {
        let (nonce_b64, encrypted_b64) = value
            .split_once('.')
            .ok_or(OidcVerifyError::Auth(AuthFailure::MalformedCookie))?;
        let mut nonce = [0u8; 12];
        let nonce_len = URL_SAFE_NO_PAD
            .decode_slice(nonce_b64, &mut nonce)
            .map_err(|_| OidcVerifyError::Auth(AuthFailure::MalformedCookie))?;
        if nonce_len != nonce.len() {
            return Err(OidcVerifyError::Auth(AuthFailure::MalformedCookie));
        }
        let encrypted = URL_SAFE_NO_PAD
            .decode(encrypted_b64)
            .map_err(|_| OidcVerifyError::Auth(AuthFailure::MalformedCookie))?;
        let nonce = Nonce::from_slice(&nonce);
        let payload = cipher
            .decrypt(nonce, encrypted.as_ref())
            .map_err(|_| OidcVerifyError::Auth(AuthFailure::InvalidCookieSignature))?;
        drop(encrypted);
        serde_json::from_slice::<T>(&payload).map_err(|_| OidcVerifyError::Auth(AuthFailure::MalformedCookie))
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

    pub(super) fn build_cookie_header_value(&self, name: &str, value: &str, max_age: u64, path: &str) -> String {
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
        let max_age_len = max_age.checked_ilog10().map_or(1, |digits| digits as usize + 1);
        let capacity = [
            name.len(),
            value.len(),
            path.len(),
            same_site.len(),
            max_age_len,
            http_only_attr.len(),
            secure_attr.len(),
            "=; Path=; SameSite=; Max-Age=".len(),
        ]
        .into_iter()
        .try_fold(0usize, usize::checked_add)
        // An unrepresentable length falls back to normal String growth.
        .unwrap_or(0);
        let mut cookie = String::with_capacity(capacity);
        // String writes and the standard str/u64 formatters cannot return fmt::Error.
        let _ = write!(
            cookie,
            "{}={}; Path={}; SameSite={}; Max-Age={}{}{}",
            name, value, path, same_site, max_age, http_only_attr, secure_attr
        );
        cookie
    }

    pub(super) fn generate_session_ref() -> String {
        let mut bytes = [0u8; 16];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    /// Remove Gateway-owned OIDC cookies from the proxied request.
    ///
    /// `edgion_oidc_session` (or `sessionCookieName`) and `edgion_oidc_state` are
    /// Gateway credentials, not application cookies. They are stripped on every
    /// path that continues upstream, independent of `hideCredentials`.
    pub(super) fn strip_gateway_owned_cookies(
        &self,
        session: &mut dyn PluginSession,
    ) -> Result<(), crate::request_cookie::CookieEditError> {
        // One plan, one rewrite: stripping the two names separately would scan
        // and rewrite the Cookie header twice for the same end state.
        strip_cookies(session, &[self.session_cookie_name(), Self::state_cookie_name()])?;
        Ok(())
    }

    pub(super) fn extract_session_cookie_from_request(
        &self,
        session: &dyn PluginSession,
    ) -> VerifyResult<Option<(OidcSessionCookie, String)>> {
        let Some(cookie_value) = session.get_cookie(self.session_cookie_name()) else {
            return Ok(None);
        };
        let cipher = self.session_cipher().ok_or_else(Self::credential_failure)?;
        let payload: OidcSessionCookie = Self::decode_signed_cookie_payload(&cookie_value, cipher)?;
        Ok(Some((payload, cookie_value)))
    }

    pub(super) fn ensure_session_cookie_size_limit(&self, cookie_header_value: &str) -> Result<(), ()> {
        let max = self.config.max_session_cookie_bytes as usize;
        if cookie_header_value.len() > max {
            return Err(());
        }
        Ok(())
    }
}
