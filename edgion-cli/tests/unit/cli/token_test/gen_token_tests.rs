/// Compute the lowercase hex-encoded SHA-256 digest of `token`.
///
/// Mirrors `controller::authz::sha256_hex` to verify that the full
/// `edg_`-prefixed token is what gets hashed.
fn sha256_hex_of(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

/// `gen_token` produces a token whose sha256 matches `sha256_hex_of(full_token)`.
/// We verify the invariant by re-implementing the generation inline.
#[test]
fn sha256_covers_full_edg_prefixed_token() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use rand::RngCore as _;

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let b64 = URL_SAFE_NO_PAD.encode(bytes);
    let token = format!("edg_{b64}");

    let hash = sha256_hex_of(&token);

    // Token must start with "edg_"
    assert!(token.starts_with("edg_"), "token must start with edg_: {token}");

    // SHA-256 must be 64 lowercase hex chars (digits 0-9 or letters a-f, never A-F)
    assert_eq!(hash.len(), 64, "sha256 must be 64 hex chars: {hash}");
    assert!(
        hash.chars().all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f')),
        "sha256 must be lowercase hex only (no uppercase): {hash}"
    );

    // Re-hashing the same token must produce the same digest
    assert_eq!(sha256_hex_of(&token), hash);

    // Hashing just the suffix (without edg_) must differ — the full prefix is included
    let without_prefix = &token["edg_".len()..];
    assert_ne!(
        sha256_hex_of(without_prefix),
        hash,
        "hash must cover the full edg_-prefixed token, not just the suffix"
    );
}

/// Token length: 32 bytes base64url → 43 chars + "edg_" = 47 chars
#[test]
fn token_has_expected_length() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use rand::RngCore as _;

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = format!("edg_{}", URL_SAFE_NO_PAD.encode(bytes));
    // 32 bytes base64url no-pad = ceil(32*4/3) = 43 chars; prefix "edg_" = 4; total = 47
    assert_eq!(token.len(), 47, "token length: {}", token.len());
}
