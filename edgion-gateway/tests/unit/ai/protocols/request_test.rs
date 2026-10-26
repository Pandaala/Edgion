use super::*;

#[test]
fn native_utf8_admission_borrows_original_body_without_allocation() {
    let body = r#" {"model":"m","extension":"é€"} "#.as_bytes();
    let (text, stats) = crate::test_allocation_counter::count_allocation_stats(|| native_json_utf8(body).unwrap());
    assert_eq!(text.as_ptr(), body.as_ptr());
    assert_eq!(text.len(), body.len());
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
}

#[test]
fn native_utf8_admission_rejects_invalid_bytes_anywhere() {
    for body in [
        b"{\"model\":\"m\",\"extension\":\"\xff\"}".as_slice(),
        b"{\"model\":\"m\",\"extension\":{\"items\":[\"\xff\"]}}",
    ] {
        assert!(matches!(native_json_utf8(body), Err(AiProxyError::MalformedBody)));
    }
}

fn previous_decode_model(raw: Option<&RawValue>) -> Result<String, AiProxyError> {
    let raw = raw.ok_or(AiProxyError::ModelNotFound)?;
    if !raw.get().starts_with('"') {
        return Err(AiProxyError::MalformedBody);
    }
    if raw.get().len() > AI_MODEL_NAME_MAX_BYTES * 6 + 2 {
        return Err(AiProxyError::ModelNotFound);
    }
    let model: String = serde_json::from_str(raw.get()).map_err(|_| AiProxyError::MalformedBody)?;
    if model.is_empty() || model.len() > AI_MODEL_NAME_MAX_BYTES {
        return Err(AiProxyError::ModelNotFound);
    }
    Ok(model)
}

#[test]
fn model_decoder_preserves_values_byte_limits_and_error_order() {
    assert_eq!(decode_model(None).as_deref(), previous_decode_model(None).as_deref());
    let mut tokens = [
        "null",
        "true",
        "512",
        "[]",
        "{}",
        r#""""#,
        r#""model""#,
        r#""café €""#,
        r#""m\u006fdel""#,
        r#""quote\"slash\\line\n""#,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    for length in [
        AI_MODEL_NAME_MAX_BYTES - 1,
        AI_MODEL_NAME_MAX_BYTES,
        AI_MODEL_NAME_MAX_BYTES + 1,
    ] {
        tokens.push(format!("\"{}\"", "a".repeat(length)));
        tokens.push(format!("\"{}\"", r"\u0061".repeat(length)));
    }
    for length in [255, 256, 257] {
        tokens.push(format!("\"{}\"", "é".repeat(length)));
    }
    tokens.push(format!("[{}0]", "0,".repeat(AI_MODEL_NAME_MAX_BYTES * 4)));
    tokens.push(format!(
        r#"{{"extension":"{}"}}"#,
        "a".repeat(AI_MODEL_NAME_MAX_BYTES * 7)
    ));
    for token in [r#""\uD800""#, r#""\uDC00""#] {
        if serde_json::from_str::<&RawValue>(token).is_ok() {
            tokens.push(token.to_owned());
        }
    }
    for token in tokens {
        let raw: &RawValue = serde_json::from_str(&token).unwrap();
        assert_eq!(
            decode_model(Some(raw)).as_deref(),
            previous_decode_model(Some(raw)).as_deref(),
            "{token}"
        );
    }
}

#[test]
fn native_model_rewrites_preserve_escaped_equality_and_other_bytes() {
    use crate::ai::protocols::{anthropic, openai};

    for rewrite in [openai::request::rewrite_model, anthropic::request::rewrite_model] {
        for token in [r#""model""#, r#""m\u006fdel""#] {
            let body = Bytes::from(format!(r#" {{ "model" : {token}, "extension": {{"text":"é€"}} }} "#));
            assert!(rewrite(&body, "model").unwrap().is_none());
            let changed = rewrite(&body, "other").unwrap().unwrap();
            assert_eq!(
                changed.as_ref(),
                r#" { "model" : "other", "extension": {"text":"é€"} } "#.as_bytes()
            );
        }
    }
}

#[test]
fn model_decoder_borrows_plain_text_and_owns_escaped_text() {
    use crate::test_allocation_counter::count_allocation_stats;

    for text in ["model", "café €"] {
        let token = format!("\"{text}\"");
        let raw: &RawValue = serde_json::from_str(&token).unwrap();
        let (previous, previous_stats) = count_allocation_stats(|| previous_decode_model(Some(raw)).unwrap());
        let (current, current_stats) = count_allocation_stats(|| decode_model(Some(raw)).unwrap());
        assert_eq!(current, previous);
        assert!(matches!(current, Cow::Borrowed(_)));
        assert_eq!(current.as_ptr(), raw.get().as_bytes()[1..].as_ptr());
        assert!(previous_stats.calls > 0);
        assert_eq!(current_stats.calls, 0);
        assert_eq!(current_stats.requested_bytes, 0);
    }
    let raw: &RawValue = serde_json::from_str(r#""m\u006fdel""#).unwrap();
    let decoded = decode_model(Some(raw)).unwrap();
    assert!(matches!(decoded, Cow::Owned(_)));
    assert_eq!(decoded, "model");
}

#[test]
fn unchanged_plain_model_rewrites_do_not_allocate() {
    use crate::ai::protocols::{anthropic, openai};
    use crate::test_allocation_counter::count_allocation_stats;

    let body = Bytes::from_static(br#"{"model":"model","max_tokens":1,"extension":{"keep":true}}"#);
    for rewrite in [openai::request::rewrite_model, anthropic::request::rewrite_model] {
        let (changed, stats) = count_allocation_stats(|| rewrite(&body, "model").unwrap());
        assert!(changed.is_none());
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}
