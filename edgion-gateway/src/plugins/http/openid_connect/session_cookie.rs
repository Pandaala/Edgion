use std::fmt::Write as _;

use super::*;
use crate::plugins::http::common::auth_common::strip_cookies;
use edgion_common::secret::get_secret;
use edgion_resources::resources::edgion_plugins::MAX_SESSION_COOKIE_CHUNKS;
use edgion_resources::resources::gateway::SecretObjectReference;

/// Hard reader cap for session chunk cookies, independent of the configured write limit.
pub(super) const SESSION_COOKIE_MAX_CHUNKS: usize = MAX_SESSION_COOKIE_CHUNKS as usize;
/// Suffix of the chunk-count cookie; also the longest derived-name suffix (`_0`..`_15`).
const SESSION_COOKIE_COUNT_SUFFIX: &str = "_chunks";
/// Single + count + chunks + state.
const GATEWAY_OWNED_COOKIE_COUNT: usize = SESSION_COOKIE_MAX_CHUNKS + 3;

/// Session cookie names derived once per plugin generation (Envoy chunk naming:
/// `<name>_0 .. <name>_{N-1}` plus `<name>_chunks = N`).
pub(super) struct SessionCookieNames {
    count: String,
    chunks: [String; SESSION_COOKIE_MAX_CHUNKS],
}

impl SessionCookieNames {
    pub(super) fn new(base: &str) -> Self {
        Self {
            count: format!("{base}{SESSION_COOKIE_COUNT_SUFFIX}"),
            chunks: std::array::from_fn(|index| format!("{base}_{index}")),
        }
    }
}

/// The raw sealed session value read from the request.
pub(super) struct SessionCookieValue {
    /// The single cookie value or the reassembled chunk value.
    pub(super) raw: String,
    /// Validated chunk count of the request (0 for a single cookie), used to expire stale chunks.
    pub(super) prior_chunks: usize,
}

impl OidcSessionCookie {
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
            OpenidConnectConfig::CLIENT_SECRET_KEYS,
        )
        .map_or(PreparedValue::Unavailable, PreparedValue::Ready);

        let session_cipher = Self::resolve_secret_once(
            config.resolved_session_secret.take(),
            config.session_secret_ref.as_ref(),
            plugin_namespace,
            OpenidConnectConfig::SESSION_SECRET_KEYS,
        )
        .and_then(|secret| Self::session_cipher_from_secret(&secret))
        .map_or(PreparedValue::Unavailable, PreparedValue::Ready);

        PreparedOidcSecrets {
            client_secret,
            session_cipher,
        }
    }

    fn session_cipher_from_secret(secret: &str) -> Option<Aes256Gcm> {
        if secret.len() < OpenidConnectConfig::MIN_SESSION_SECRET_LEN {
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

    /// Remove Gateway-owned OIDC cookies from the proxied request.
    ///
    /// The session cookie (`sessionCookieName`), its chunk cookies (`<name>_chunks`,
    /// `<name>_0` .. `<name>_15`) and `edgion_oidc_state` are Gateway credentials, not
    /// application cookies. They are stripped on every path that continues upstream,
    /// independent of `hideCredentials`.
    pub(super) fn strip_gateway_owned_cookies(
        &self,
        session: &mut dyn PluginSession,
    ) -> Result<(), crate::request_cookie::CookieEditError> {
        // One plan, one rewrite: every Gateway-owned name (single, chunk count,
        // all chunk slots and the state cookie) is removed in a single edit.
        let names = &self.session_cookie_names;
        let mut owned = [""; GATEWAY_OWNED_COOKIE_COUNT];
        owned[0] = self.session_cookie_name();
        owned[1] = Self::state_cookie_name();
        owned[2] = &names.count;
        for (slot, chunk) in owned[3..].iter_mut().zip(names.chunks.iter()) {
            *slot = chunk;
        }
        strip_cookies(session, &owned)?;
        Ok(())
    }

    pub(super) fn extract_session_cookie_from_request(
        &self,
        session: &dyn PluginSession,
    ) -> VerifyResult<Option<(OidcSessionCookie, SessionCookieValue)>> {
        let Some(value) = self.read_session_cookie_value(session)? else {
            return Ok(None);
        };
        let cipher = self.session_cipher().ok_or_else(Self::credential_failure)?;
        let payload: OidcSessionCookie = Self::decode_signed_cookie_payload(&value.raw, cipher)?;
        Ok(Some((payload, value)))
    }

    /// Parsed chunk count on the request, clamped to the reader cap (0 when absent or invalid).
    pub(super) fn prior_session_chunks(&self, session: &dyn PluginSession) -> usize {
        Self::present_cookie(session, &self.session_cookie_names.count)
            .and_then(Self::parse_chunk_count)
            .map_or(0, |count| count.min(SESSION_COOKIE_MAX_CHUNKS))
    }

    fn parse_chunk_count(raw: &str) -> Option<usize> {
        raw.parse::<usize>().ok()
    }

    /// A shape-selecting cookie with an empty value counts as absent: some cookie jars keep
    /// expired cookies as `name=` instead of deleting them.
    fn present_cookie<'s>(session: &'s dyn PluginSession, name: &str) -> Option<&'s str> {
        session.get_cookie_ref(name).filter(|value| !value.is_empty())
    }

    /// Read the sealed value.
    ///
    /// Without a count cookie the single cookie is the session. A count cookie selects the
    /// chunked shape exclusively: it must be 2..=16 with every chunk present, a reassembled
    /// length within 16 x `maxSessionCookieBytes`, and no single cookie alongside. Anything
    /// else fails closed instead of falling back, because the derived names are not covered
    /// by the browser's Secure-cookie overwrite protection for the single name, so a planted
    /// chunk or count cookie must never select which session authenticates.
    pub(super) fn read_session_cookie_value(
        &self,
        session: &dyn PluginSession,
    ) -> VerifyResult<Option<SessionCookieValue>> {
        let single = Self::present_cookie(session, self.session_cookie_name());
        let Some(count_cookie) = Self::present_cookie(session, &self.session_cookie_names.count) else {
            return Ok(single.map(|single| SessionCookieValue {
                raw: single.to_owned(),
                prior_chunks: 0,
            }));
        };
        let malformed = || OidcVerifyError::Auth(AuthFailure::MalformedCookie);
        if single.is_some() {
            return Err(malformed());
        }
        let count = Self::parse_chunk_count(count_cookie)
            .filter(|count| (2..=SESSION_COOKIE_MAX_CHUNKS).contains(count))
            .ok_or_else(malformed)?;
        let raw = self.reassemble_session_chunks(session, count).ok_or_else(malformed)?;
        Ok(Some(SessionCookieValue {
            raw,
            prior_chunks: count,
        }))
    }

    fn reassemble_session_chunks(&self, session: &dyn PluginSession, count: usize) -> Option<String> {
        let max_total = usize::try_from(self.config.max_session_cookie_bytes)
            .unwrap_or(usize::MAX)
            .saturating_mul(SESSION_COOKIE_MAX_CHUNKS);
        let mut parts = [""; SESSION_COOKIE_MAX_CHUNKS];
        let mut total = 0usize;
        for (part, name) in parts
            .iter_mut()
            .zip(self.session_cookie_names.chunks.iter())
            .take(count)
        {
            *part = session.get_cookie_ref(name)?;
            total = total.checked_add(part.len()).filter(|total| *total <= max_total)?;
        }
        let mut raw = String::with_capacity(total);
        for part in &parts[..count] {
            raw.push_str(part);
        }
        Some(raw)
    }

    /// Build the Set-Cookie lines for one sealed session value.
    ///
    /// The single cookie is used when it fits `maxSessionCookieBytes`; otherwise the value is
    /// split into chunk cookies plus a count cookie. Every write expires the opposite shape so
    /// the last applied response wins, and expires stale chunk indexes the request carried.
    pub(super) fn build_session_set_cookies(
        &self,
        sealed: &str,
        max_age: u64,
        prior_chunks: usize,
    ) -> Result<Vec<String>, ()> {
        let name = self.session_cookie_name();
        let names = &self.session_cookie_names;
        let max = usize::try_from(self.config.max_session_cookie_bytes).unwrap_or(usize::MAX);
        let prior = prior_chunks.min(SESSION_COOKIE_MAX_CHUNKS);
        let single = self.build_cookie_header_value(name, sealed, max_age, "/");
        if single.len() <= max {
            let mut cookies = Vec::with_capacity(2 + prior);
            cookies.push(single);
            cookies.push(self.expired_cookie(&names.count));
            cookies.extend(names.chunks[..prior].iter().map(|chunk| self.expired_cookie(chunk)));
            return Ok(cookies);
        }

        // `_chunks` is the longest derived suffix, so this overhead bounds every chunk cookie
        // and the count cookie (whose value has at most two digits).
        let overhead = single.len() - sealed.len() + SESSION_COOKIE_COUNT_SUFFIX.len();
        let capacity = max.checked_sub(overhead).filter(|capacity| *capacity >= 2).ok_or(())?;
        let count = sealed.len().div_ceil(capacity);
        let configured = usize::try_from(self.config.max_session_cookie_chunks).unwrap_or(0);
        if count > configured.min(SESSION_COOKIE_MAX_CHUNKS) {
            return Err(());
        }
        let mut cookies = Vec::with_capacity(count + 2 + prior.saturating_sub(count));
        for (index, chunk_name) in names.chunks[..count].iter().enumerate() {
            let start = index * capacity;
            let end = sealed.len().min(start + capacity);
            let part = sealed.get(start..end).ok_or(())?;
            cookies.push(self.build_cookie_header_value(chunk_name, part, max_age, "/"));
        }
        cookies.push(self.build_cookie_header_value(&names.count, &count.to_string(), max_age, "/"));
        cookies.push(self.expired_cookie(name));
        if prior > count {
            cookies.extend(
                names.chunks[count..prior]
                    .iter()
                    .map(|chunk| self.expired_cookie(chunk)),
            );
        }
        Ok(cookies)
    }

    /// Expire the single cookie, the count cookie and every chunk slot.
    pub(super) fn expired_session_cookies(&self) -> Vec<String> {
        let names = &self.session_cookie_names;
        let mut cookies = Vec::with_capacity(SESSION_COOKIE_MAX_CHUNKS + 2);
        cookies.push(self.expired_cookie(self.session_cookie_name()));
        cookies.push(self.expired_cookie(&names.count));
        cookies.extend(names.chunks.iter().map(|chunk| self.expired_cookie(chunk)));
        cookies
    }

    fn expired_cookie(&self, name: &str) -> String {
        self.build_cookie_header_value(name, "", 0, "/")
    }
}
