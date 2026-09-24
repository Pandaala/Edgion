use std::mem::size_of;

use super::{tokenize_document, QuoteStyle, Token, TokenizeError, TokenizedLineRecord, TokenizerLimits};
use crate::seclang::{scan_source, SourceLimits, SourcePosition};

fn tokenize(source: &[u8]) -> super::TokenizedDocument<'_> {
    let document = Box::leak(Box::new(scan_source("test", source, SourceLimits::default()).unwrap()));
    tokenize_document(document, TokenizerLimits::default()).unwrap()
}

#[test]
fn tokenizes_quotes_separators_and_empty_values() {
    let result = tokenize(b"SecRule\tARGS  '@pm one' \"\"");
    let tokens = result.tokens();
    assert_eq!(
        tokens.iter().map(Token::raw).collect::<Vec<_>>(),
        ["SecRule", "ARGS", "'@pm one'", "\"\""]
    );
    assert_eq!(
        tokens.iter().map(Token::value).collect::<Vec<_>>(),
        ["SecRule", "ARGS", "@pm one", ""]
    );
    assert_eq!(
        tokens.iter().map(Token::quote_style).collect::<Vec<_>>(),
        [
            QuoteStyle::Unquoted,
            QuoteStyle::Unquoted,
            QuoteStyle::Single,
            QuoteStyle::Double
        ]
    );
    assert_eq!(tokens[0].raw_logical_range(), 0..7);
    assert_eq!(tokens[2].raw_logical_range(), 14..23);
    assert_eq!(tokens[2].content_logical_range(), 15..22);
    assert_eq!(tokens[3].raw_logical_range(), 24..26);
    assert_eq!(tokens[3].content_logical_range(), 25..25);
}

#[test]
fn preserves_escapes_inline_hash_and_unquoted_selector_quotes() {
    let result = tokenize(br##"SecRule ARGS:a\ b|REQUEST_HEADERS:'/xy/' "@rx a\s+#b""##);
    assert_eq!(
        result.tokens().iter().map(Token::value).collect::<Vec<_>>(),
        ["SecRule", "ARGS:a\\ b|REQUEST_HEADERS:'/xy/'", "@rx a\\s+#b"]
    );
    assert_eq!(result.tokens()[2].raw(), r#""@rx a\s+#b""#);
}

#[test]
fn uses_odd_even_backslash_runs_for_quote_closing() {
    let result = tokenize(br#""a\"b" "c\\" "d\\\"e""#);
    assert_eq!(
        result.tokens().iter().map(Token::value).collect::<Vec<_>>(),
        [r#"a\"b"#, r#"c\\"#, r#"d\\\"e"#]
    );
}

#[test]
fn retains_adjacent_quoted_tokens_and_rejects_suffixes() {
    let result = tokenize(br#""one"'two'"#);
    assert_eq!(
        result.tokens().iter().map(Token::value).collect::<Vec<_>>(),
        ["one", "two"]
    );

    let document = scan_source("suffix", br#""one"tail"#, SourceLimits::default()).unwrap();
    assert!(matches!(
        tokenize_document(&document, TokenizerLimits::default()),
        Err(TokenizeError::CharactersAfterClosingQuote {
            position: SourcePosition { column: 6, .. }
        })
    ));
}

#[test]
fn maps_continued_token_endpoints_and_empty_content_directionally() {
    let document = scan_source("joins", b"SecRule A\\\nB \"\\\n\"", SourceLimits::default()).unwrap();
    let result = tokenize_document(&document, TokenizerLimits::default()).unwrap();
    let spanning = &result.tokens()[1];
    assert_eq!(spanning.value(), "AB");
    assert_eq!(spanning.raw_span().start.line, 1);
    assert_eq!(spanning.raw_span().end.line, 2);

    let empty = &result.tokens()[2];
    assert_eq!(empty.value(), "");
    assert_eq!(empty.content_span().start.line, 3);
    assert_eq!(empty.content_span().start, empty.content_span().end);
}

#[test]
fn keeps_multibyte_utf8_borrowed_from_the_source_document() {
    let document = scan_source("utf8", "SecMarker café".as_bytes(), SourceLimits::default()).unwrap();
    let source_pointer = document.lines()[0].text().as_ptr();
    let result = tokenize_document(&document, TokenizerLimits::default()).unwrap();
    assert_eq!(result.tokens()[1].value(), "café");
    assert_eq!(result.tokens()[0].raw().as_ptr(), source_pointer);
    assert!(std::ptr::eq(result.source(), &document));
}

#[test]
fn enforces_total_and_per_line_token_limits_at_first_excess() {
    let per_line_document = scan_source("per-line", b"a b c", SourceLimits::default()).unwrap();
    let per_line_limits = TokenizerLimits {
        max_tokens_per_line: 2,
        ..TokenizerLimits::default()
    };
    assert!(matches!(
        tokenize_document(&per_line_document, per_line_limits),
        Err(TokenizeError::LineTokenLimit {
            position: SourcePosition { column: 5, .. },
            ..
        })
    ));

    let total_document = scan_source("total", b"a b\nc", SourceLimits::default()).unwrap();
    let total_limits = TokenizerLimits {
        max_tokens: 2,
        ..TokenizerLimits::default()
    };
    assert!(matches!(
        tokenize_document(&total_document, total_limits),
        Err(TokenizeError::TokenLimit {
            position: SourcePosition { line: 2, column: 1, .. },
            ..
        })
    ));

    let malformed_document = scan_source("precedence", b"\"unterminated", SourceLimits::default()).unwrap();
    let no_tokens = TokenizerLimits {
        max_tokens: 0,
        max_token_bytes: 1,
        ..TokenizerLimits::default()
    };
    assert!(matches!(
        tokenize_document(&malformed_document, no_tokens),
        Err(TokenizeError::TokenLimit {
            position: SourcePosition { column: 1, .. },
            ..
        })
    ));

    let second_malformed = scan_source("precedence", b"ok \"unterminated", SourceLimits::default()).unwrap();
    let one_per_line = TokenizerLimits {
        max_tokens_per_line: 1,
        max_token_bytes: 2,
        ..TokenizerLimits::default()
    };
    assert!(matches!(
        tokenize_document(&second_malformed, one_per_line),
        Err(TokenizeError::LineTokenLimit {
            position: SourcePosition { column: 4, .. },
            ..
        })
    ));
}

#[test]
fn applies_raw_token_byte_limit_to_all_quote_styles() {
    for source in [&b"abc"[..], &b"'a'"[..], &b"\"a\""[..]] {
        let document = scan_source("bytes", source, SourceLimits::default()).unwrap();
        let exact = TokenizerLimits {
            max_token_bytes: 3,
            ..TokenizerLimits::default()
        };
        assert!(tokenize_document(&document, exact).is_ok());
        let short = TokenizerLimits {
            max_token_bytes: 2,
            ..TokenizerLimits::default()
        };
        assert!(matches!(
            tokenize_document(&document, short),
            Err(TokenizeError::TokenByteLimit {
                position: SourcePosition { column: 3, .. },
                ..
            })
        ));
    }

    let utf8_document = scan_source("utf8-limit", "éx".as_bytes(), SourceLimits::default()).unwrap();
    let utf8_limit = TokenizerLimits {
        max_token_bytes: 2,
        ..TokenizerLimits::default()
    };
    assert!(matches!(
        tokenize_document(&utf8_document, utf8_limit),
        Err(TokenizeError::TokenByteLimit {
            position: SourcePosition {
                column: 3,
                byte_offset: 2,
                ..
            },
            ..
        })
    ));
}

#[test]
fn reports_dangling_escapes_unterminated_quotes_and_content_free_errors() {
    let dangling = scan_source("dangling", b"abc\\ ", SourceLimits::default()).unwrap_err();
    assert!(matches!(
        dangling,
        crate::seclang::SourceError::UnfinishedContinuation { .. }
    ));

    let document = scan_source("quote", br#""private value"#, SourceLimits::default()).unwrap();
    let error = tokenize_document(&document, TokenizerLimits::default()).unwrap_err();
    assert!(matches!(error, TokenizeError::UnterminatedQuote { .. }));
    assert!(!error.to_string().contains("private"));
    assert!(!format!("{error:?}").contains("private"));
}

#[test]
fn bounds_flat_metadata_for_many_tiny_tokens() {
    let document = scan_source("tiny", b"a b c", SourceLimits::default()).unwrap();
    let exact_charge = size_of::<TokenizedLineRecord>() + 3 * size_of::<Token<'_>>();
    let exact = TokenizerLimits {
        max_owned_bytes: exact_charge,
        ..TokenizerLimits::default()
    };
    let result = tokenize_document(&document, exact).unwrap();
    assert_eq!(result.charged_bytes(), exact_charge);
    assert_eq!(result.line_count(), 1);
    assert_eq!(result.line(0).unwrap().tokens().len(), 3);
    assert!(result.line(1).is_none());

    let short = TokenizerLimits {
        max_owned_bytes: exact_charge - 1,
        ..TokenizerLimits::default()
    };
    assert!(matches!(
        tokenize_document(&document, short),
        Err(TokenizeError::OwnedByteLimit { .. })
    ));
}
