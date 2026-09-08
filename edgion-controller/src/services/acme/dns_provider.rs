//! DNS-01 Challenge Provider Trait and Implementations
//!
//! Provides a trait for DNS-01 challenge validation by creating/removing
//! TXT records at `_acme-challenge.<domain>`.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use edgion_common::link_sys_core::http_client::{build_outbound_client, OutboundClientConfig};
use futures::StreamExt;

/// Connect timeout for a DNS provider API request (TCP + TLS handshake).
const DNS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Total per-request timeout (connect + TLS + headers + body) for a single DNS
/// provider API call. Bounds how long one HTTP call to a DNS API can hang so a
/// slow or misbehaving endpoint cannot keep an ACME flow task alive forever.
const DNS_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on a DNS provider response body we are willing to buffer.
/// Provider responses are small JSON/text payloads; anything larger is treated
/// as a misbehaving endpoint and rejected before fully buffering it.
const MAX_DNS_RESPONSE_BYTES: usize = 256 * 1024;

/// Build a reqwest client for DNS provider API calls through the shared
/// outbound factory, with explicit connect and total request timeouts. The
/// factory's default `Deny` redirect policy is kept deliberately: all supported
/// provider APIs (Cloudflare, AliDNS, challtestsrv) are direct REST endpoints,
/// and following redirects would only re-expose credentialed requests. Falls
/// back to a default client if the builder fails; with the static config above
/// that path is practically unreachable.
fn build_dns_http_client() -> reqwest::Client {
    build_outbound_client(OutboundClientConfig {
        connect_timeout: Some(DNS_CONNECT_TIMEOUT),
        total_timeout: Some(DNS_REQUEST_TIMEOUT),
        ..Default::default()
    })
    .unwrap_or_else(|_| {
        tracing::warn!(
            component = "acme_dns_provider",
            "Failed to build DNS HTTP client with timeouts; falling back to default client"
        );
        reqwest::Client::new()
    })
}

/// Read a response body with a hard byte cap, mirroring the gateway's
/// `fetch_json_limited`: a `Content-Length` pre-check rejects well-behaved
/// oversized responses before any body is read, and streaming accumulation
/// cuts off a chunked/dishonest server the moment the running total would
/// exceed the cap (peak memory is at most `MAX_DNS_RESPONSE_BYTES` + one chunk).
async fn read_body_limited(resp: reqwest::Response, ctx: &'static str) -> Result<Vec<u8>> {
    if let Some(cl) = resp.content_length() {
        if cl > MAX_DNS_RESPONSE_BYTES as u64 {
            anyhow::bail!(
                "{} response too large: Content-Length {} > limit {} bytes",
                ctx,
                cl,
                MAX_DNS_RESPONSE_BYTES
            );
        }
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(8192.min(MAX_DNS_RESPONSE_BYTES));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| anyhow::anyhow!("{} response body read failed", ctx))?;
        if buf.len().saturating_add(chunk.len()) > MAX_DNS_RESPONSE_BYTES {
            anyhow::bail!("{} response body exceeds limit {} bytes", ctx, MAX_DNS_RESPONSE_BYTES);
        }
        buf.extend_from_slice(&chunk);
    }

    Ok(buf)
}

/// Read and parse a response body as JSON, refusing bodies larger than
/// `MAX_DNS_RESPONSE_BYTES` (see [`read_body_limited`]).
async fn read_json_limited(resp: reqwest::Response, ctx: &'static str) -> Result<serde_json::Value> {
    check_http_status(&resp, ctx)?;
    let bytes = read_body_limited(resp, ctx).await?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("{} invalid JSON response", ctx))
}

/// Drop the original error, including its source chain: URLs may contain signed
/// AliDNS queries or response-controlled record IDs. Removing only the top-level
/// message is insufficient for the ACME status writer's alternate Display.
fn dns_transport_error(error: reqwest::Error, ctx: &'static str) -> anyhow::Error {
    let category = if error.is_timeout() {
        "request timed out"
    } else if error.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    anyhow::anyhow!("{} {}", ctx, category)
}

fn check_http_status(resp: &reqwest::Response, ctx: &'static str) -> Result<()> {
    if !resp.status().is_success() {
        anyhow::bail!("{} HTTP request rejected (status={})", ctx, resp.status().as_u16());
    }
    Ok(())
}

/// Provider text is never diagnostic data. Keep only the first bounded numeric
/// Cloudflare code; string codes, messages, IDs and extra fields are discarded.
fn check_cloudflare_response(body: &serde_json::Value, ctx: &'static str) -> Result<()> {
    if body["success"].as_bool().unwrap_or(false) {
        return Ok(());
    }
    let code = body["errors"]
        .as_array()
        .and_then(|errors| errors.first())
        .and_then(|error| error["code"].as_u64())
        .and_then(|code| u16::try_from(code).ok());
    match code {
        Some(code) => anyhow::bail!("{} provider rejected request (code={})", ctx, code),
        None => anyhow::bail!("{} provider rejected request", ctx),
    }
}

fn check_alidns_response(body: &serde_json::Value, ctx: &'static str) -> Result<()> {
    if body.get("Code").is_some() {
        anyhow::bail!("{} provider rejected request", ctx);
    }
    Ok(())
}

/// DNS-01 challenge provider trait
///
/// Implementations create and remove TXT records for ACME DNS-01 validation.
#[async_trait]
pub trait DnsProvider: Send + Sync {
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

/// Create a DNS provider from configuration.
///
/// Key-presence preconditions (`api-token` for Cloudflare, `access-key-id` /
/// `access-key-secret` for AliDNS) are enforced up-front by
/// [`edgion_resources::credentials::resolvers::DnsCredentialResolver`] in the
/// Controller's credential pipeline — a Secret missing those keys surfaces
/// as `ResolvedRefs=False/MissingKeys` long before we reach this code path.
/// The `ok_or_else` checks below are kept as defense in depth for direct
/// test harnesses and to guarantee we never pass an empty token to the
/// upstream API client; in normal operation they are unreachable.
pub fn create_dns_provider(
    provider_name: &str,
    credentials: &std::collections::HashMap<String, String>,
) -> Result<Box<dyn DnsProvider>> {
    match provider_name {
        "cloudflare" => {
            let api_token = credentials.get("api-token").ok_or_else(|| {
                anyhow::anyhow!(
                    "Cloudflare: 'api-token' not found in credential Secret (pipeline layer should have caught this)"
                )
            })?;
            Ok(Box::new(CloudflareDnsProvider::new(api_token.clone())))
        }
        "alidns" => {
            let access_key_id = credentials.get("access-key-id").ok_or_else(|| {
                anyhow::anyhow!(
                    "AliDNS: 'access-key-id' not found in credential Secret (pipeline layer should have caught this)"
                )
            })?;
            let access_key_secret = credentials
                .get("access-key-secret")
                .ok_or_else(|| anyhow::anyhow!("AliDNS: 'access-key-secret' not found in credential Secret (pipeline layer should have caught this)"))?;
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
        _ => Err(anyhow::anyhow!("Unsupported DNS provider")),
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
            client: build_dns_http_client(),
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
            .await
            .map_err(|e| dns_transport_error(e, "Cloudflare list-zones"))?;

        let body = read_json_limited(resp, "Cloudflare list-zones").await?;

        check_cloudflare_response(&body, "Cloudflare list-zones")?;

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
            .await
            .map_err(|e| dns_transport_error(e, "Cloudflare list-records"))?;

        let body = read_json_limited(resp, "Cloudflare list-records").await?;

        check_cloudflare_response(&body, "Cloudflare list-records")?;

        Ok(body["result"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|record| record["id"].as_str())
            .map(|s| s.to_string()))
    }
}

#[async_trait]
impl DnsProvider for CloudflareDnsProvider {
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
            .await
            .map_err(|e| dns_transport_error(e, "Cloudflare create-record"))?;

        let body = read_json_limited(resp, "Cloudflare create-record").await?;

        check_cloudflare_response(&body, "Cloudflare create-record")?;

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
            tracing::info!(provider = "cloudflare", domain = domain, "Removing DNS TXT record");

            let resp = self
                .client
                .delete(format!(
                    "https://api.cloudflare.com/client/v4/zones/{}/dns_records/{}",
                    zone_id, record_id
                ))
                .bearer_auth(&self.api_token)
                .send()
                .await
                .map_err(|e| dns_transport_error(e, "Cloudflare delete-record"))?;

            let body = read_json_limited(resp, "Cloudflare delete-record").await?;

            if let Err(error) = check_cloudflare_response(&body, "Cloudflare delete-record") {
                tracing::warn!(
                    provider = "cloudflare",
                    domain = domain,
                    error = %error,
                    "Failed to remove TXT record"
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
            client: build_dns_http_client(),
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
        params.push(("SignatureNonce".to_string(), uuid::Uuid::new_v4().to_string()));
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
        let string_to_sign = format!("GET&{}&{}", percent_encode("/"), percent_encode(&canonical_query));

        // Calculate HMAC-SHA1
        let signing_key = format!("{}&", self.access_key_secret);
        let mut mac = Hmac::<Sha1>::new_from_slice(signing_key.as_bytes()).expect("HMAC can take key of any size");
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
            .await
            .map_err(|e| dns_transport_error(e, "AliDNS add-record"))?;

        let body = read_json_limited(resp, "AliDNS add-record").await?;

        check_alidns_response(&body, "AliDNS add-record")?;

        tracing::info!(
            provider = "alidns",
            domain = domain,
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
            .await
            .map_err(|e| dns_transport_error(e, "AliDNS describe-records"))?;

        let body = read_json_limited(resp, "AliDNS describe-records").await?;

        if let Some(records) = body["DomainRecords"]["Record"].as_array() {
            for record in records {
                if let Some(record_id) = record["RecordId"].as_str() {
                    tracing::info!(provider = "alidns", domain = domain, "Removing DNS TXT record");

                    let mut delete_params = vec![
                        ("Action".to_string(), "DeleteDomainRecord".to_string()),
                        ("RecordId".to_string(), record_id.to_string()),
                    ];

                    self.sign_request(&mut delete_params);

                    self.client
                        .get("https://alidns.aliyuncs.com/")
                        .query(&delete_params)
                        .send()
                        .await
                        .map_err(|e| dns_transport_error(e, "AliDNS delete-record"))?;
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
            client: build_dns_http_client(),
        }
    }
}

#[async_trait]
impl DnsProvider for PebbleChalltestDnsProvider {
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
            .map_err(|e| dns_transport_error(e, "challtestsrv /set-txt"))?;

        check_http_status(&resp, "challtestsrv /set-txt")?;

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
            .map_err(|e| dns_transport_error(e, "challtestsrv /clear-txt"))?;

        check_http_status(&resp, "challtestsrv /clear-txt")?;

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

    const SECRET_CANARY: &str = "DNS_SECRET_CANARY";
    const IDENTITY_CANARY: &str = "DNS_IDENTITY_CANARY";

    fn response(status: u16, body: impl Into<reqwest::Body>) -> reqwest::Response {
        http::Response::builder()
            .status(status)
            .body(body.into())
            .unwrap()
            .into()
    }

    fn assert_safe_error(error: anyhow::Error, expected: &str) {
        assert_eq!(error.to_string(), expected);
        // The service logs Display and persists alternate Display. Debug and
        // source traversal must not recover discarded dependency/body text.
        for text in [
            format!("{error}"),
            format!("{error:#}"),
            format!("{error:?}"),
            format!("{error:#?}"),
        ] {
            assert!(!text.contains(SECRET_CANARY), "{text}");
            assert!(!text.contains(IDENTITY_CANARY), "{text}");
        }
        assert_eq!(error.chain().count(), 1);
        let contextual = error.context("Failed to create DNS TXT record for example.com");
        let status = edgion_resources::EdgionAcmeStatus {
            last_failure_reason: Some(format!("{contextual:#}")),
            ..Default::default()
        };
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(serialized.contains(expected));
        assert!(!serialized.contains(SECRET_CANARY));
        assert!(!serialized.contains(IDENTITY_CANARY));
    }

    #[test]
    fn provider_rejections_discard_all_external_text() {
        for ctx in [
            "Cloudflare list-zones",
            "Cloudflare list-records",
            "Cloudflare create-record",
            "Cloudflare delete-record",
        ] {
            let mut body = serde_json::json!({
                "success": false,
                "errors": [{"code": 10000, "message": SECRET_CANARY, "identity": IDENTITY_CANARY}],
                "result": {"id": IDENTITY_CANARY}
            });
            assert_safe_error(
                check_cloudflare_response(&body, ctx).unwrap_err(),
                &format!("{ctx} provider rejected request (code=10000)"),
            );
            for code in [
                serde_json::json!(SECRET_CANARY),
                serde_json::json!(65536),
                serde_json::json!(-1),
                serde_json::json!(1.5),
                serde_json::Value::Null,
            ] {
                body["errors"][0]["code"] = code;
                assert_safe_error(
                    check_cloudflare_response(&body, ctx).unwrap_err(),
                    &format!("{ctx} provider rejected request"),
                );
            }
        }
        let body = serde_json::json!({"Code": IDENTITY_CANARY, "Message": SECRET_CANARY});
        assert_safe_error(
            check_alidns_response(&body, "AliDNS add-record").unwrap_err(),
            "AliDNS add-record provider rejected request",
        );
    }

    #[tokio::test]
    async fn provider_success_and_malformed_json() {
        let cloudflare = read_json_limited(
            response(200, r#"{"success":true,"result":[{"id":"zone-id"}]}"#),
            "Cloudflare list-zones",
        )
        .await
        .unwrap();
        check_cloudflare_response(&cloudflare, "Cloudflare list-zones").unwrap();
        assert_eq!(cloudflare["result"][0]["id"], "zone-id");
        let alidns = read_json_limited(response(200, r#"{"RecordId":"record-id"}"#), "AliDNS add-record")
            .await
            .unwrap();
        check_alidns_response(&alidns, "AliDNS add-record").unwrap();
        assert_eq!(alidns["RecordId"], "record-id");
        check_http_status(&response(204, ""), "challtestsrv /clear-txt").unwrap();
        let error = read_json_limited(
            response(200, format!("{{\"{SECRET_CANARY}\":{IDENTITY_CANARY}")),
            "Cloudflare create-record",
        )
        .await
        .unwrap_err();
        assert_safe_error(error, "Cloudflare create-record invalid JSON response");
    }

    #[tokio::test]
    async fn http_rejections_never_read_or_format_the_response_body() {
        for ctx in [
            "Cloudflare create-record",
            "Cloudflare delete-record",
            "AliDNS add-record",
            "AliDNS describe-records",
            "challtestsrv /set-txt",
            "challtestsrv /clear-txt",
        ] {
            let body = reqwest::Body::wrap_stream(futures::stream::poll_fn(
                |_| -> std::task::Poll<Option<std::io::Result<bytes::Bytes>>> {
                    panic!("HTTP rejection must not poll its body");
                },
            ));
            assert_safe_error(
                read_json_limited(response(403, body), ctx).await.unwrap_err(),
                &format!("{ctx} HTTP request rejected (status=403)"),
            );
        }
    }

    #[tokio::test]
    async fn response_caps_and_body_errors_remain_safe() {
        let declared: reqwest::Response = http::Response::builder()
            .header("content-length", MAX_DNS_RESPONSE_BYTES + 1)
            .body(reqwest::Body::from(vec![b'x'; MAX_DNS_RESPONSE_BYTES + 1]))
            .unwrap()
            .into();
        let error = read_body_limited(declared, "AliDNS add-record").await.unwrap_err();
        assert_safe_error(
            error,
            &format!(
                "AliDNS add-record response too large: Content-Length {} > limit {} bytes",
                MAX_DNS_RESPONSE_BYTES + 1,
                MAX_DNS_RESPONSE_BYTES
            ),
        );

        let chunks = vec![
            Ok::<_, std::io::Error>(bytes::Bytes::from(vec![b'x'; MAX_DNS_RESPONSE_BYTES])),
            Ok(bytes::Bytes::from_static(SECRET_CANARY.as_bytes())),
        ];
        let oversized = response(200, reqwest::Body::wrap_stream(futures::stream::iter(chunks)));
        assert_safe_error(
            read_body_limited(oversized, "AliDNS add-record").await.unwrap_err(),
            &format!(
                "AliDNS add-record response body exceeds limit {} bytes",
                MAX_DNS_RESPONSE_BYTES
            ),
        );

        let exact = read_body_limited(response(200, vec![b'x'; MAX_DNS_RESPONSE_BYTES]), "AliDNS add-record")
            .await
            .unwrap();
        assert_eq!(exact.len(), MAX_DNS_RESPONSE_BYTES);
        let broken = futures::stream::iter([Err::<bytes::Bytes, _>(std::io::Error::other(format!(
            "{SECRET_CANARY} {IDENTITY_CANARY}"
        )))]);
        assert_safe_error(
            read_body_limited(response(200, reqwest::Body::wrap_stream(broken)), "AliDNS add-record")
                .await
                .unwrap_err(),
            "AliDNS add-record response body read failed",
        );
    }

    #[test]
    fn transport_errors_drop_signed_url_and_source_chain() {
        let url = reqwest::Url::parse(&format!(
            "https://alidns.aliyuncs.com/?AccessKeyId={IDENTITY_CANARY}&Signature={SECRET_CANARY}"
        ))
        .unwrap();
        let error = reqwest::Client::new()
            .get(url.clone())
            .header("authorization", "invalid\nheader")
            .build()
            .unwrap_err()
            .with_url(url);
        assert!(format!("{error}").contains(SECRET_CANARY));
        assert_safe_error(
            dns_transport_error(error, "AliDNS delete-record"),
            "AliDNS delete-record request failed",
        );
    }

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
        assert_eq!(build_acme_rr("sub.example.com", "example.com"), "_acme-challenge.sub");
        assert_eq!(build_acme_rr("*.example.com", "example.com"), "_acme-challenge");
        assert_eq!(build_acme_rr("a.b.example.com", "example.com"), "_acme-challenge.a.b");
    }
}
