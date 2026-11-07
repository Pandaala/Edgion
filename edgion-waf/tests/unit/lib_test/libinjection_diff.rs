use super::{sqli, xss};
use std::fs;

#[test]
#[ignore = "requires the pinned upstream fixture directory"]
fn compare_upstream_fixtures() {
    let dir = std::env::var("LIBINJECTION_GO_FIXTURES").expect("fixture directory");
    let mut mismatches = Vec::new();
    let mut count = 0;
    for entry in fs::read_dir(dir).expect("read fixtures") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let name = path.file_name().expect("file name").to_string_lossy();
        if !(name.contains("-sqli-") || name.contains("-xss-")) {
            continue;
        }
        let source = fs::read_to_string(&path).expect("utf8 fixture");
        let Some((_, body)) = source.split_once("--INPUT--\n") else {
            continue;
        };
        let Some((input, expected)) = body.split_once("--EXPECTED--\n") else {
            continue;
        };
        let input = input.trim_end_matches([' ', '\t', '\n', '\r']).as_bytes();
        let expected = expected.trim_end_matches([' ', '\t', '\n', '\r']);
        let actual = if name.contains("-sqli-") {
            let mut state = sqli::SqliState::new(input, sqli::SqliFlags::FLAG_NONE);
            if state.detect() {
                state.fingerprint_string()
            } else {
                String::new()
            }
        } else {
            if xss::XssDetector::new().detect(input).is_injection() {
                "1".to_owned()
            } else {
                "0".to_owned()
            }
        };
        count += 1;
        if actual != expected {
            mismatches.push(format!("{name}: expected {expected:?} actual {actual:?}"));
        }
    }
    eprintln!(
        "count={count} mismatches={}\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    assert!(mismatches.is_empty());
}

#[test]
fn compare_generated_corpus() {
    let content = if let Ok(path) = std::env::var("LIBINJECTION_GO_CORPUS") {
        fs::read(path).expect("read corpus")
    } else {
        include_bytes!("../../fixtures/libinjection-go-v0.3.2.json").to_vec()
    };
    let cases: serde_json::Value = serde_json::from_slice(&content).expect("json corpus");
    let cases = cases.as_array().expect("array");
    let mut mismatches = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let input = case["input"].as_str().expect("input");
        let mut state = sqli::SqliState::new(input.as_bytes(), sqli::SqliFlags::FLAG_NONE);
        let hit = state.detect();
        let fingerprint = if hit { state.fingerprint_string() } else { String::new() };
        let xss_hit = xss::XssDetector::new().detect(input.as_bytes()).is_injection();
        if hit != case["sqli"].as_bool().expect("sqli")
            || fingerprint != case["fingerprint"].as_str().expect("fingerprint")
            || xss_hit != case["xss"].as_bool().expect("xss")
        {
            mismatches.push(format!(
                "{index} {input:?}: sqli {hit} {fingerprint:?}, xss {xss_hit}; expected sqli {} {:?}, xss {}",
                case["sqli"], case["fingerprint"], case["xss"]
            ));
        }
    }
    eprintln!(
        "count={} mismatches={}\n{}",
        cases.len(),
        mismatches.len(),
        mismatches.iter().take(100).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(mismatches.is_empty());
}

/// Inputs where Edgion intentionally differs from the pinned libinjection-go
/// v0.3.2 oracle because it follows libinjection C. Each entry is
/// `(input, edgion_sqli_fingerprint_or_empty, oracle_sqli_fingerprint)`.
///
/// F-S4-2: Go `parseStringCore` checks for a backslash escape before the FIRST
/// occurrence of the remaining suffix (`strings.Index`), so a bare closing quote
/// at the end of input is misjudged as escaped when an earlier `\'` exists in
/// the same literal. The literal stays unclosed and Go's 3-token `sos`/`s&s`
/// whitelist reports SQLi. C scans back from the actual quote position, closes
/// the literal, and whitelists the shape. Still present in v0.3.3. Go is the
/// false-positive side; attack-shaped variants agree on both sides.
const KNOWN_ORACLE_DIVERGENCES: &[(&[u8], &str, &str)] = &[(b"x'||'a\\'b'", "", "s&s"), (b"'^ 'a\\'b'", "", "sos")];

/// Attack-shaped neighbours of the divergence above, where Edgion and the
/// oracle agree on the SQLi fingerprint.
const BACKSLASH_QUOTE_AGREEMENT: &[(&[u8], &str)] = &[(b"'a\\'b' or 1=1--", "s&1c"), (b"' or 'a\\'b'='a\\'b", "s&sos")];

#[test]
fn known_oracle_divergences_follow_libinjection_c() {
    for &(input, edgion, oracle) in KNOWN_ORACLE_DIVERGENCES {
        let mut state = sqli::SqliState::new(input, sqli::SqliFlags::FLAG_NONE);
        let fingerprint = if state.detect() {
            state.fingerprint_string()
        } else {
            String::new()
        };
        assert_eq!(fingerprint, edgion, "{input:?} (pinned Go oracle reports {oracle:?})");
        assert_ne!(edgion, oracle);
    }
    for &(input, oracle) in BACKSLASH_QUOTE_AGREEMENT {
        let mut state = sqli::SqliState::new(input, sqli::SqliFlags::FLAG_NONE);
        assert!(state.detect(), "{input:?}");
        assert_eq!(state.fingerprint_string(), oracle, "{input:?}");
    }
}

#[test]
#[ignore = "requires the pinned upstream fixture directory"]
fn compare_all_upstream_token_fixtures() {
    let dir = std::env::var("LIBINJECTION_GO_FIXTURES").expect("fixture directory");
    let mut mismatches = Vec::new();
    let mut count = 0;
    for entry in fs::read_dir(dir).expect("read fixtures") {
        let path = entry.expect("entry").path();
        let name = path.file_name().expect("file name").to_string_lossy();
        if !(name.contains("-tokens-") || name.contains("-folding-") || name.contains("-html5-")) {
            continue;
        }
        let source = fs::read(&path).expect("fixture");
        let Some(begin) = source.windows(10).position(|w| w == b"--INPUT--\n") else {
            continue;
        };
        let begin = begin + 10;
        let Some(end) = source[begin..].windows(13).position(|w| w == b"--EXPECTED--\n") else {
            continue;
        };
        let end = begin + end;
        let input = source[begin..end].trim_ascii_end();
        let expected = source[end + 13..].trim_ascii_end();
        let actual = parser_fixture_output(&name, input);
        count += 1;
        if actual.as_slice() != expected {
            mismatches.push(format!(
                "{name}: expected {:?} actual {:?}",
                String::from_utf8_lossy(expected),
                String::from_utf8_lossy(&actual)
            ));
        }
    }
    eprintln!(
        "count={count} mismatches={}\n{}",
        mismatches.len(),
        mismatches.iter().take(100).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(mismatches.is_empty());
}

#[test]
fn compare_offline_parser_fixtures() {
    let cases: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../fixtures/libinjection-go-v0.3.2-parser.json"))
            .expect("offline parser corpus");
    let mut count = 0;
    for case in cases.as_array().expect("fixture array") {
        let name = case["source"].as_str().expect("source");
        if !(name.contains("-tokens-") || name.contains("-folding-") || name.contains("-html5-")) {
            continue;
        }
        let input = case["input"].as_str().expect("input");
        let expected = case["expected"].as_str().expect("expected");
        let actual = parser_fixture_output(name, input.as_bytes());
        assert_eq!(actual.as_slice(), expected.as_bytes(), "{name}");
        count += 1;
    }
    assert_eq!(count, 431);
}

fn parser_fixture_output(name: &str, input: &[u8]) -> Vec<u8> {
    let mut actual = Vec::new();
    if name.contains("-html5-") {
        let mut h = xss::html5::Html5State::new(input, xss::html5::Html5Flags::DataState);
        let mut steps = 0;
        while h.next() {
            steps += 1;
            assert!(steps <= input.len() * 4 + 8, "{name}: HTML tokenizer did not advance");
            actual.extend_from_slice(h.token_type.to_string().as_bytes());
            actual.push(b',');
            actual.extend_from_slice(h.token_len.to_string().as_bytes());
            actual.push(b',');
            actual.extend_from_slice(h.token_start.get(..h.token_len).expect("token range"));
            actual.push(b'\n');
        }
    } else {
        let flags = sqli::SqliFlags::FLAG_QUOTE_NONE | sqli::SqliFlags::FLAG_SQL_ANSI;
        if name.contains("-folding-") {
            let mut state = sqli::SqliState::new(input, flags);
            let n = state.fold_tokens();
            for token in state.tokens.iter().take(n) {
                append_sql_token(&mut actual, token);
            }
        } else {
            let mut tokenizer = sqli::SqliTokenizer::new(input, flags);
            let mut steps = 0;
            while let Some(token) = tokenizer.next_token() {
                steps += 1;
                assert!(steps <= input.len() * 4 + 8, "{name}: SQL tokenizer did not advance");
                append_sql_token(&mut actual, &token);
            }
        }
    }
    actual.trim_ascii_end().to_vec()
}

fn append_sql_token(out: &mut Vec<u8>, token: &sqli::Token) {
    out.push(token.token_type.to_char() as u8);
    out.push(b' ');
    if token.token_type == sqli::TokenType::Variable {
        for _ in 0..token.count {
            out.push(b'@');
        }
    }
    if matches!(token.token_type, sqli::TokenType::String | sqli::TokenType::Variable) && token.str_open != 0 {
        out.push(token.str_open);
    }
    out.extend_from_slice(&token.val[..token.len.min(31)]);
    if matches!(token.token_type, sqli::TokenType::String | sqli::TokenType::Variable) && token.str_close != 0 {
        out.push(token.str_close);
    }
    out.push(b'\n');
}

#[test]
fn randomized_byte_inputs_do_not_panic() {
    let mut seed = 0xd143_7528_91ab_cdef_u64;
    for length in 0..256 {
        for _ in 0..40 {
            let mut bytes = Vec::with_capacity(length);
            for _ in 0..length {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                bytes.push(seed as u8);
            }
            let mut state = sqli::SqliState::new(&bytes, sqli::SqliFlags::FLAG_NONE);
            state.detect();
            xss::XssDetector::new().detect(&bytes);
        }
    }
}
