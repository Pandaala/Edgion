//! Expression parsing for EdgionDSL — Pratt / precedence climbing

use nom::IResult;

use super::super::ast::*;
use super::super::error::ParseError;
use super::{lit, ws, ParseCtx};

fn with_nesting<'a, T>(
    ctx: &mut ParseCtx<'a>,
    parse: impl FnOnce(&mut ParseCtx<'a>) -> IResult<&'a str, T, ParseError>,
) -> IResult<&'a str, T, ParseError> {
    if let Err(error) = ctx.enter_nesting() {
        ctx.leave_nesting();
        return Err(nom::Err::Failure(error));
    }
    let result = parse(ctx);
    ctx.leave_nesting();
    result
}

// ==================== Expression Parsing (Pratt / Precedence Climbing) ====================

pub(super) fn parse_expr<'a>(input: &'a str, ctx: &mut ParseCtx<'a>) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;
    parse_expr_bp(input, ctx, 0)
}

/// Pratt parser: parse expression with minimum binding power
fn parse_expr_bp<'a>(input: &'a str, ctx: &mut ParseCtx<'a>, min_bp: u8) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;

    // Parse prefix / atom
    let (mut input, mut lhs) = parse_unary_or_atom(input, ctx)?;

    loop {
        input = ws(input);

        // Check for method call / field access: `.`
        if input.starts_with('.') {
            let (rest, new_lhs) = parse_dot_access(input, lhs, ctx)?;
            lhs = new_lhs;
            input = rest;
            continue;
        }

        // Index access: container[key]
        if input.starts_with('[') {
            ctx.tick().map_err(nom::Err::Failure)?;
            ctx.add_node().map_err(nom::Err::Failure)?;
            let span = ctx.current_span(input);
            let rest = ws(&input[1..]);
            let (rest, key) = with_nesting(ctx, |ctx| {
                let (rest, key) = parse_expr(rest, ctx)?;
                let rest = ws(rest);
                if !rest.starts_with(']') {
                    return Err(nom::Err::Failure(ParseError::new("expected ']' after index key")));
                }
                Ok((&rest[1..], key))
            })?;
            lhs = Expr::with_span(
                ExprKind::Index {
                    object: Box::new(lhs),
                    key: Box::new(key),
                },
                span,
            );
            input = rest;
            continue;
        }

        // Check for binary operator
        if let Some((op, op_len)) = peek_binop(input) {
            let bp = op.precedence();
            if bp <= min_bp {
                break;
            }

            let span = ctx.current_span(input); // span at the operator token
            input = ws(&input[op_len..]);

            // Short-circuit for && and ||
            let (rest, rhs) = parse_expr_bp(input, ctx, bp)?;

            ctx.add_node().map_err(nom::Err::Failure)?;
            lhs = Expr::with_span(
                ExprKind::BinaryOp {
                    op,
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                },
                span,
            );
            input = rest;
            continue;
        }

        break;
    }

    Ok((input, lhs))
}

fn parse_unary_or_atom<'a>(input: &'a str, ctx: &mut ParseCtx<'a>) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;
    let input = ws(input);

    // Unary !
    if let Some(stripped) = input.strip_prefix('!') {
        let span = ctx.current_span(input);
        ctx.add_node().map_err(nom::Err::Failure)?;
        let (rest, operand) = with_nesting(ctx, |ctx| parse_unary_or_atom(stripped, ctx))?;
        return Ok((
            rest,
            Expr::with_span(
                ExprKind::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                },
                span,
            ),
        ));
    }

    // Unary - (negative): supports -42, -x, -(expr), -func()
    if input.starts_with('-') && input.len() > 1 && !input[1..].starts_with(['>', '-']) {
        let next = input.as_bytes()[1];
        // Accept digits, '(', identifiers (alpha/underscore)
        if next.is_ascii_digit() || next == b'(' || next.is_ascii_alphabetic() || next == b'_' {
            let span = ctx.current_span(input);
            ctx.add_node().map_err(nom::Err::Failure)?;
            let (rest, operand) = with_nesting(ctx, |ctx| parse_unary_or_atom(&input[1..], ctx))?;
            return Ok((
                rest,
                Expr::with_span(
                    ExprKind::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                ),
            ));
        }
    }

    // Parenthesized expression
    if let Some(stripped) = input.strip_prefix('(') {
        return with_nesting(ctx, |ctx| {
            let (rest, expr) = parse_expr(stripped, ctx)?;
            let rest = ws(rest);
            if !rest.starts_with(')') {
                return Err(nom::Err::Failure(ParseError::new("expected ')'")));
            }
            Ok((&rest[1..], expr))
        });
    }

    parse_atom(input, ctx)
}

fn parse_atom<'a>(input: &'a str, ctx: &mut ParseCtx<'a>) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;
    ctx.add_node().map_err(nom::Err::Failure)?;
    let input = ws(input);
    let span = ctx.current_span(input);

    // String literal
    if input.starts_with('"') {
        let (rest, mut e) = lit::parse_string_literal(input, ctx)?;
        e.span = Some(span);
        return Ok((rest, e));
    }

    // Integer literal
    if input.starts_with(|c: char| c.is_ascii_digit()) {
        let (rest, mut e) = lit::parse_int_literal(input)?;
        e.span = Some(span);
        return Ok((rest, e));
    }

    // Keywords and identifiers
    if input.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        let (rest, ident) =
            lit::parse_identifier(input).map_err(|_| nom::Err::Failure(ParseError::new("expected identifier")))?;

        return match ident {
            "true" => Ok((rest, Expr::with_span(ExprKind::BoolLit(true), span))),
            "false" => Ok((rest, Expr::with_span(ExprKind::BoolLit(false), span))),
            "nil" => Ok((rest, Expr::with_span(ExprKind::NilLit, span))),
            _ => {
                // Check for function call: ident(...)
                let rest_ws = ws(rest);
                if let Some(stripped) = rest_ws.strip_prefix('(') {
                    let (rest, args) = parse_call_args(stripped, ctx)?;
                    Ok((
                        rest,
                        Expr::with_span(
                            ExprKind::FnCall {
                                name: ident.to_string(),
                                args,
                            },
                            span,
                        ),
                    ))
                } else {
                    Ok((rest, Expr::with_span(ExprKind::Ident(ident.to_string()), span)))
                }
            }
        };
    }

    // Map literal: {"key": expr, ...}
    if let Some(stripped) = input.strip_prefix('{') {
        let (input, pairs) = with_nesting(ctx, |ctx| {
            let mut input = ws(stripped);
            let mut pairs: Vec<(Expr, Expr)> = Vec::new();

            if let Some(stripped) = input.strip_prefix('}') {
                return Ok((stripped, pairs));
            }

            loop {
                ctx.tick().map_err(nom::Err::Failure)?;
                if !input.starts_with('"') {
                    return Err(nom::Err::Failure(ParseError::new("map key must be a string literal")));
                }
                ctx.add_node().map_err(nom::Err::Failure)?;
                let key_span = ctx.current_span(input);
                let (rest, mut key_expr) = lit::parse_string_literal(input, ctx)?;
                key_expr.span = Some(key_span);
                let rest = ws(rest);
                if !rest.starts_with(':') {
                    return Err(nom::Err::Failure(ParseError::new("expected ':' after map key")));
                }
                let rest = ws(&rest[1..]);
                let (rest, val_expr) = parse_expr(rest, ctx)?;
                pairs.push((key_expr, val_expr));
                let rest = ws(rest);
                if let Some(stripped) = rest.strip_prefix('}') {
                    return Ok((stripped, pairs));
                }
                if let Some(stripped) = rest.strip_prefix(',') {
                    input = ws(stripped);
                } else {
                    return Err(nom::Err::Failure(ParseError::new("expected ',' or '}' in map literal")));
                }
            }
        })?;
        ctx.add_node().map_err(nom::Err::Failure)?;
        return Ok((input, Expr::with_span(ExprKind::MapLiteral(pairs), span)));
    }

    let end = (0..=input.len().min(20))
        .rev()
        .find(|&i| input.is_char_boundary(i))
        .unwrap_or(0);
    Err(nom::Err::Failure(ParseError::new(format!(
        "unexpected token: '{}'",
        &input[..end]
    ))))
}

// ==================== Dot Access (Method Call / Field Access) ====================

fn parse_dot_access<'a>(input: &'a str, object: Expr, ctx: &mut ParseCtx<'a>) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;
    ctx.add_node().map_err(nom::Err::Failure)?;
    let span = ctx.current_span(input); // span at '.'

    let input = &input[1..]; // skip '.'
    let (input, method) = lit::parse_identifier(input)
        .map_err(|_| nom::Err::Failure(ParseError::new("expected method/field name after '.'")))?;

    let input_ws = ws(input);
    if let Some(stripped) = input_ws.strip_prefix('(') {
        // Method call: obj.method(args...)
        let (rest, args) = parse_call_args(stripped, ctx)?;
        Ok((
            rest,
            Expr::with_span(
                ExprKind::MethodCall {
                    object: Box::new(object),
                    method: method.to_string(),
                    args,
                },
                span,
            ),
        ))
    } else {
        // Field access: obj.field
        Ok((
            input,
            Expr::with_span(
                ExprKind::FieldAccess {
                    object: Box::new(object),
                    field: method.to_string(),
                },
                span,
            ),
        ))
    }
}

/// Parse comma-separated arguments inside parentheses. Input starts AFTER '('
fn parse_call_args<'a>(input: &'a str, ctx: &mut ParseCtx<'a>) -> IResult<&'a str, Vec<Expr>, ParseError> {
    with_nesting(ctx, |ctx| parse_call_args_inner(input, ctx))
}

fn parse_call_args_inner<'a>(input: &'a str, ctx: &mut ParseCtx<'a>) -> IResult<&'a str, Vec<Expr>, ParseError> {
    let mut args = Vec::new();
    let mut input = ws(input);

    if let Some(stripped) = input.strip_prefix(')') {
        return Ok((stripped, args));
    }

    loop {
        ctx.tick().map_err(nom::Err::Failure)?;
        let (rest, arg) = parse_expr(input, ctx)?;
        args.push(arg);
        let rest = ws(rest);
        if let Some(stripped) = rest.strip_prefix(')') {
            return Ok((stripped, args));
        }
        if let Some(stripped) = rest.strip_prefix(',') {
            input = ws(stripped);
        } else {
            return Err(nom::Err::Failure(ParseError::new(
                "expected ',' or ')' in argument list",
            )));
        }
    }
}

// ==================== Binary Operator Peeking ====================

/// Peek at the next binary operator without consuming input.
/// Returns (BinOp, length_in_chars) or None.
fn peek_binop(input: &str) -> Option<(BinOp, usize)> {
    // Two-character operators first
    if input.starts_with("==") {
        return Some((BinOp::Eq, 2));
    }
    if input.starts_with("!=") {
        return Some((BinOp::Ne, 2));
    }
    if input.starts_with("<=") {
        return Some((BinOp::Le, 2));
    }
    if input.starts_with(">=") {
        return Some((BinOp::Ge, 2));
    }
    if input.starts_with("&&") {
        return Some((BinOp::And, 2));
    }
    if input.starts_with("||") {
        return Some((BinOp::Or, 2));
    }

    // Single-character operators
    match input.chars().next() {
        Some('+') => Some((BinOp::Add, 1)),
        Some('-') => Some((BinOp::Sub, 1)),
        Some('*') => Some((BinOp::Mul, 1)),
        Some('/') if !input.starts_with("//") => Some((BinOp::Div, 1)),
        Some('<') => Some((BinOp::Lt, 1)),
        Some('>') => Some((BinOp::Gt, 1)),
        _ => None,
    }
}

// ==================== Tests ====================

#[cfg(test)]
#[path = "../../tests/unit/parser/expr_test.rs"]
mod tests;
