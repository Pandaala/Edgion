//! DNS-01 Challenge Provider Trait and Implementations
//!
//! Provides a trait for DNS-01 challenge validation by creating/removing
//! TXT records at `_acme-challenge.<domain>`.

use anyhow::{Context, Result};
use async_trait::async_trait;

/// DNS-01 challenge provider trait
///
/// Implementations create and remove TXT records for ACME DNS-01 validation.
#[async_trait]
pub trait DnsProvider: Send + Sync {
    /// Provider name (for logging/diagnostics)
    fn name(&self) -> &str;

    /// Create a DNS TXT record for the ACME challenge.
    ///
    /// # Arguments
    /// * `domain` - The domain being validated (e.g., "example.com")
    /// * `value` - The challenge digest value to set as TXT record content
    ///
    /// The TXT record should be created at `_acme-challenge.<domain>`.
    async fn create_txt_record(&self, domain: &str, value: &str) -> Result<()>;

    /// Remove the DNS TXT record after challenge validation.
    ///
    /// # Arguments
    /// * `domain` - The domain being validated
    /// * `value` - The challenge digest value (to identify which record to remove)
    async fn remove_txt_record(&self, domain: &str, value: &str) -> Result<()>;
}

/// Create a DNS provider from configuration
pub fn create_dns_provider(
    provider_name: &str,
    credentials: &std::collections::HashMap<String, String>,
) -> Result<Box<dyn DnsProvider>> {
    match provider_name {
        "cloudflare" => {
            let api_token = credentials
                .get("api-token")
                .ok_or_else(|| anyhow::anyhow!("Cloudflare: 'api-token' not found in credential Secret"))?;
            Ok(Box::new(CloudflareDnsProvider::new(api_token.clone())))
        }
        "alidns" => {
            let access_key_id = credentials
                .get("access-key-id")
                .ok_or_else(|| anyhow::anyhow!("AliDNS: 'access-key-id' not found in credential Secret"))?;
            let access_key_secret = credentials
                .get("access-key-secret")
                .ok_or_else(|| anyhow::anyhow!("AliDNS: 'access-key-secret' not found in credential Secret"))?;
            Ok(Box::new(AlidnsDnsProvider::new(
                access_key_id.clone(),
                access_key_secret.clone(),
            )))
        }
        "pebble" => {
            let api_url = credentials
                .get("api-url")
                .cloned()
                .unwrap_or_else(|| "http://localhost:8055".to_string());
            Ok(Box::new(PebbleChalltestDnsProvider::new(api_url)))
        }
        other => Err(anyhow::anyhow!("Unsupported DNS provider: {}", other)),
    }
}

// ============================================================================
// Cloudflare DNS Provider
// ============================================================================

/// Cloudflare DNS API provider
///
/// Uses the Cloudflare API to create/remove TXT records.
/// Requires an API token with `Zone.DNS:Edit` permission.
pub struct CloudflareDnsProvider {
    api_token: String,
    client: reqwest::Client,
}

impl CloudflareDnsProvider {
    pub fn new(api_token: String) -> Self {
        Self {
            api_token,
            client: reqwest::Client::new(),
        }
    }

    /// Get the zone ID for a domain by querying the Cloudflare API
    async fn get_zone_id(&self, domain: &str) -> Result<String> {
        // Extract the root domain (e.g., "example.com" from "sub.example.com")
        let root_domain = extract_root_domain(domain);

        let resp = self
            .client
            .get("https://api.cloudflare.com/client/v4/zones")
            .query(&[("name", root_domain.as_str())])
            .bearer_auth(&self.api_token)
            .send()
            .await?;

        let body: serde_json::Value = resp.json().await?;

        if !body["success"].as_bool().unwrap_or(false) {
            return Err(anyhow::anyhow!(
                "Cloudflare API error: {}",
                body["errors"]
            ));
        }

        body["result"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|zone| zone["id"].as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Zone not found for domain: {}", root_domain))
    }

    /// Find existing TXT record ID
    async fn find_txt_record(&self, zone_id: &str, record_name: &str) -> Result<Option<String>> {
        let resp = self
            .client
            .get(format!(
                "https://api.cloudflare.com/client/v4/zones/{}/dns_records",
                zone_id
            ))
            .query(&[("type", "TXT"), ("name", record_name)])
            .bearer_auth(&self.api_token)
            .send()
            .await?;

        let body: serde_json::Value = resp.json().await?;

        if !body["success"].as_bool().unwrap_or(false) {
            return Err(anyhow::anyhow!("Cloudflare API error: {}", body["errors"]));
        }

        Ok(body["result"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|record| record["id"].as_str())
            .map(|s| s.to_string()))
    }
}

#[async_trait]
impl DnsProvider for CloudflareDnsProvider {
    fn name(&self) -> &str {
        "cloudflare"
    }

    async fn create_txt_record(&self, domain: &str, value: &str) -> Result<()> {
        let zone_id = self.get_zone_id(domain).await?;
        let record_name = format!("_acme-challenge.{}", domain);

        tracing::info!(
            provider = "cloudflare",
            domain = domain,
            record = %record_name,
            "Creating DNS TXT record for ACME challenge"
        );

        let resp = self
            .client
            .post(format!(
                "https://api.cloudflare.com/client/v4/zones/{}/dns_records",
                zone_id
            ))
            .bearer_auth(&self.api_token)
            .json(&serde_json::json!({
                "type": "TXT",
                "name": record_name,
                "content": value,
                "ttl": 60
            }))
            .send()
            .await?;

        let body: serde_json::Value = resp.json().await?;

        if !body["success"].as_bool().unwrap_or(false) {
            return Err(anyhow::anyhow!(
                "Failed to create TXT record: {}",
                body["errors"]
            ));
        }

        tracing::info!(
            provider = "cloudflare",
            domain = domain,
            "DNS TXT record created successfully"
        );

        Ok(())
    }

    async fn remove_txt_record(&self, domain: &str, _value: &str) -> Result<()> {
        let zone_id = self.get_zone_id(domain).await?;
        let record_name = format!("_acme-challenge.{}", domain);

        if let Some(record_id) = self.find_txt_record(&zone_id, &record_name).await? {
            tracing::info!(
                provider = "cloudflare",
                domain = domain,
                record_id = %record_id,
                "Removing DNS TXT record"
            );

            let resp = self
                .client
                .delete(format!(
                    "https://api.cloudflare.com/client/v4/zones/{}/dns_records/{}",
                    zone_id, record_id
                ))
                .bearer_auth(&self.api_token)
                .send()
                .await?;

            let body: serde_json::Value = resp.json().await?;

            if !body["success"].as_bool().unwrap_or(false) {
                tracing::warn!(
                    provider = "cloudflare",
                    domain = domain,
                    "Failed to remove TXT record: {}",
                    body["errors"]
                );
            }
        }

        Ok(())
    }
}

// ============================================================================
// Alibaba Cloud DNS Provider
// ============================================================================

/// Alibaba Cloud DNS (AliDNS) provider
///
/// Uses the Alibaba Cloud DNS API to create/remove TXT records.
/// Requires AccessKeyId and AccessKeySecret with DNS management permissions.
pub struct AlidnsDnsProvider {
    access_key_id: String,
    access_key_secret: String,
    client: reqwest::Client,
}

impl AlidnsDnsProvider {
    pub fn new(access_key_id: String, access_key_secret: String) -> Self {
        Self {
            access_key_id,
            access_key_secret,
            client: reqwest::Client::new(),
        }
    }

    /// Sign the request using Alibaba Cloud signature v1
    fn sign_request(&self, params: &mut Vec<(String, String)>) {
        use hmac::{Hmac, Mac};
        use sha1::Sha1;

        // Add common parameters
        params.push(("Format".to_string(), "JSON".to_string()));
        params.push(("Version".to_string(), "2015-01-09".to_string()));
        params.push(("AccessKeyId".to_string(), self.access_key_id.clone()));
        params.push(("SignatureMethod".to_string(), "HMAC-SHA1".to_string()));
        params.push(("SignatureVersion".to_string(), "1.0".to_string()));
        params.push((
            "SignatureNonce".to_string(),
            uuid::Uuid::new_v4().to_string(),
        ));
        params.push((
            "Timestamp".to_string(),
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        ));

        // Sort parameters
        params.sort_by(|a, b| a.0.cmp(&b.0));

        // Build canonical query string
        let canonical_query: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        // Build string to sign
        let string_to_sign = format!(
            "GET&{}&{}",
            percent_encode("/"),
            percent_encode(&canonical_query)
        );

        // Calculate HMAC-SHA1
        let signing_key = format!("{}&", self.access_key_secret);
        let mut mac =
            Hmac::<Sha1>::new_from_slice(signing_key.as_bytes()).expect("HMAC can take key of any size");
        mac.update(string_to_sign.as_bytes());
        let signature = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, mac.finalize().into_bytes());

        params.push(("Signature".to_string(), signature));
    }
}

/// Percent-encode a string per RFC 3986
fn percent_encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[async_trait]
impl DnsProvider for AlidnsDnsProvider {
    fn name(&self) -> &str {
        "alidns"
    }

    async fn create_txt_record(&self, domain: &str, value: &str) -> Result<()> {
        let root_domain = extract_root_domain(domain);
        // For wildcard domains like "*.example.com", the RR is "_acme-challenge"
        // For regular domains like "sub.example.com", the RR is "_acme-challenge.sub"
        let rr = build_acme_rr(domain, &root_domain);

        tracing::info!(
            provider = "alidns",
            domain = domain,
            rr = %rr,
            root_domain = %root_domain,
            "Creating DNS TXT record for ACME challenge"
        );

        let mut params = vec![
            ("Action".to_string(), "AddDomainRecord".to_string()),
            ("DomainName".to_string(), root_domain),
            ("RR".to_string(), rr),
            ("Type".to_string(), "TXT".to_string()),
            ("Value".to_string(), value.to_string()),
            ("TTL".to_string(), "600".to_string()),
        ];

        self.sign_request(&mut params);

        let resp = self
            .client
            .get("https://alidns.aliyuncs.com/")
            .query(&params)
            .send()
            .await?;

        let body: serde_json::Value = resp.json().await?;

        if body.get("Code").is_some() {
            return Err(anyhow::anyhow!(
                "AliDNS API error: {} - {}",
                body["Code"],
                body["Message"]
            ));
        }

        tracing::info!(
            provider = "alidns",
            domain = domain,
            record_id = %body["RecordId"],
            "DNS TXT record created successfully"
        );

        Ok(())
    }

    async fn remove_txt_record(&self, domain: &str, _value: &str) -> Result<()> {
        let root_domain = extract_root_domain(domain);
        let rr = build_acme_rr(domain, &root_domain);

        // First, find the record
        let mut params = vec![
            ("Action".to_string(), "DescribeDomainRecords".to_string()),
            ("DomainName".to_string(), root_domain),
            ("RRKeyWord".to_string(), rr),
            ("TypeKeyWord".to_string(), "TXT".to_string()),
        ];

        self.sign_request(&mut params);

        let resp = self
            .client
            .get("https://alidns.aliyuncs.com/")
            .query(&params)
            .send()
            .await?;

        let body: serde_json::Value = resp.json().await?;

        if let Some(records) = body["DomainRecords"]["Record"].as_array() {
            for record in records {
                if let Some(record_id) = record["RecordId"].as_str() {
                    tracing::info!(
                        provider = "alidns",
                        domain = domain,
                        record_id = record_id,
                        "Removing DNS TXT record"
                    );

                    let mut delete_params = vec![
                        ("Action".to_string(), "DeleteDomainRecord".to_string()),
                        ("RecordId".to_string(), record_id.to_string()),
                    ];

                    self.sign_request(&mut delete_params);

                    self.client
                        .get("https://alidns.aliyuncs.com/")
                        .query(&delete_params)
                        .send()
                        .await?;
                }
            }
        }

        Ok(())
    }
}

// ============================================================================
// Utility functions
// ============================================================================

/// Extract root domain from a full domain name.
/// e.g., "sub.example.com" -> "example.com"
///       "*.example.com" -> "example.com"
///       "example.com" -> "example.com"
fn extract_root_domain(domain: &str) -> String {
    let domain = domain.strip_prefix("*.").unwrap_or(domain);
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        domain.to_string()
    }
}

/// Build the ACME challenge RR (subdomain) for AliDNS.
/// e.g., domain="example.com", root="example.com" -> "_acme-challenge"
///       domain="sub.example.com", root="example.com" -> "_acme-challenge.sub"
///       domain="*.example.com", root="example.com" -> "_acme-challenge"
fn build_acme_rr(domain: &str, root_domain: &str) -> String {
    let domain = domain.strip_prefix("*.").unwrap_or(domain);
    let suffix = format!(".{}", root_domain);
    let prefix = domain.strip_suffix(&suffix).unwrap_or("");
    if prefix.is_empty() {
        "_acme-challenge".to_string()
    } else {
        format!("_acme-challenge.{}", prefix)
    }
}

// Need sha1 for AliDNS HMAC-SHA1 signature
use sha1;

// ============================================================================
// Pebble challtestsrv DNS Provider (for integration testing)
// ============================================================================

/// DNS provider backed by Pebble's `challtestsrv` mock DNS server.
///
/// Uses the challtestsrv management REST API to add/remove TXT records.
/// See: <https://github.com/letsencrypt/challtestsrv>
///
/// ## API
///
/// - `POST /set-txt`   body: `{"host": "_acme-challenge.example.com.", "value": "digest"}`
/// - `POST /clear-txt` body: `{"host": "_acme-challenge.example.com."}`
pub struct PebbleChalltestDnsProvider {
    /// challtestsrv management API URL (e.g., "http://localhost:8055")
    api_url: String,
    client: reqwest::Client,
}

impl PebbleChalltestDnsProvider {
    pub fn new(api_url: String) -> Self {
        Self {
            api_url,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl DnsProvider for PebbleChalltestDnsProvider {
    fn name(&self) -> &str {
        "pebble-challtestsrv"
    }

    async fn create_txt_record(&self, domain: &str, value: &str) -> Result<()> {
        // challtestsrv expects FQDN with trailing dot
        let fqdn = format!("_acme-challenge.{}.", domain.trim_end_matches('.'));

        let resp: reqwest::Response = self
            .client
            .post(format!("{}/set-txt", self.api_url))
            .json(&serde_json::json!({
                "host": fqdn,
                "value": value,
            }))
            .send()
            .await
            .context("Failed to call challtestsrv /set-txt")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "challtestsrv /set-txt failed: {} - {}",
                status,
                body
            ));
        }

        tracing::debug!(
            domain = domain,
            fqdn = %fqdn,
            "challtestsrv: TXT record set"
        );
        Ok(())
    }

    async fn remove_txt_record(&self, domain: &str, _value: &str) -> Result<()> {
        let fqdn = format!("_acme-challenge.{}.", domain.trim_end_matches('.'));

        let resp: reqwest::Response = self
            .client
            .post(format!("{}/clear-txt", self.api_url))
            .json(&serde_json::json!({
                "host": fqdn,
            }))
            .send()
            .await
            .context("Failed to call challtestsrv /clear-txt")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "challtestsrv /clear-txt failed: {} - {}",
                status,
                body
            ));
        }

        tracing::debug!(
            domain = domain,
            fqdn = %fqdn,
            "challtestsrv: TXT record cleared"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_root_domain() {
        assert_eq!(extract_root_domain("example.com"), "example.com");
        assert_eq!(extract_root_domain("sub.example.com"), "example.com");
        assert_eq!(extract_root_domain("*.example.com"), "example.com");
        assert_eq!(extract_root_domain("a.b.example.com"), "example.com");
    }

    #[test]
    fn test_build_acme_rr() {
        assert_eq!(build_acme_rr("example.com", "example.com"), "_acme-challenge");
        assert_eq!(
            build_acme_rr("sub.example.com", "example.com"),
            "_acme-challenge.sub"
        );
        assert_eq!(
            build_acme_rr("*.example.com", "example.com"),
            "_acme-challenge"
        );
        assert_eq!(
            build_acme_rr("a.b.example.com", "example.com"),
            "_acme-challenge.a.b"
        );
    }
}
