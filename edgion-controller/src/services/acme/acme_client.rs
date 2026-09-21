//! ACME Protocol Client
//!
//! Wraps `instant-acme` to provide certificate ordering, challenge handling,
//! and certificate retrieval for the Edgion ACME system.
//!
//! Uses the builder-style API of instant-acme 0.8.x:
//! ```text
//! Account::builder()?.create(...) -> (Account, AccountCredentials)
//! account.new_order(...)          -> Order
//! order.authorizations().next()   -> AuthorizationHandle
//! auth.challenge(type)            -> ChallengeHandle
//! challenge.key_authorization()   -> KeyAuthorization
//! challenge.set_ready()           -> notify server
//! order.poll_ready(...)           -> wait for ready
//! order.finalize_csr(csr_der)     -> submit CSR
//! order.poll_certificate(...)     -> get certificate
//! ```

use anyhow::{Context, Result};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, ExternalAccountKey, Identifier, NewAccount,
    NewOrder,
};
use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use zeroize::Zeroizing;

use edgion_resources::resources::edgion_acme::AcmeKeyType;

/// Attempt-local authority and cancellation seam for irreversible ACME calls.
#[async_trait::async_trait]
pub trait AcmeOperationGuard: Send + Sync {
    async fn check(&self, operation: &'static str) -> Result<()>;
    async fn cancelled(&self);
}

/// ACME client wrapper
pub struct AcmeClient {
    account: Account,
}

/// Result of an ACME certificate order
pub struct AcmeCertificateResult {
    /// PEM-encoded certificate chain
    pub certificate_pem: String,
    /// PEM-encoded private key (zeroized on drop)
    pub private_key_pem: Zeroizing<String>,
}

/// Pending HTTP-01 challenge information
pub struct PendingHttpChallenge {
    /// The domain being validated
    pub domain: String,
    /// Challenge token (appears in the URL path: /.well-known/acme-challenge/{token})
    pub token: String,
    /// Key authorization (returned as the HTTP response body)
    pub key_authorization: String,
}

impl AcmeClient {
    /// Create a new ACME client with a new account registration
    pub async fn new(
        server_url: &str,
        email: &str,
        eab_kid: Option<&str>,
        eab_hmac_key: Option<&str>,
        guard: &impl AcmeOperationGuard,
    ) -> Result<(Self, AccountCredentials)> {
        let new_account = NewAccount {
            contact: &[&format!("mailto:{}", email)],
            terms_of_service_agreed: true,
            only_return_existing: false,
        };

        // Build external account key if provided
        let eab = match (eab_kid, eab_hmac_key) {
            (Some(kid), Some(hmac_b64)) => {
                let hmac_bytes = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, hmac_b64)
                    .or_else(|_| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, hmac_b64))
                    .context("Failed to decode EAB HMAC key from base64")?;
                Some(ExternalAccountKey::new(kid.to_string(), &hmac_bytes))
            }
            _ => None,
        };

        let builder = Account::builder().context("Failed to create ACME account builder")?;

        guard.check("register ACME account").await?;
        let create = builder.create(&new_account, server_url.to_string(), eab.as_ref());
        let (account, credentials) = tokio::select! {
            result = create => result.context("Failed to create ACME account")?,
            _ = guard.cancelled() => anyhow::bail!("ACME account registration was cancelled"),
        };

        tracing::info!(
            server = server_url,
            email = email,
            account_id = %account.id(),
            "ACME account created"
        );

        Ok((Self { account }, credentials))
    }

    /// Create a new ACME client with a custom root CA certificate (for testing with Pebble).
    ///
    /// The `ca_pem_path` should point to the PEM file of the test CA.
    pub async fn new_with_ca(
        server_url: &str,
        email: &str,
        ca_pem_path: impl AsRef<std::path::Path>,
    ) -> Result<(Self, AccountCredentials)> {
        let new_account = NewAccount {
            contact: &[&format!("mailto:{}", email)],
            terms_of_service_agreed: true,
            only_return_existing: false,
        };

        let builder =
            Account::builder_with_root(ca_pem_path).context("Failed to create ACME account builder with custom CA")?;

        let (account, credentials) = builder
            .create(&new_account, server_url.to_string(), None)
            .await
            .context("Failed to create ACME account")?;

        tracing::info!(
            server = server_url,
            email = email,
            account_id = %account.id(),
            "ACME account created (with custom CA)"
        );

        Ok((Self { account }, credentials))
    }

    /// Restore an ACME client from existing account credentials
    pub async fn from_credentials(credentials: AccountCredentials) -> Result<Self> {
        let builder = Account::builder().context("Failed to create ACME account builder")?;

        let account = builder
            .from_credentials(credentials)
            .await
            .context("Failed to restore ACME account from credentials")?;

        tracing::debug!(
            account_id = %account.id(),
            "ACME account restored from credentials"
        );

        Ok(Self { account })
    }

    /// Restore an ACME client from credentials with a custom root CA (for testing).
    pub async fn from_credentials_with_ca(
        credentials: AccountCredentials,
        ca_pem_path: impl AsRef<std::path::Path>,
    ) -> Result<Self> {
        let builder =
            Account::builder_with_root(ca_pem_path).context("Failed to create ACME account builder with custom CA")?;

        let account = builder
            .from_credentials(credentials)
            .await
            .context("Failed to restore ACME account from credentials")?;

        Ok(Self { account })
    }

    /// Get the account ID (URL)
    pub fn account_id(&self) -> &str {
        self.account.id()
    }

    /// Phase 1: Prepare an HTTP-01 certificate order.
    ///
    /// Creates an ACME order and extracts challenge tokens, but does NOT notify
    /// the ACME server yet (no `set_ready()`).
    ///
    /// Correct flow:
    /// 1. `prepare_http01_order()` — extract tokens
    /// 2. Caller deploys tokens to Gateway (via CRD patch → gRPC)
    /// 3. Caller waits for Gateway to load tokens
    /// 4. `activate_challenges()` — notify ACME server to start validation
    /// 5. `complete_http01_order()` — poll & finalize
    pub async fn prepare_http01_order(
        &self,
        domains: &[String],
        guard: &impl AcmeOperationGuard,
    ) -> Result<(Vec<PendingHttpChallenge>, Http01OrderContext)> {
        let identifiers: Vec<Identifier> = domains.iter().map(|d| Identifier::Dns(d.clone())).collect();

        guard.check("create ACME order").await?;
        let new_order = NewOrder::new(&identifiers);
        let create_order = self.account.new_order(&new_order);
        let mut order = tokio::select! {
            result = create_order => result.context("Failed to create ACME order")?,
            _ = guard.cancelled() => anyhow::bail!("ACME order creation was cancelled"),
        };

        let mut auths = order.authorizations();
        let mut pending = Vec::new();

        while let Some(auth_result) = auths.next().await {
            let mut auth = auth_result.context("Failed to get authorization")?;

            if auth.status == AuthorizationStatus::Valid {
                continue;
            }

            // Extract identifier info first (immutable borrow)
            let domain = auth.identifier().to_string();

            // Now get challenge (mutable borrow) — only extract token, do NOT set_ready
            let challenge = auth
                .challenge(ChallengeType::Http01)
                .ok_or_else(|| anyhow::anyhow!("No HTTP-01 challenge available for {}", &domain))?;

            let key_auth = challenge.key_authorization();
            let token = challenge.token.clone();
            let key_auth_str = key_auth.as_str().to_string();

            pending.push(PendingHttpChallenge {
                domain,
                token,
                key_authorization: key_auth_str,
            });
        }

        // Drop auths to release borrow on order

        Ok((pending, Http01OrderContext { order }))
    }

    /// Phase 2: Notify the ACME server that HTTP-01 challenges are ready for validation.
    ///
    /// Call this AFTER the challenge tokens have been deployed to the Gateway.
    /// Re-fetches authorizations and calls `set_ready()` on each pending challenge.
    pub async fn activate_http01_challenges(
        &self,
        ctx: &mut Http01OrderContext,
        guard: &impl AcmeOperationGuard,
    ) -> Result<()> {
        let mut auths = ctx.order.authorizations();

        while let Some(auth_result) = auths.next().await {
            let mut auth = auth_result.context("Failed to get authorization")?;

            if auth.status == AuthorizationStatus::Valid {
                continue;
            }

            let domain = auth.identifier().to_string();

            let mut challenge = auth
                .challenge(ChallengeType::Http01)
                .ok_or_else(|| anyhow::anyhow!("No HTTP-01 challenge for {}", &domain))?;

            guard.check("activate HTTP-01 challenge").await?;
            let set_ready = challenge.set_ready();
            tokio::select! {
                result = set_ready => {
                    result.context(format!("Failed to set HTTP-01 challenge ready for {}", &domain))?;
                }
                _ = guard.cancelled() => anyhow::bail!("HTTP-01 challenge activation was cancelled"),
            }

            tracing::debug!(
                domain = %domain,
                "Notified ACME server: HTTP-01 challenge ready"
            );
        }

        Ok(())
    }

    /// Phase 3: Complete an HTTP-01 order after challenges have been validated.
    ///
    /// Polls for order readiness, generates CSR, and retrieves the certificate.
    pub async fn complete_http01_order(
        &self,
        ctx: Http01OrderContext,
        domains: &[String],
        key_type: &AcmeKeyType,
        guard: &impl AcmeOperationGuard,
    ) -> Result<AcmeCertificateResult> {
        self.finalize_order(ctx.order, domains, key_type, guard).await
    }

    /// Finalize the HTTP-01 order
    async fn finalize_order(
        &self,
        mut order: instant_acme::Order,
        domains: &[String],
        key_type: &AcmeKeyType,
        guard: &impl AcmeOperationGuard,
    ) -> Result<AcmeCertificateResult> {
        // Wait for the order to become ready (challenges validated)
        let retry = instant_acme::RetryPolicy::default();
        tokio::select! {
            result = order.poll_ready(&retry) => {
                let _ = result.context("Order validation failed or timed out")?;
            }
            _ = guard.cancelled() => anyhow::bail!("ACME order validation was cancelled"),
        }

        // Generate key pair and CSR
        let (private_key_pem, csr_der) = generate_csr(domains, key_type)?;

        // Submit CSR to ACME server
        guard.check("finalize ACME order").await?;
        tokio::select! {
            result = order.finalize_csr(&csr_der) => result.context("Failed to finalize ACME order with CSR")?,
            _ = guard.cancelled() => anyhow::bail!("ACME order finalization was cancelled"),
        }

        // Wait for and retrieve the certificate
        let certificate_pem = tokio::select! {
            result = order.poll_certificate(&retry) => result.context("Failed to retrieve certificate")?,
            _ = guard.cancelled() => anyhow::bail!("ACME certificate polling was cancelled"),
        };

        tracing::info!(
            domains = ?domains,
            "ACME certificate obtained successfully"
        );

        Ok(AcmeCertificateResult {
            certificate_pem,
            private_key_pem: Zeroizing::new(private_key_pem),
        })
    }
}

/// Context for an in-progress HTTP-01 ACME order
pub struct Http01OrderContext {
    order: instant_acme::Order,
}

/// Generate a private key and CSR (DER-encoded) for the given domains
fn generate_csr(domains: &[String], key_type: &AcmeKeyType) -> Result<(String, Vec<u8>)> {
    let alg = match key_type {
        AcmeKeyType::EcdsaP256 => &rcgen::PKCS_ECDSA_P256_SHA256,
        AcmeKeyType::EcdsaP384 => &rcgen::PKCS_ECDSA_P384_SHA384,
    };

    let key_pair = KeyPair::generate_for(alg).context("Failed to generate key pair")?;
    let private_key_pem = key_pair.serialize_pem();

    let mut params = CertificateParams::new(domains.to_vec()).context("Failed to create certificate params")?;
    params.distinguished_name = DistinguishedName::new();

    let csr = params.serialize_request(&key_pair).context("Failed to serialize CSR")?;
    let csr_der = csr.der().to_vec();

    Ok((private_key_pem, csr_der))
}
