//! Expression parsing for EdgionDSL — Pratt / precedence climbing

use nom::IResult;

use super::super::ast::*;
use super::super::error::ParseError;
use super::{lit, ws, ParseCtx};

// ==================== Expression Parsing (Pratt / Precedence Climbing) ====================

pub(super) fn parse_expr<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;
    parse_expr_bp(input, ctx, 0)
}

/// Pratt parser: parse expression with minimum binding power
fn parse_expr_bp<'a>(input: &'a str, ctx: &mut ParseCtx, min_bp: u8) -> IResult<&'a str, Expr, ParseError> {
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
            let rest = ws(&input[1..]);
            let (rest, key) = parse_expr(rest, ctx)?;
            let rest = ws(rest);
            if !rest.starts_with(']') {
                return Err(nom::Err::Failure(ParseError::new("expected ']' after index key")));
            }
            lhs = Expr::new(ExprKind::Index {
                object: Box::new(lhs),
                key: Box::new(key),
            });
            input = &rest[1..];
            continue;
        }

        // Check for binary operator
        if let Some((op, op_len)) = peek_binop(input) {
            let bp = op.precedence();
            if bp <= min_bp {
                break;
            }

            input = ws(&input[op_len..]);

            // Short-circuit for && and ||
            let (rest, rhs) = parse_expr_bp(input, ctx, bp)?;

            ctx.add_node().map_err(nom::Err::Failure)?;
            lhs = Expr::new(ExprKind::BinaryOp {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            });
            input = rest;
            continue;
        }

        break;
    }

    Ok((input, lhs))
}

fn parse_unary_or_atom<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;
    let input = ws(input);

    // Unary !
    if let Some(stripped) = input.strip_prefix('!') {
        ctx.add_node().map_err(nom::Err::Failure)?;
        let (rest, operand) = parse_unary_or_atom(stripped, ctx)?;
        return Ok((
            rest,
            Expr::new(ExprKind::UnaryOp {
                op: UnaryOp::Not,
                operand: Box::new(operand),
            }),
        ));
    }

    // Unary - (negative): supports -42, -x, -(expr), -func()
    if input.starts_with('-') && input.len() > 1 && !input[1..].starts_with(['>', '-']) {
        let next = input.as_bytes()[1];
        // Accept digits, '(', identifiers (alpha/underscore)
        if next.is_ascii_digit() || next == b'(' || next.is_ascii_alphabetic() || next == b'_' {
            ctx.add_node().map_err(nom::Err::Failure)?;
            let (rest, operand) = parse_unary_or_atom(&input[1..], ctx)?;
            return Ok((
                rest,
                Expr::new(ExprKind::UnaryOp {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                }),
            ));
        }
    }

    // Parenthesized expression
    if let Some(stripped) = input.strip_prefix('(') {
        ctx.enter_nesting().map_err(nom::Err::Failure)?;
        let (rest, expr) = parse_expr(stripped, ctx)?;
        let rest = ws(rest);
        if !rest.starts_with(')') {
            ctx.leave_nesting();
            return Err(nom::Err::Failure(ParseError::new("expected ')'")));
        }
        ctx.leave_nesting();
        return Ok((&rest[1..], expr));
    }

    parse_atom(input, ctx)
}

fn parse_atom<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;
    ctx.add_node().map_err(nom::Err::Failure)?;
    let input = ws(input);

    // String literal
    if input.starts_with('"') {
        return lit::parse_string_literal(input, ctx);
    }

    // Integer literal
    if input.starts_with(|c: char| c.is_ascii_digit()) {
        return lit::parse_int_literal(input);
    }

    // Keywords and identifiers
    if input.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        let (rest, ident) =
            lit::parse_identifier(input).map_err(|_| nom::Err::Failure(ParseError::new("expected identifier")))?;

        return match ident {
            "true" => Ok((rest, Expr::bool_val(true))),
            "false" => Ok((rest, Expr::bool_val(false))),
            "nil" => Ok((rest, Expr::nil())),
            _ => {
                // Check for function call: ident(...)
                let rest_ws = ws(rest);
                if let Some(stripped) = rest_ws.strip_prefix('(') {
                    let (rest, args) = parse_call_args(stripped, ctx)?;
                    Ok((
                        rest,
                        Expr::new(ExprKind::FnCall {
                            name: ident.to_string(),
                            args,
                        }),
                    ))
                } else {
                    Ok((rest, Expr::ident(ident)))
                }
            }
        };
    }

    // Map literal: {"key": expr, ...}
    if input.starts_with('{') {
        ctx.enter_nesting().map_err(nom::Err::Failure)?;
        let mut input = ws(&input[1..]);
        let mut pairs: Vec<(Expr, Expr)> = Vec::new();

        if input.starts_with('}') {
            ctx.leave_nesting();
            ctx.add_node().map_err(nom::Err::Failure)?;
            return Ok((&input[1..], Expr::new(ExprKind::MapLiteral(pairs))));
        }

        loop {
            ctx.tick().map_err(nom::Err::Failure)?;
            if !input.starts_with('"') {
                ctx.leave_nesting();
                return Err(nom::Err::Failure(ParseError::new("map key must be a string literal")));
            }
            let (rest, key_expr) = lit::parse_string_literal(input, ctx)?;
            let rest = ws(rest);
            if !rest.starts_with(':') {
                ctx.leave_nesting();
                return Err(nom::Err::Failure(ParseError::new("expected ':' after map key")));
            }
            let rest = ws(&rest[1..]);
            let (rest, val_expr) = parse_expr(rest, ctx)?;
            pairs.push((key_expr, val_expr));
            let rest = ws(rest);
            if rest.starts_with('}') {
                ctx.leave_nesting();
                input = &rest[1..];
                break;
            }
            if rest.starts_with(',') {
                input = ws(&rest[1..]);
            } else {
                ctx.leave_nesting();
                return Err(nom::Err::Failure(ParseError::new("expected ',' or '}' in map literal")));
            }
        }
        ctx.add_node().map_err(nom::Err::Failure)?;
        return Ok((input, Expr::new(ExprKind::MapLiteral(pairs))));
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

fn parse_dot_access<'a>(input: &'a str, object: Expr, ctx: &mut ParseCtx) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;
    ctx.add_node().map_err(nom::Err::Failure)?;

    let input = &input[1..]; // skip '.'
    let (input, method) = lit::parse_identifier(input)
        .map_err(|_| nom::Err::Failure(ParseError::new("expected method/field name after '.'")))?;

    let input_ws = ws(input);
    if let Some(stripped) = input_ws.strip_prefix('(') {
        // Method call: obj.method(args...)
        let (rest, args) = parse_call_args(stripped, ctx)?;
        Ok((
            rest,
            Expr::new(ExprKind::MethodCall {
                object: Box::new(object),
                method: method.to_string(),
                args,
            }),
        ))
    } else {
        // Field access: obj.field
        Ok((
            input,
            Expr::new(ExprKind::FieldAccess {
                object: Box::new(object),
                field: method.to_string(),
            }),
        ))
    }
}

/// Parse comma-separated arguments inside parentheses. Input starts AFTER '('
fn parse_call_args<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Vec<Expr>, ParseError> {
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
mod tests {
    use super::super::super::ast::*;
    use super::super::parse_program;

    #[test]
    fn test_parse_string_literal() {
        let prog = parse_program(r#"let s = "hello world""#).unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::StringLit(s) => assert_eq!(s, "hello world"),
                _ => panic!("expected StringLit"),
            },
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_method_call() {
        let prog = parse_program(r#"let ip = req.header("X-Real-IP")"#).unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::MethodCall { object, method, args } => {
                    assert!(matches!(object.kind, ExprKind::Ident(ref s) if s == "req"));
                    assert_eq!(method, "header");
                    assert_eq!(args.len(), 1);
                }
                _ => panic!("expected MethodCall"),
            },
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_binary_ops() {
        let prog = parse_program("let x = 1 + 2 * 3").unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { value, .. } => {
                // Should be 1 + (2 * 3) due to precedence
                assert!(matches!(value.kind, ExprKind::BinaryOp { op: BinOp::Add, .. }));
            }
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_logical_ops() {
        let prog = parse_program("let x = a && b || c").unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { value, .. } => {
                // Should be (a && b) || c due to precedence
                assert!(matches!(value.kind, ExprKind::BinaryOp { op: BinOp::Or, .. }));
            }
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_unary_not() {
        let prog = parse_program("let x = !true").unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { value, .. } => {
                assert!(matches!(value.kind, ExprKind::UnaryOp { op: UnaryOp::Not, .. }));
            }
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_fn_call() {
        let prog = parse_program(r#"log("hello")"#).unwrap();
        match &prog.stmts[0].kind {
            StmtKind::ExprStmt { expr } => match &expr.kind {
                ExprKind::FnCall { name, args } => {
                    assert_eq!(name, "log");
                    assert_eq!(args.len(), 1);
                }
                _ => panic!("expected FnCall"),
            },
            _ => panic!("expected ExprStmt"),
        }
    }

    #[test]
    fn test_parse_map_literal() {
        let prog = parse_program(r#"let m = {"a": "1", "b": "2"}"#).unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::MapLiteral(pairs) => assert_eq!(pairs.len(), 2),
                _ => panic!("expected MapLiteral"),
            },
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_empty_map() {
        let prog = parse_program(r#"let m = {}"#).unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::MapLiteral(pairs) => assert_eq!(pairs.len(), 0),
                _ => panic!("expected MapLiteral"),
            },
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_index_access() {
        let prog = parse_program(r#"let v = m["key"]"#).unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::Index { .. } => {}
                _ => panic!("expected Index"),
            },
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_chained_index() {
        let prog = parse_program(r#"let v = resp["headers"]["x-token"]"#).unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { value, .. } => match &value.kind {
                ExprKind::Index { object, .. } => match &object.kind {
                    ExprKind::Index { .. } => {}
                    _ => panic!("expected nested Index"),
                },
                _ => panic!("expected Index"),
            },
            _ => panic!("expected Let"),
        }
    }
}
