use super::*;

fn headers(lines: &[&[u8]]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for line in lines {
        headers.append(COOKIE, HeaderValue::from_bytes(line).unwrap());
    }
    headers
}

#[test]
fn lookup_preserves_duplicates_quotes_empty_and_field_scope() {
    let headers = headers(&[b"flag; token=; token=later; quoted=\"a=b\"; spaced =x", b"other=second"]);
    assert_eq!(lookup(&headers, "token", FieldScope::All), Some(""));
    assert_eq!(lookup(&headers, "quoted", FieldScope::All), Some("\"a=b\""));
    assert_eq!(lookup(&headers, "flag", FieldScope::All), None);
    assert_eq!(lookup(&headers, "spaced", FieldScope::All), None);
    assert_eq!(lookup(&headers, "other", FieldScope::First), None);
    assert_eq!(lookup(&headers, "other", FieldScope::All), Some("second"));
}

#[test]
fn lookup_stops_at_non_text_only_before_match() {
    let bad = HeaderValue::from_bytes(b"bad=\xff").unwrap();
    let mut headers = headers(&[b"first=value"]);
    headers.append(COOKIE, bad.clone());
    headers.append(COOKIE, HeaderValue::from_static("last=value"));
    assert_eq!(lookup(&headers, "first", FieldScope::All), Some("value"));
    assert_eq!(lookup(&headers, "last", FieldScope::All), None);
    assert!(matches!(
        plan_removal(&headers, &["missing"]),
        Err(CookieEditError::Unsanitizable)
    ));
    assert!(matches!(plan_removal(&headers, &[]), Ok(CookieEditPlan::NoChange)));
}

#[test]
fn no_match_preserves_all_raw_fields_and_flags() {
    let headers = headers(&[b" ; a=1;; flag ;", b"a=2; empty="]);
    let original = headers.clone();
    assert!(matches!(
        plan_removal(&headers, &["missing"]),
        Ok(CookieEditPlan::NoChange)
    ));
    assert_eq!(headers, original);
    assert!(matches!(
        plan_removal(&HeaderMap::new(), &["a"]),
        Ok(CookieEditPlan::Absent)
    ));
}

#[test]
fn deletion_removes_all_selected_names_and_preserves_survivor_semantics() {
    let headers = headers(&[b"token=secret; bare; a=\"x=y\"; token", b"a=%2F; token=other; empty="]);
    let CookieEditPlan::Replace(value) = plan_removal(&headers, &["token", "bare"]).unwrap() else {
        panic!("expected replacement");
    };
    assert_eq!(value.as_bytes(), b"a=\"x=y\"; a=%2F; empty=");
    assert!(value.is_sensitive());
    assert!(matches!(
        plan_removal(&headers, &["token", "bare", "a", "empty"]),
        Ok(CookieEditPlan::Remove)
    ));
}

#[test]
fn editing_utf8_policy_is_distinct_from_lookup_policy() {
    let headers = headers(&["token=secret; label=é".as_bytes()]);
    assert_eq!(lookup(&headers, "token", FieldScope::All), None);
    let CookieEditPlan::Replace(value) = plan_removal(&headers, &["token"]).unwrap() else {
        panic!("expected replacement");
    };
    assert_eq!(value.as_bytes(), "label=é".as_bytes());
}

// Frozen pre-refactor reader: deliberately does not use the new tokenizer.
fn old_lookup<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    for header in headers.get_all(COOKIE) {
        let cookies = header.to_str().ok()?;
        if let Some(value) = cookies.split(';').find_map(|pair| {
            let mut parts = pair.trim().splitn(2, '=');
            let key = parts.next()?;
            if key == name {
                parts.next()
            } else {
                None
            }
        }) {
            return Some(value);
        }
    }
    None
}

#[test]
fn frozen_reader_oracle_preserves_field_order_and_edge_grammar() {
    let corpus: &[&[u8]] = &[
        b"",
        b" ; ;",
        b"token=; token=second",
        b"token; token=value",
        b"=empty-name; flag",
        b"token =space; TOKEN=upper; token=lower",
        b"\ttoken=left\t; q=\"a=b\"",
        b"token=%2F+raw; nested=a=b=c",
        "\u{a0}token=value\u{a0}".as_bytes(),
        "label=é; token=value".as_bytes(),
        b"opaque=\xff; token=value",
    ];
    for first in corpus {
        for second in corpus {
            let input = headers(&[first, second]);
            for name in ["token", "TOKEN", "token ", "", "flag", "q", "nested", "missing"] {
                assert_eq!(lookup(&input, name, FieldScope::All), old_lookup(&input, name));
                let first_only = headers(&[first]);
                assert_eq!(lookup(&input, name, FieldScope::First), old_lookup(&first_only, name));
            }
        }
    }
}

#[test]
fn matched_utf8_deletion_matches_frozen_survivor_oracle() {
    let corpus = [
        "",
        " ; ;",
        "token=; token=second",
        "token; flag",
        "=empty; keep=x",
        "token =space; TOKEN=upper",
        "\tkeep=left\t; q=\"a=b\"",
        "keep=%2F+raw; nested=a=b=c",
        "\u{a0}keep=value\u{a0}",
        "label=é",
    ];
    for first in corpus {
        for second in corpus {
            // Ensure actual deletion: no-match normalization is intentionally
            // excluded from this oracle and independently covered above.
            let lines = [first, second, "token=removed"];
            let input = headers(&lines.iter().map(|line| line.as_bytes()).collect::<Vec<_>>());
            for names in [&["token"][..], &["token", "keep", ""][..]] {
                let mut survivors = Vec::new();
                for line in lines {
                    for segment in line.split(';') {
                        let trimmed = segment.trim();
                        if !trimmed.is_empty() && !names.contains(&trimmed.split('=').next().unwrap_or("")) {
                            survivors.push(trimmed);
                        }
                    }
                }
                match plan_removal(&input, names).unwrap() {
                    CookieEditPlan::Remove => assert!(survivors.is_empty()),
                    CookieEditPlan::Replace(value) => assert_eq!(value.as_bytes(), survivors.join("; ").as_bytes()),
                    other => panic!("matched deletion must publish a plan: {other:?}"),
                }
            }
        }
    }
}
