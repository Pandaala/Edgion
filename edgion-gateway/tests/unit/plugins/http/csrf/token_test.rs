use super::*;

const KEY: &str = "test-signing-key";

fn signed_token(expires: i64) -> CsrfToken {
    let random = "00".repeat(32);
    let sign = CsrfToken::gen_sign(&random, expires, KEY);
    CsrfToken { random, expires, sign }
}

#[test]
fn verify_at_preserves_the_inclusive_lifetime_boundary() {
    let token = signed_token(100);
    assert!(token.verify_at(KEY, 10, 109));
    assert!(token.verify_at(KEY, 10, 110));
    assert!(!token.verify_at(KEY, 10, 111));
}

#[test]
fn verify_at_rejects_nonpositive_lifetimes_future_tokens_and_extreme_timestamps() {
    let token = signed_token(100);
    assert!(!token.verify_at(KEY, 0, 100));
    assert!(!token.verify_at(KEY, -1, 100));
    assert!(!token.verify_at(KEY, 10, 99));

    let earliest = signed_token(i64::MIN);
    assert!(!earliest.verify_at(KEY, i64::MAX, i64::MAX));
    let latest = signed_token(i64::MAX);
    assert!(!latest.verify_at(KEY, i64::MAX, i64::MIN));
}

#[test]
fn verify_at_rejects_an_invalid_signature() {
    let mut token = signed_token(100);
    token.sign.replace_range(..1, "f");
    assert!(!token.verify_at(KEY, 10, 100));
}

#[test]
fn token_wire_shape_and_signature_input_remain_stable() {
    let token = signed_token(100);
    assert_eq!(
        token.sign,
        "9810c1eed8795dab07eda4939817f0cff36454b7a0fc7a735f41d356efbfdb95"
    );

    let encoded = token.encode().unwrap();
    let decoded_bytes = general_purpose::STANDARD.decode(&encoded).unwrap();
    let wire: serde_json::Value = serde_json::from_slice(&decoded_bytes).unwrap();
    assert_eq!(wire["random"], "00".repeat(32));
    assert_eq!(wire["expires"], 100);
    assert_eq!(wire["sign"], token.sign);

    let decoded = CsrfToken::decode(&encoded).unwrap();
    assert_eq!(decoded.random, token.random);
    assert_eq!(decoded.expires, token.expires);
    assert_eq!(decoded.sign, token.sign);
}

#[test]
fn generated_nonce_is_exactly_32_bytes() {
    let token = CsrfToken::generate(KEY);
    assert_eq!(hex::decode(token.random).unwrap().len(), 32);
}

#[test]
fn decode_rejects_invalid_base64_utf8_and_json_without_dynamic_errors() {
    assert!(CsrfToken::decode("not-base64!").is_err());
    assert!(CsrfToken::decode("/w==").is_err());
    assert!(CsrfToken::decode("bm90LWpzb24=").is_err());
}
