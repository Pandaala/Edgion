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

use crate::types::resources::edgion_acme::AcmeKeyType;

/// ACME client wrapper
pub struct AcmeClient {
    account: Account,
}

/// Result of an ACME certificate order
pub struct AcmeCertificateResult {
    /// PEM-encoded certificate chain
    pub certificate_pem: String,
    /// PEM-encoded private key
    pub private_key_pem: String,
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

/// Pending DNS-01 challenge information
pub struct PendingDnsChallenge {
    /// The domain being validated
    pub domain: String,
    /// The digest value to set as DNS TXT record at _acme-challenge.{domain}
    pub digest: String,
}

impl AcmeClient {
    /// Create a new ACME client with a new account registration
    pub async fn new(
        server_url: &str,
        email: &str,
        eab_kid: Option<&str>,
        eab_hmac_key: Option<&str>,
    ) -> Result<(Self, AccountCredentials)> {
        let new_account = NewAccount {
            contact: &[&format!("mailto:{}", email)],
            terms_of_service_agreed: true,
            only_return_existing: false,
        };

        // Build external account key if provided
        let eab = match (eab_kid, eab_hmac_key) {
            (Some(kid), Some(hmac_b64)) => {
                let hmac_bytes = base64::Engine::decode(
                    &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                    hmac_b64,
                )
                .or_else(|_| {
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, hmac_b64)
                })
                .context("Failed to decode EAB HMAC key from base64")?;
                Some(ExternalAccountKey::new(kid.to_string(), &hmac_bytes))
            }
            _ => None,
        };

        let builder = Account::builder().context("Failed to create ACME account builder")?;

        let (account, credentials) = builder
            .create(&new_account, server_url.to_string(), eab.as_ref())
            .await
            .context("Failed to create ACME account")?;

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

        let builder = Account::builder_with_root(ca_pem_path)
            .context("Failed to create ACME account builder with custom CA")?;

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
        let builder = Account::builder_with_root(ca_pem_path)
            .context("Failed to create ACME account builder with custom CA")?;

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
    ) -> Result<(Vec<PendingHttpChallenge>, Http01OrderContext)> {
        let identifiers: Vec<Identifier> = domains.iter().map(|d| Identifier::Dns(d.clone())).collect();

        let mut order = self
            .account
            .new_order(&NewOrder::new(&identifiers))
            .await
            .context("Failed to create ACME order")?;

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
        drop(auths);

        Ok((pending, Http01OrderContext { order }))
    }

    /// Phase 2: Notify the ACME server that HTTP-01 challenges are ready for validation.
    ///
    /// Call this AFTER the challenge tokens have been deployed to the Gateway.
    /// Re-fetches authorizations and calls `set_ready()` on each pending challenge.
    pub async fn activate_http01_challenges(
        &self,
        ctx: &mut Http01OrderContext,
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

            challenge
                .set_ready()
                .await
                .context(format!("Failed to set HTTP-01 challenge ready for {}", &domain))?;

            tracing::debug!(
                domain = %domain,
                "Notified ACME server: HTTP-01 challenge ready"
            );
        }

        drop(auths);
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
    ) -> Result<AcmeCertificateResult> {
        self.finalize_order(ctx.order, domains, key_type).await
    }

    /// Phase 1: Prepare a DNS-01 certificate order.
    ///
    /// Creates an ACME order and extracts challenge digests, but does NOT notify
    /// the ACME server yet (no `set_ready()`).
    ///
    /// Correct flow:
    /// 1. `prepare_dns01_order()` — extract digests
    /// 2. Caller creates DNS TXT records
    /// 3. Caller waits for DNS propagation
    /// 4. `activate_dns01_challenges()` — notify ACME server to start validation
    /// 5. `complete_dns01_order()` — poll & finalize
    pub async fn prepare_dns01_order(
        &self,
        domains: &[String],
    ) -> Result<(Vec<PendingDnsChallenge>, Dns01OrderContext)> {
        // For DNS identifiers, strip wildcard prefix
        let identifiers: Vec<Identifier> = domains
            .iter()
            .map(|d| Identifier::Dns(d.strip_prefix("*.").unwrap_or(d).to_string()))
            .collect();

        let mut order = self
            .account
            .new_order(&NewOrder::new(&identifiers))
            .await
            .context("Failed to create ACME order")?;

        let mut auths = order.authorizations();
        let mut pending = Vec::new();

        while let Some(auth_result) = auths.next().await {
            let mut auth = auth_result.context("Failed to get authorization")?;

            if auth.status == AuthorizationStatus::Valid {
                continue;
            }

            // Extract identifier info first (immutable borrow)
            let domain = auth.identifier().to_string();

            // Now get challenge (mutable borrow) — only extract digest, do NOT set_ready
            let challenge = auth
                .challenge(ChallengeType::Dns01)
                .ok_or_else(|| anyhow::anyhow!("No DNS-01 challenge available for {}", &domain))?;

            let key_auth = challenge.key_authorization();
            let digest = instant_acme::KeyAuthorization::dns_value(&key_auth);

            pending.push(PendingDnsChallenge { domain, digest });
        }

        drop(auths);

        Ok((pending, Dns01OrderContext { order }))
    }

    /// Phase 2: Notify the ACME server that DNS-01 challenges are ready for validation.
    ///
    /// Call this AFTER DNS TXT records have been created and propagated.
    pub async fn activate_dns01_challenges(
        &self,
        ctx: &mut Dns01OrderContext,
    ) -> Result<()> {
        let mut auths = ctx.order.authorizations();

        while let Some(auth_result) = auths.next().await {
            let mut auth = auth_result.context("Failed to get authorization")?;

            if auth.status == AuthorizationStatus::Valid {
                continue;
            }

            let domain = auth.identifier().to_string();

            let mut challenge = auth
                .challenge(ChallengeType::Dns01)
                .ok_or_else(|| anyhow::anyhow!("No DNS-01 challenge for {}", &domain))?;

            challenge
                .set_ready()
                .await
                .context(format!("Failed to set DNS-01 challenge ready for {}", &domain))?;

            tracing::debug!(
                domain = %domain,
                "Notified ACME server: DNS-01 challenge ready"
            );
        }

        drop(auths);
        Ok(())
    }

    /// Phase 3: Complete a DNS-01 order after challenges have been validated.
    pub async fn complete_dns01_order(
        &self,
        ctx: Dns01OrderContext,
        domains: &[String],
        key_type: &AcmeKeyType,
    ) -> Result<AcmeCertificateResult> {
        self.finalize_order(ctx.order, domains, key_type).await
    }

    /// Internal: finalize an order (shared by HTTP-01 and DNS-01)
    async fn finalize_order(
        &self,
        mut order: instant_acme::Order,
        domains: &[String],
        key_type: &AcmeKeyType,
    ) -> Result<AcmeCertificateResult> {
        // Wait for the order to become ready (challenges validated)
        let retry = instant_acme::RetryPolicy::default();
        order
            .poll_ready(&retry)
            .await
            .context("Order validation failed or timed out")?;

        // Generate key pair and CSR
        let (private_key_pem, csr_der) = generate_csr(domains, key_type)?;

        // Submit CSR to ACME server
        order
            .finalize_csr(&csr_der)
            .await
            .context("Failed to finalize ACME order with CSR")?;

        // Wait for and retrieve the certificate
        let certificate_pem = order
            .poll_certificate(&retry)
            .await
            .context("Failed to retrieve certificate")?;

        tracing::info!(
            domains = ?domains,
            "ACME certificate obtained successfully"
        );

        Ok(AcmeCertificateResult {
            certificate_pem,
            private_key_pem,
        })
    }
}

/// Context for an in-progress HTTP-01 ACME order
pub struct Http01OrderContext {
    order: instant_acme::Order,
}

/// Context for an in-progress DNS-01 ACME order
pub struct Dns01OrderContext {
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

    let mut params =
        CertificateParams::new(domains.to_vec()).context("Failed to create certificate params")?;
    params.distinguished_name = DistinguishedName::new();

    let csr = params
        .serialize_request(&key_pair)
        .context("Failed to serialize CSR")?;
    let csr_der = csr.der().to_vec();

    Ok((private_key_pem, csr_der))
}
