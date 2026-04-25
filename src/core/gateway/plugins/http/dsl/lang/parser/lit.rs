//! Literal parsing for EdgionDSL — strings, ints, identifiers

use nom::IResult;

use super::super::ast::*;
use super::super::error::ParseError;
use super::ParseCtx;

// ==================== Literals ====================

pub(super) fn parse_string_literal<'a>(input: &'a str, _ctx: &mut ParseCtx) -> IResult<&'a str, Expr, ParseError> {
    let input = &input[1..]; // skip opening "
    let mut result = String::new();
    let mut chars = input.chars();
    let mut consumed = 0;

    loop {
        match chars.next() {
            Some('"') => {
                consumed += 1;
                return Ok((&input[consumed..], Expr::string(result)));
            }
            Some('\\') => {
                consumed += 1;
                match chars.next() {
                    Some('n') => {
                        result.push('\n');
                        consumed += 1;
                    }
                    Some('t') => {
                        result.push('\t');
                        consumed += 1;
                    }
                    Some('\\') => {
                        result.push('\\');
                        consumed += 1;
                    }
                    Some('"') => {
                        result.push('"');
                        consumed += 1;
                    }
                    Some(c) => {
                        result.push('\\');
                        result.push(c);
                        consumed += c.len_utf8();
                    }
                    None => {
                        return Err(nom::Err::Failure(ParseError::new("unterminated string literal")));
                    }
                }
            }
            Some(c) => {
                result.push(c);
                consumed += c.len_utf8();
            }
            None => {
                return Err(nom::Err::Failure(ParseError::new("unterminated string literal")));
            }
        }
    }
}

pub(super) fn parse_int_literal(input: &str) -> IResult<&str, Expr, ParseError> {
    let end = input.find(|c: char| !c.is_ascii_digit()).unwrap_or(input.len());
    if end == 0 {
        return Err(nom::Err::Failure(ParseError::new("expected integer")));
    }
    let num_str = &input[..end];
    let n: i64 = num_str
        .parse()
        .map_err(|_| nom::Err::Failure(ParseError::new(format!("invalid integer: {}", num_str))))?;
    Ok((&input[end..], Expr::int(n)))
}

// ==================== Identifiers ====================

pub(super) fn parse_identifier(input: &str) -> IResult<&str, &str, ParseError> {
    if input.is_empty() || (!input.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')) {
        return Err(nom::Err::Error(ParseError::new("expected identifier")));
    }
    let end = input
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(input.len());
    Ok((&input[end..], &input[..end]))
}
