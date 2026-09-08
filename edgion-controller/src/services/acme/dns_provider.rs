//! DNS-01 Challenge Provider Trait and Implementations
//!
//! Provides a trait for DNS-01 challenge validation by creating/removing
//! TXT records at `_acme-challenge.<domain>`.

use std::collections::HashSet;
use std::future::Future;
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

const ALIDNS_CLEANUP_PAGE_SIZE: u64 = 100;
const MAX_ALIDNS_CLEANUP_PAGES: u64 = 100;
const MAX_ALIDNS_CLEANUP_RECORDS: u64 = 4096;
const MAX_ALIDNS_CLEANUP_ID_BYTES: usize = 256 * 1024;

/// Finish the bounded lookup before deleting anything: deletion must not shift
/// records between pages. External edits are not snapshot-isolated by AliDNS;
/// reject detectable changes rather than claiming a complete selection.
async fn cleanup_alidns_records<L, LFut, F, Fut>(
    rr: &str,
    value: &str,
    mut list_records: L,
    mut delete_record: F,
) -> Result<()>
where
    L: FnMut(u64) -> LFut,
    LFut: Future<Output = reqwest::Result<reqwest::Response>>,
    F: FnMut(&str) -> Fut,
    Fut: Future<Output = reqwest::Result<reqwest::Response>>,
{
    let mut page = 1u64;
    let mut metadata = None;
    let mut seen_ids = HashSet::new();
    let mut record_ids = Vec::new();
    let mut id_bytes = 0usize;
    loop {
        if page > MAX_ALIDNS_CLEANUP_PAGES {
            anyhow::bail!("AliDNS describe-records pagination limit exceeded");
        }
        let response = list_records(page)
            .await
            .map_err(|error| dns_transport_error(error, "AliDNS describe-records"))?;
        let body = read_json_limited(response, "AliDNS describe-records").await?;
        check_alidns_response(&body, "AliDNS describe-records")?;
        let records = body["DomainRecords"]["Record"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("AliDNS describe-records invalid record list"))?;
        let number = |key: &str| {
            body[key]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("AliDNS describe-records invalid pagination"))
        };
        let total = number("TotalCount")?;
        let size = number("PageSize")?;
        if number("PageNumber")? != page || size == 0 || size > ALIDNS_CLEANUP_PAGE_SIZE {
            anyhow::bail!("AliDNS describe-records invalid pagination");
        }
        if total > MAX_ALIDNS_CLEANUP_RECORDS {
            anyhow::bail!("AliDNS describe-records record limit exceeded");
        }
        if metadata.is_some_and(|previous| previous != (total, size)) {
            anyhow::bail!("AliDNS describe-records pagination changed");
        }
        metadata = Some((total, size));
        let offset = page
            .checked_sub(1)
            .and_then(|n| n.checked_mul(size))
            .ok_or_else(|| anyhow::anyhow!("AliDNS describe-records invalid pagination"))?;
        let remaining = total
            .checked_sub(offset)
            .ok_or_else(|| anyhow::anyhow!("AliDNS describe-records invalid pagination"))?;
        if records.len() as u64 != remaining.min(size) {
            anyhow::bail!("AliDNS describe-records incomplete page");
        }
        for record in records {
            let id = record["RecordId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| anyhow::anyhow!("AliDNS describe-records invalid record ID"))?;
            // Count both retained copies for selected records against the byte cap.
            let field = |key: &str| {
                record[key]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("AliDNS describe-records invalid record fields"))
            };
            let record_rr = field("RR")?;
            let record_type = field("Type")?;
            let record_value = field("Value")?;
            let owned =
                record_rr.eq_ignore_ascii_case(rr) && record_type.eq_ignore_ascii_case("TXT") && record_value == value;
            id_bytes = id_bytes
                .checked_add(id.len())
                .and_then(|n| n.checked_add(if owned { id.len() } else { 0 }))
                .filter(|n| *n <= MAX_ALIDNS_CLEANUP_ID_BYTES)
                .ok_or_else(|| anyhow::anyhow!("AliDNS describe-records record ID byte limit exceeded"))?;
            if !seen_ids.insert(id.to_owned()) {
                anyhow::bail!("AliDNS describe-records repeated record ID");
            }
            if owned {
                record_ids.push(id.to_owned());
            }
        }
        if remaining <= size {
            break;
        }
        page = page
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("AliDNS describe-records invalid pagination"))?;
    }

    let mut first_error = None;
    for record_id in record_ids {
        let result = async {
            let response = delete_record(&record_id)
                .await
                .map_err(|error| dns_transport_error(error, "AliDNS delete-record"))?;
            let body = read_json_limited(response, "AliDNS delete-record").await?;
            check_alidns_response(&body, "AliDNS delete-record")?;
            if body["RecordId"].as_str() != Some(record_id.as_str()) {
                anyhow::bail!("AliDNS delete-record invalid record ID");
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
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

    async fn remove_txt_record(&self, domain: &str, value: &str) -> Result<()> {
        let root_domain = extract_root_domain(domain);
        let rr = build_acme_rr(domain, &root_domain);

        cleanup_alidns_records(
            &rr,
            value,
            |page| {
                let mut params = vec![
                    ("Action".to_string(), "DescribeDomainRecords".to_string()),
                    ("DomainName".to_string(), root_domain.clone()),
                    ("RRKeyWord".to_string(), rr.clone()),
                    ("TypeKeyWord".to_string(), "TXT".to_string()),
                    ("PageNumber".to_string(), page.to_string()),
                    ("PageSize".to_string(), ALIDNS_CLEANUP_PAGE_SIZE.to_string()),
                ];
                self.sign_request(&mut params);
                self.client.get("https://alidns.aliyuncs.com/").query(&params).send()
            },
            |record_id| {
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
            },
        )
        .await
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

    async fn cleanup_alidns_single_page<F, Fut>(response: reqwest::Response, delete_record: F) -> Result<()>
    where
        F: FnMut(&str) -> Fut,
        Fut: Future<Output = reqwest::Result<reqwest::Response>>,
    {
        let mut response = Some(response);
        cleanup_alidns_records(
            "_acme-challenge",
            "digest",
            |page| {
                assert_eq!(page, 1);
                std::future::ready(Ok(response.take().expect("one lookup")))
            },
            delete_record,
        )
        .await
    }

    fn alidns_records(ids: &[&str]) -> reqwest::Response {
        let records: Vec<_> = ids
            .iter()
            .map(|id| serde_json::json!({"RecordId": id, "RR": "_acme-challenge", "Type": "TXT", "Value": "digest"}))
            .collect();
        response(
            200,
            serde_json::json!({"TotalCount": ids.len(), "PageSize": 100, "PageNumber": 1, "DomainRecords": {"Record": records}}).to_string(),
        )
    }

    fn cleanup_record(id: &str, rr: &str, kind: &str, value: &str) -> serde_json::Value {
        serde_json::json!({"RecordId": id, "RR": rr, "Type": kind, "Value": value})
    }

    fn cleanup_page(page: u64, size: u64, total: u64, records: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({"PageNumber": page, "PageSize": size, "TotalCount": total,
            "DomainRecords": {"Record": records}})
    }

    #[tokio::test]
    async fn alidns_cleanup_selects_exact_ownership_before_any_deletion() {
        let records = std::cell::RefCell::new(vec![
            cleanup_record("near-rr", "_acme-challenge.sub", "TXT", "digest"),
            cleanup_record("other-value", "_acme-challenge", "TXT", "Digest"),
            cleanup_record("first", "_ACME-CHALLENGE", "txt", "digest"),
            cleanup_record("other-type", "_acme-challenge", "CNAME", "digest"),
            cleanup_record("second", "_acme-challenge", "TXT", "digest"),
        ]);
        let pages = std::cell::RefCell::new(Vec::new());
        let mut deleted = Vec::new();
        cleanup_alidns_records(
            "_acme-challenge",
            "digest",
            |page| {
                pages.borrow_mut().push(page);
                let records = records.borrow();
                let entries = records
                    .iter()
                    .skip(((page - 1) * 2) as usize)
                    .take(2)
                    .cloned()
                    .collect();
                std::future::ready(Ok(response(
                    200,
                    cleanup_page(page, 2, records.len() as u64, entries).to_string(),
                )))
            },
            |id| {
                assert_eq!(*pages.borrow(), [1, 2, 3], "finish listing before deleting");
                records.borrow_mut().retain(|record| record["RecordId"] != id);
                deleted.push(id.to_owned());
                std::future::ready(Ok(response(200, serde_json::json!({"RecordId": id}).to_string())))
            },
        )
        .await
        .unwrap();
        assert_eq!(deleted, ["first", "second"]);
        assert_eq!(records.borrow().len(), 3);
    }

    #[tokio::test]
    async fn alidns_cleanup_rejects_bad_pagination_and_fields_without_deletion() {
        let valid = cleanup_page(
            1,
            1,
            1,
            vec![cleanup_record("first", "_acme-challenge", "TXT", "digest")],
        );
        let mut cases = Vec::new();
        for key in ["PageNumber", "PageSize", "TotalCount"] {
            for invalid in [
                serde_json::Value::Null,
                serde_json::json!(-1),
                serde_json::json!(1.5),
                serde_json::json!(SECRET_CANARY),
            ] {
                let mut body = valid.clone();
                body[key] = invalid;
                cases.push((body, "invalid pagination"));
            }
            let mut body = valid.clone();
            body.as_object_mut().unwrap().remove(key);
            cases.push((body, "invalid pagination"));
        }
        for (key, value, error) in [
            ("PageNumber", 0, "invalid pagination"),
            ("PageNumber", u64::MAX, "invalid pagination"),
            ("PageSize", 0, "invalid pagination"),
            ("PageSize", u64::MAX, "invalid pagination"),
            ("TotalCount", u64::MAX, "record limit exceeded"),
            ("TotalCount", MAX_ALIDNS_CLEANUP_RECORDS + 1, "record limit exceeded"),
            ("TotalCount", 0, "incomplete page"),
        ] {
            let mut body = valid.clone();
            body[key] = serde_json::json!(value);
            cases.push((body, error));
        }
        cases.push((cleanup_page(1, 1, 1, vec![]), "incomplete page"));
        for key in ["RR", "Type", "Value"] {
            for invalid in [serde_json::Value::Null, serde_json::json!(123)] {
                let mut body = valid.clone();
                body["DomainRecords"]["Record"][0][key] = invalid;
                cases.push((body, "invalid record fields"));
            }
        }
        for (body, expected) in cases {
            let error = cleanup_alidns_single_page(
                response(200, body.to_string()),
                |_| -> std::future::Ready<reqwest::Result<reqwest::Response>> {
                    panic!("invalid lookup must not delete");
                },
            )
            .await
            .unwrap_err();
            assert_safe_error(error, &format!("AliDNS describe-records {expected}"));
        }
    }

    #[tokio::test]
    async fn alidns_cleanup_rejects_changed_repeated_and_failed_later_pages() {
        let first = cleanup_page(
            1,
            1,
            2,
            vec![cleanup_record("first", "_acme-challenge", "TXT", "digest")],
        );
        let second = cleanup_page(
            2,
            1,
            2,
            vec![cleanup_record("second", "_acme-challenge", "TXT", "digest")],
        );
        let mut cases = vec![
            (first.clone(), "invalid pagination"),
            (cleanup_page(2, 1, 2, vec![]), "incomplete page"),
            (
                cleanup_page(2, 1, 2, vec![cleanup_record("first", "other", "TXT", "other")]),
                "repeated record ID",
            ),
        ];
        for (key, value) in [("TotalCount", 3), ("PageSize", 2)] {
            let mut changed = second.clone();
            changed[key] = serde_json::json!(value);
            cases.push((changed, "pagination changed"));
        }
        let mut responses: Vec<_> = cases
            .into_iter()
            .map(|(body, error)| (response(200, body.to_string()), error.to_owned()))
            .collect();
        responses.extend(alidns_response_failures());
        for (later, expected) in responses {
            let mut responses = vec![response(200, first.to_string()), later].into_iter();
            let error = cleanup_alidns_records(
                "_acme-challenge",
                "digest",
                |_| std::future::ready(Ok(responses.next().expect("two pages only"))),
                |_| -> std::future::Ready<reqwest::Result<reqwest::Response>> {
                    panic!("later lookup failure must prevent all deletion");
                },
            )
            .await
            .unwrap_err();
            assert_safe_error(error, &format!("AliDNS describe-records {expected}"));
        }
    }

    #[tokio::test]
    async fn alidns_cleanup_enforces_page_record_and_retained_byte_limits() {
        for total in [
            MAX_ALIDNS_CLEANUP_PAGES,
            MAX_ALIDNS_CLEANUP_PAGES + 1,
            MAX_ALIDNS_CLEANUP_RECORDS,
        ] {
            let size = if total == MAX_ALIDNS_CLEANUP_RECORDS {
                ALIDNS_CLEANUP_PAGE_SIZE
            } else {
                1
            };
            let mut looked_up = 0;
            let mut deleted = 0;
            let result = cleanup_alidns_records(
                "_acme-challenge",
                "digest",
                |page| {
                    looked_up += 1;
                    let start = (page - 1) * size;
                    let records = (start..total.min(start + size))
                        .map(|n| cleanup_record(&n.to_string(), "_acme-challenge", "TXT", "digest"))
                        .collect();
                    std::future::ready(Ok(response(200, cleanup_page(page, size, total, records).to_string())))
                },
                |id| {
                    deleted += 1;
                    std::future::ready(Ok(response(200, serde_json::json!({"RecordId": id}).to_string())))
                },
            )
            .await;
            if total == MAX_ALIDNS_CLEANUP_PAGES + 1 {
                assert_safe_error(result.unwrap_err(), "AliDNS describe-records pagination limit exceeded");
                assert_eq!(looked_up, MAX_ALIDNS_CLEANUP_PAGES);
                assert_eq!(deleted, 0);
            } else {
                result.unwrap();
                assert_eq!(deleted, total);
            }
        }
        for extra in [0, 1] {
            let id = "x".repeat(MAX_ALIDNS_CLEANUP_ID_BYTES / 2 + extra);
            let body = cleanup_page(1, 100, 1, vec![cleanup_record(&id, "_acme-challenge", "TXT", "digest")]);
            let mut deleted = 0;
            let result = cleanup_alidns_single_page(response(200, body.to_string()), |id| {
                deleted += 1;
                std::future::ready(Ok(response(200, serde_json::json!({"RecordId": id}).to_string())))
            })
            .await;
            if extra == 0 {
                result.unwrap();
                assert_eq!(deleted, 1);
            } else {
                assert_safe_error(
                    result.unwrap_err(),
                    "AliDNS describe-records record ID byte limit exceeded",
                );
                assert_eq!(deleted, 0);
            }
        }
    }

    fn alidns_response_failures() -> Vec<(reqwest::Response, String)> {
        let unread_body = reqwest::Body::wrap_stream(futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<std::io::Result<bytes::Bytes>>> {
                panic!("HTTP rejection must not poll its body");
            },
        ));
        let oversized: reqwest::Response = http::Response::builder()
            .header("content-length", MAX_DNS_RESPONSE_BYTES + 1)
            .body(reqwest::Body::from(vec![b' '; MAX_DNS_RESPONSE_BYTES + 1]))
            .unwrap()
            .into();
        let chunks = futures::stream::iter([
            Ok::<_, std::io::Error>(bytes::Bytes::from(vec![b' '; MAX_DNS_RESPONSE_BYTES])),
            Ok(bytes::Bytes::from_static(SECRET_CANARY.as_bytes())),
        ]);
        let broken = futures::stream::iter([Err::<bytes::Bytes, _>(std::io::Error::other(SECRET_CANARY))]);
        vec![
            (
                response(403, unread_body),
                "HTTP request rejected (status=403)".to_string(),
            ),
            (
                response(
                    200,
                    serde_json::json!({
                        "Code": IDENTITY_CANARY, "Message": SECRET_CANARY,
                        "DomainRecords": {"Record": []}, "RecordId": "first"
                    })
                    .to_string(),
                ),
                "provider rejected request".to_string(),
            ),
            (
                response(200, format!("{{\"{SECRET_CANARY}\":{IDENTITY_CANARY}")),
                "invalid JSON response".to_string(),
            ),
            (
                oversized,
                format!(
                    "response too large: Content-Length {} > limit {} bytes",
                    MAX_DNS_RESPONSE_BYTES + 1,
                    MAX_DNS_RESPONSE_BYTES
                ),
            ),
            (
                response(200, reqwest::Body::wrap_stream(chunks)),
                format!("response body exceeds limit {} bytes", MAX_DNS_RESPONSE_BYTES),
            ),
            (
                response(200, reqwest::Body::wrap_stream(broken)),
                "response body read failed".to_string(),
            ),
        ]
    }

    #[tokio::test]
    async fn alidns_cleanup_rejects_failed_lookup_before_deletion() {
        for (lookup, expected) in alidns_response_failures() {
            let error =
                cleanup_alidns_single_page(lookup, |_| -> std::future::Ready<reqwest::Result<reqwest::Response>> {
                    panic!("a rejected lookup must not delete records")
                })
                .await
                .unwrap_err();
            assert_safe_error(error, &format!("AliDNS describe-records {expected}"));
        }
    }

    #[tokio::test]
    async fn alidns_cleanup_validates_all_record_ids_before_deletion() {
        for body in [
            "null",
            "[]",
            "{}",
            r#"{"DomainRecords":{"Record":null}}"#,
            r#"{"DomainRecords":{"Record":{}}}"#,
        ] {
            let error = cleanup_alidns_single_page(response(200, body), |_| std::future::ready(Ok(response(200, ""))))
                .await
                .unwrap_err();
            assert_safe_error(error, "AliDNS describe-records invalid record list");
        }
        for invalid in [
            serde_json::json!({}),
            serde_json::json!({"RecordId": null}),
            serde_json::json!({"RecordId": 123}),
            serde_json::json!({"RecordId": ""}),
        ] {
            let body = serde_json::json!({"TotalCount": 2, "PageSize": 100, "PageNumber": 1, "DomainRecords": {"Record": [{"RecordId": "first", "RR": "_acme-challenge", "Type": "TXT", "Value": "digest"}, invalid]}});
            let mut calls = 0;
            let error = cleanup_alidns_single_page(response(200, body.to_string()), |_| {
                calls += 1;
                std::future::ready(Ok(response(200, r#"{"RecordId":"first"}"#)))
            })
            .await
            .unwrap_err();
            assert_eq!(calls, 0);
            assert_safe_error(error, "AliDNS describe-records invalid record ID");
        }
    }

    #[tokio::test]
    async fn alidns_cleanup_accepts_empty_and_multiple_records_at_body_limit() {
        for ids in [vec![], vec!["first", "second"]] {
            let mut calls = Vec::new();
            cleanup_alidns_single_page(alidns_records(&ids), |id| {
                calls.push(id.to_string());
                let mut body = serde_json::json!({"RecordId": id}).to_string();
                // JSON trailing whitespace is valid at the exact byte limit.
                body.extend(std::iter::repeat_n(' ', MAX_DNS_RESPONSE_BYTES - body.len()));
                std::future::ready(Ok(response(200, body)))
            })
            .await
            .unwrap();
            assert_eq!(calls, ids);
        }
    }

    #[tokio::test]
    async fn alidns_cleanup_continues_after_failed_delete_and_reports_failure() {
        let mut failures = alidns_response_failures();
        for body in [
            "null",
            "[]",
            "{}",
            r#"{"RecordId":null}"#,
            r#"{"RecordId":123}"#,
            r#"{"RecordId":""}"#,
            r#"{"RecordId":"DNS_IDENTITY_CANARY"}"#,
        ] {
            failures.push((response(200, body), "invalid record ID".to_string()));
        }
        for (failed, expected) in failures {
            let mut failed = Some(failed);
            let mut calls = Vec::new();
            let error = cleanup_alidns_single_page(alidns_records(&["first", "second"]), |id| {
                calls.push(id.to_string());
                std::future::ready(Ok(failed
                    .take()
                    .unwrap_or_else(|| response(200, serde_json::json!({"RecordId": id}).to_string()))))
            })
            .await
            .unwrap_err();
            assert_eq!(calls, ["first", "second"]);
            assert_safe_error(error, &format!("AliDNS delete-record {expected}"));
        }
    }

    #[tokio::test]
    async fn alidns_cleanup_keeps_first_error_and_continues_after_transport_failure() {
        let mut calls = Vec::new();
        let error = cleanup_alidns_single_page(alidns_records(&["first", "second", "third"]), |id| {
            calls.push(id.to_string());
            let result = match id {
                "first" => reqwest::Client::new()
                    .get(format!("https://alidns.aliyuncs.com/?Signature={SECRET_CANARY}"))
                    .header("authorization", "invalid\nheader")
                    .build()
                    .map(|_| response(200, "")),
                "second" => Ok(response(403, SECRET_CANARY)),
                _ => Ok(response(200, serde_json::json!({"RecordId": id}).to_string())),
            };
            std::future::ready(result)
        })
        .await
        .unwrap_err();
        assert_eq!(calls, ["first", "second", "third"]);
        assert_safe_error(error, "AliDNS delete-record request failed");
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
