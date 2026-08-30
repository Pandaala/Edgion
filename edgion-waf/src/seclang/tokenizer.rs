use std::mem::size_of;
use std::ops::Range;

use thiserror::Error;

use super::{LogicalLine, SourceDocument, SourcePosition, SourceSpan};

/// Default maximum number of tokens in one source document.
pub const DEFAULT_MAX_TOKENS: usize = 200_000;
/// Default maximum number of tokens in one logical line.
pub const DEFAULT_MAX_TOKENS_PER_LINE: usize = 128;
/// Default maximum raw bytes in one token, including quote delimiters.
pub const DEFAULT_MAX_TOKEN_BYTES: usize = 4 * 1024 * 1024;
/// Default maximum charged bytes owned by one tokenized document.
pub const DEFAULT_MAX_TOKENIZER_OWNED_BYTES: usize = 32 * 1024 * 1024;

/// Resource limits for tokenizing one scanned SecLang document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenizerLimits {
    /// Maximum number of tokens across the document.
    pub max_tokens: usize,
    /// Maximum number of tokens in one logical line.
    pub max_tokens_per_line: usize,
    /// Maximum raw bytes in one token, including quote delimiters.
    pub max_token_bytes: usize,
    /// Maximum charged bytes owned by the returned tokenized document.
    pub max_owned_bytes: usize,
}

impl Default for TokenizerLimits {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_MAX_TOKENS,
            max_tokens_per_line: DEFAULT_MAX_TOKENS_PER_LINE,
            max_token_bytes: DEFAULT_MAX_TOKEN_BYTES,
            max_owned_bytes: DEFAULT_MAX_TOKENIZER_OWNED_BYTES,
        }
    }
}

/// The lexical quoting form of a top-level token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteStyle {
    /// The token has no enclosing quotes.
    Unquoted,
    /// The token is enclosed in single quotes.
    Single,
    /// The token is enclosed in double quotes.
    Double,
}

/// One borrowed top-level SecLang token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'source> {
    raw: &'source str,
    value: &'source str,
    quote_style: QuoteStyle,
    raw_span: SourceSpan,
    content_span: SourceSpan,
    raw_logical_start: usize,
}

impl<'source> Token<'source> {
    /// Raw normalized token text, including enclosing quotes when present.
    pub const fn raw(&self) -> &'source str {
        self.raw
    }

    /// Token content with only the enclosing top-level quotes removed.
    ///
    /// Escapes are intentionally not decoded at this lexical layer.
    pub const fn value(&self) -> &'source str {
        self.value
    }

    /// Top-level quote style.
    pub const fn quote_style(&self) -> QuoteStyle {
        self.quote_style
    }

    /// Diagnostic envelope for the raw token range.
    pub const fn raw_span(&self) -> SourceSpan {
        self.raw_span
    }

    /// Diagnostic envelope for the unquoted content range.
    pub const fn content_span(&self) -> SourceSpan {
        self.content_span
    }

    /// Half-open raw byte range in the owning normalized logical line.
    pub fn raw_logical_range(&self) -> Range<usize> {
        self.raw_logical_start..self.raw_logical_start + self.raw.len()
    }

    /// Half-open unquoted content byte range in the owning normalized logical line.
    pub fn content_logical_range(&self) -> Range<usize> {
        let content_start = self.raw_logical_start + usize::from(self.quote_style != QuoteStyle::Unquoted);
        content_start..content_start + self.value.len()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TokenizedLineRecord {
    logical_line_index: usize,
    token_start: usize,
    token_end: usize,
}

/// A borrowed view of one tokenized logical line.
#[derive(Debug, Clone, Copy)]
pub struct TokenizedLine<'view, 'source> {
    logical_line: &'view LogicalLine,
    tokens: &'view [Token<'source>],
}

impl<'view, 'source> TokenizedLine<'view, 'source> {
    /// The normalized source line represented by this view.
    pub const fn logical_line(&self) -> &'view LogicalLine {
        self.logical_line
    }

    /// Tokens in source order.
    pub const fn tokens(&self) -> &'view [Token<'source>] {
        self.tokens
    }
}

/// A bounded flat tokenization of a scanned SecLang document.
#[derive(Debug, PartialEq, Eq)]
pub struct TokenizedDocument<'source> {
    source: &'source SourceDocument,
    tokens: Vec<Token<'source>>,
    lines: Vec<TokenizedLineRecord>,
    charged_bytes: usize,
}

impl<'source> TokenizedDocument<'source> {
    /// The scanned source document that owns all borrowed token text.
    pub const fn source(&self) -> &'source SourceDocument {
        self.source
    }

    /// All tokens in source order.
    pub fn tokens(&self) -> &[Token<'source>] {
        &self.tokens
    }

    /// Number of tokenized logical lines.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Return a borrowed view of one tokenized logical line.
    pub fn line(&self, index: usize) -> Option<TokenizedLine<'_, 'source>> {
        let record = self.lines.get(index)?;
        Some(TokenizedLine {
            logical_line: self.source.lines().get(record.logical_line_index)?,
            tokens: self.tokens.get(record.token_start..record.token_end)?,
        })
    }

    /// Bytes charged to the tokenizer's configured owned-metadata budget.
    pub const fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }
}

/// Error returned by the bounded SecLang directive tokenizer.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TokenizeError {
    /// The document token count exceeds its configured limit.
    #[error("token count exceeds the configured document limit of {limit} at {position:?}")]
    TokenLimit { limit: usize, position: SourcePosition },
    /// One logical line has too many tokens.
    #[error("logical line {logical_line} exceeds the configured token limit of {limit} at {position:?}")]
    LineTokenLimit {
        logical_line: usize,
        limit: usize,
        position: SourcePosition,
    },
    /// One raw token exceeds its configured byte limit.
    #[error("token has more than {limit} raw bytes at {position:?}")]
    TokenByteLimit { limit: usize, position: SourcePosition },
    /// A quoted token has no closing quote.
    #[error("unterminated {quote_style:?} quoted token opened at {position:?}")]
    UnterminatedQuote {
        quote_style: QuoteStyle,
        position: SourcePosition,
    },
    /// A backslash has no following byte to protect.
    #[error("dangling token escape at {position:?}")]
    DanglingEscape { position: SourcePosition },
    /// A non-separator suffix follows a closing top-level quote.
    #[error("characters follow a closing quote at {position:?}; separate or quote the next token")]
    CharactersAfterClosingQuote { position: SourcePosition },
    /// Logical-to-source position mapping failed.
    #[error("cannot map logical line {logical_line} byte offset {logical_offset} to the original source")]
    PositionMapping { logical_line: usize, logical_offset: usize },
    /// The returned document would exceed its charged owned-byte limit.
    #[error("tokenized source requires more than {limit} charged owned bytes at {position:?}")]
    OwnedByteLimit { limit: usize, position: SourcePosition },
    /// A bounded allocation failed.
    #[error("cannot allocate bounded SecLang {operation}; reduce the source or configured limits")]
    Allocation { operation: &'static str },
}

/// Tokenize all normalized logical lines without copying token text.
pub fn tokenize_document(
    source: &SourceDocument,
    limits: TokenizerLimits,
) -> Result<TokenizedDocument<'_>, TokenizeError> {
    let mut result = TokenizedDocument {
        source,
        tokens: Vec::new(),
        lines: Vec::new(),
        charged_bytes: 0,
    };

    for (line_index, line) in source.lines().iter().enumerate() {
        let line_position = map_start(line, line_index, 0)?;
        charge_owned(
            &mut result.charged_bytes,
            size_of::<TokenizedLineRecord>(),
            limits.max_owned_bytes,
            line_position,
        )?;
        result.lines.try_reserve(1).map_err(|_| TokenizeError::Allocation {
            operation: "tokenized line records",
        })?;

        let token_start = result.tokens.len();
        tokenize_line(line, line_index, limits, &mut result)?;
        result.lines.push(TokenizedLineRecord {
            logical_line_index: line_index,
            token_start,
            token_end: result.tokens.len(),
        });
    }

    Ok(result)
}

fn tokenize_line<'source>(
    line: &'source LogicalLine,
    line_index: usize,
    limits: TokenizerLimits,
    result: &mut TokenizedDocument<'source>,
) -> Result<(), TokenizeError> {
    let text = line.text();
    let bytes = text.as_bytes();
    let initial_count = result.tokens.len();
    let mut cursor = 0;

    while cursor < bytes.len() {
        while cursor < bytes.len() && is_separator(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }

        let raw_start = cursor;
        let token_position = map_start(line, line_index, raw_start)?;
        let line_count = result.tokens.len() - initial_count;
        if line_count >= limits.max_tokens_per_line {
            return Err(TokenizeError::LineTokenLimit {
                logical_line: line_index + 1,
                limit: limits.max_tokens_per_line,
                position: token_position,
            });
        }
        if result.tokens.len() >= limits.max_tokens {
            return Err(TokenizeError::TokenLimit {
                limit: limits.max_tokens,
                position: token_position,
            });
        }
        let (raw_end, content_start, content_end, quote_style) = match bytes[cursor] {
            b'\'' => scan_quoted(line, line_index, bytes, cursor, b'\'', QuoteStyle::Single, limits)?,
            b'"' => scan_quoted(line, line_index, bytes, cursor, b'"', QuoteStyle::Double, limits)?,
            _ => scan_unquoted(line, line_index, bytes, cursor, limits)?,
        };

        charge_owned(
            &mut result.charged_bytes,
            size_of::<Token<'source>>(),
            limits.max_owned_bytes,
            token_position,
        )?;
        result.tokens.try_reserve(1).map_err(|_| TokenizeError::Allocation {
            operation: "token records",
        })?;

        let content_span = if content_start == content_end {
            let point = map_start(line, line_index, content_start)?;
            SourceSpan {
                start: point,
                end: point,
            }
        } else {
            map_span(line, line_index, content_start, content_end)?
        };
        result.tokens.push(Token {
            raw: &text[raw_start..raw_end],
            value: &text[content_start..content_end],
            quote_style,
            raw_span: map_span(line, line_index, raw_start, raw_end)?,
            content_span,
            raw_logical_start: raw_start,
        });
        cursor = raw_end;
    }
    Ok(())
}

fn scan_quoted(
    line: &LogicalLine,
    line_index: usize,
    bytes: &[u8],
    raw_start: usize,
    quote: u8,
    quote_style: QuoteStyle,
    limits: TokenizerLimits,
) -> Result<(usize, usize, usize, QuoteStyle), TokenizeError> {
    check_token_bytes(line, line_index, raw_start, raw_start + 1, limits.max_token_bytes)?;
    let mut cursor = raw_start + 1;
    let mut backslash_run = 0_usize;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor += 1;
        check_token_bytes(line, line_index, raw_start, cursor, limits.max_token_bytes)?;
        if byte == quote && backslash_run.is_multiple_of(2) {
            let content_end = cursor - 1;
            if cursor < bytes.len() && !is_separator(bytes[cursor]) && !is_quote(bytes[cursor]) {
                return Err(TokenizeError::CharactersAfterClosingQuote {
                    position: map_start(line, line_index, cursor)?,
                });
            }
            return Ok((cursor, raw_start + 1, content_end, quote_style));
        }
        if byte == b'\\' {
            backslash_run = backslash_run.saturating_add(1);
        } else {
            backslash_run = 0;
        }
    }

    if backslash_run % 2 == 1 {
        return Err(TokenizeError::DanglingEscape {
            position: map_start(line, line_index, bytes.len() - 1)?,
        });
    }
    Err(TokenizeError::UnterminatedQuote {
        quote_style,
        position: map_start(line, line_index, raw_start)?,
    })
}

fn scan_unquoted(
    line: &LogicalLine,
    line_index: usize,
    bytes: &[u8],
    raw_start: usize,
    limits: TokenizerLimits,
) -> Result<(usize, usize, usize, QuoteStyle), TokenizeError> {
    let mut cursor = raw_start;
    while cursor < bytes.len() && !is_separator(bytes[cursor]) {
        if bytes[cursor] == b'\\' {
            if cursor + 1 == bytes.len() {
                return Err(TokenizeError::DanglingEscape {
                    position: map_start(line, line_index, cursor)?,
                });
            }
            cursor += 2;
        } else {
            cursor += 1;
        }
        check_token_bytes(line, line_index, raw_start, cursor, limits.max_token_bytes)?;
    }
    Ok((cursor, raw_start, cursor, QuoteStyle::Unquoted))
}

fn check_token_bytes(
    line: &LogicalLine,
    line_index: usize,
    raw_start: usize,
    raw_end: usize,
    limit: usize,
) -> Result<(), TokenizeError> {
    if raw_end - raw_start > limit {
        return Err(TokenizeError::TokenByteLimit {
            limit,
            position: map_start(line, line_index, raw_start + limit)?,
        });
    }
    Ok(())
}

fn map_span(line: &LogicalLine, line_index: usize, start: usize, end: usize) -> Result<SourceSpan, TokenizeError> {
    Ok(SourceSpan {
        start: map_start(line, line_index, start)?,
        end: line.position_at_end(end).ok_or(TokenizeError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset: end,
        })?,
    })
}

fn map_start(line: &LogicalLine, line_index: usize, offset: usize) -> Result<SourcePosition, TokenizeError> {
    line.position_at(offset).ok_or(TokenizeError::PositionMapping {
        logical_line: line_index + 1,
        logical_offset: offset,
    })
}

const fn is_separator(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

const fn is_quote(byte: u8) -> bool {
    matches!(byte, b'\'' | b'"')
}

fn charge_owned(
    charged: &mut usize,
    amount: usize,
    limit: usize,
    position: SourcePosition,
) -> Result<(), TokenizeError> {
    let next = charged
        .checked_add(amount)
        .ok_or(TokenizeError::OwnedByteLimit { limit, position })?;
    if next > limit {
        return Err(TokenizeError::OwnedByteLimit { limit, position });
    }
    *charged = next;
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
