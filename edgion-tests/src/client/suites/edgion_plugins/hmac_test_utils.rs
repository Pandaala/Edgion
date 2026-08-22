use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub(super) struct HmacRequestHeaders {
    pub date: String,
    pub digest: Option<String>,
    pub authorization: String,
}

pub(super) fn build_hmac_headers(
    username: &str,
    secret: &[u8],
    method: &str,
    path: &str,
    query: Option<&str>,
    host: &str,
    body: Option<&[u8]>,
) -> HmacRequestHeaders {
    let mut request_target = path.to_string();
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        request_target.push('?');
        request_target.push_str(query);
    }

    let date = Utc::now().to_rfc2822();
    let digest = body.map(|body| format!("SHA-256={}", STANDARD.encode(Sha256::digest(body))));
    let mut signed_headers = vec!["@request-target", "host", "date"];
    let mut signing_lines = vec![
        format!("@request-target: {} {request_target}", method.to_ascii_lowercase()),
        format!("host: {host}"),
        format!("date: {date}"),
    ];
    if let Some(digest) = &digest {
        signed_headers.push("digest");
        signing_lines.push(format!("digest: {digest}"));
    }

    let mut mac = HmacSha256::new_from_slice(secret).expect("static HMAC key is valid");
    mac.update(signing_lines.join("\n").as_bytes());
    let signature = STANDARD.encode(mac.finalize().into_bytes());
    let authorization = format!(
        "hmac username=\"{username}\", algorithm=\"hmac-sha256\", headers=\"{}\", signature=\"{signature}\"",
        signed_headers.join(" ")
    );

    HmacRequestHeaders {
        date,
        digest,
        authorization,
    }
}
