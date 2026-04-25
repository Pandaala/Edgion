//! Statement parsing for EdgionDSL

use nom::IResult;

use super::super::ast::*;
use super::super::error::ParseError;
use super::{expr, is_reserved_keyword, lit, starts_with_keyword, ws, ParseCtx};

// ==================== Statement Parsing ====================

pub(super) fn parse_stmt<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Stmt, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;

    let input = ws(input);

    if starts_with_keyword(input, "let") {
        parse_let_stmt(input, ctx)
    } else if starts_with_keyword(input, "if") || input.starts_with("if(") {
        parse_if_stmt(input, ctx)
    } else if starts_with_keyword(input, "for") {
        parse_for_stmt(input, ctx)
    } else if starts_with_keyword(input, "while") || input.starts_with("while(") {
        parse_while_stmt(input, ctx)
    } else if starts_with_keyword(input, "break") {
        ctx.add_node().map_err(nom::Err::Failure)?;
        Ok((&input[5..], Stmt::new(StmtKind::Break)))
    } else if starts_with_keyword(input, "continue") {
        ctx.add_node().map_err(nom::Err::Failure)?;
        Ok((&input[8..], Stmt::new(StmtKind::Continue)))
    } else if starts_with_keyword(input, "return") {
        parse_return_stmt(input, ctx)
    } else {
        // Try assignment or expression statement
        parse_assign_or_expr_stmt(input, ctx)
    }
}

fn parse_let_stmt<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Stmt, ParseError> {
    ctx.add_node().map_err(nom::Err::Failure)?;

    let input = ws(&input[3..]); // skip "let" + whitespace

    // Check for "mut"
    let (input, mutable) = if starts_with_keyword(input, "mut") {
        (ws(&input[3..]), true)
    } else {
        (input, false)
    };
    let input = ws(input);

    // Parse variable name
    let (input, name) = lit::parse_identifier(input)
        .map_err(|_| nom::Err::Failure(ParseError::new("expected variable name after 'let'")))?;
    if is_reserved_keyword(name) {
        return Err(nom::Err::Failure(ParseError::new(format!(
            "'{}' is a reserved keyword and cannot be used as a variable name",
            name
        ))));
    }
    let input = ws(input);

    // Expect '=' but not '=='
    if !input.starts_with('=') || input.starts_with("==") {
        return Err(nom::Err::Failure(ParseError::new(
            "expected '=' after variable name in let statement",
        )));
    }
    let input = ws(&input[1..]);

    // Parse value expression
    let (input, value) = expr::parse_expr(input, ctx)?;

    Ok((
        input,
        Stmt::new(StmtKind::Let {
            name: name.to_string(),
            mutable,
            value,
        }),
    ))
}

fn parse_if_stmt<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Stmt, ParseError> {
    ctx.add_node().map_err(nom::Err::Failure)?;
    ctx.enter_nesting().map_err(nom::Err::Failure)?;

    let mut branches = Vec::new();
    let mut current_input = input;

    // Parse first "if condition { body }"
    current_input = &current_input[2..]; // skip "if"
    let current_input_trimmed = ws(current_input);

    let (rest, condition) = expr::parse_expr(current_input_trimmed, ctx)?;
    let rest = ws(rest);

    if !rest.starts_with('{') {
        ctx.leave_nesting();
        return Err(nom::Err::Failure(ParseError::new("expected '{' after if condition")));
    }
    let (rest, body) = parse_block(&rest[1..], ctx)?;
    branches.push((condition, body));
    current_input = ws(rest);

    // Parse "else if" and "else" chains
    let mut else_body = None;
    loop {
        if starts_with_keyword(current_input, "else") && {
            let after_else = ws(&current_input[4..]);
            starts_with_keyword(after_else, "if") || after_else.starts_with("if(")
        } {
            current_input = ws(&current_input[4..]); // skip "else", then ws
            current_input = &current_input[2..]; // skip "if"
            let trimmed = ws(current_input);

            let (rest, condition) = expr::parse_expr(trimmed, ctx)?;
            let rest = ws(rest);
            if !rest.starts_with('{') {
                ctx.leave_nesting();
                return Err(nom::Err::Failure(ParseError::new(
                    "expected '{' after else if condition",
                )));
            }
            let (rest, body) = parse_block(&rest[1..], ctx)?;
            branches.push((condition, body));
            current_input = ws(rest);
        } else if (starts_with_keyword(current_input, "else") || current_input.starts_with("else{"))
            && current_input[4..].trim_start().starts_with('{')
        {
            current_input = current_input[4..].trim_start();
            let (rest, body) = parse_block(&current_input[1..], ctx)?;
            else_body = Some(body);
            current_input = rest;
            break;
        } else {
            break;
        }
    }

    ctx.leave_nesting();
    Ok((current_input, Stmt::new(StmtKind::If { branches, else_body })))
}

fn parse_for_stmt<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Stmt, ParseError> {
    ctx.add_node().map_err(nom::Err::Failure)?;
    ctx.enter_nesting().map_err(nom::Err::Failure)?;

    let input = ws(&input[3..]); // skip "for" + whitespace

    // Parse loop variable name
    let (input, var_name) = lit::parse_identifier(input)
        .map_err(|_| nom::Err::Failure(ParseError::new("expected variable name after 'for'")))?;
    if is_reserved_keyword(var_name) {
        ctx.leave_nesting();
        return Err(nom::Err::Failure(ParseError::new(format!(
            "'{}' is a reserved keyword and cannot be used as a loop variable",
            var_name
        ))));
    }
    let input = ws(input);

    // Expect "in"
    if !starts_with_keyword(input, "in") {
        ctx.leave_nesting();
        return Err(nom::Err::Failure(ParseError::new("expected 'in' after for variable")));
    }
    let input = ws(&input[2..]);

    // Check for "range(start, end)"
    if input.starts_with("range(") || input.starts_with("range (") {
        let input = input
            .strip_prefix("range(")
            .or_else(|| input.strip_prefix("range ("))
            .expect("guard ensures one of these matches");
        let input = ws(input);
        let (input, start) = expr::parse_expr(input, ctx)?;
        let input = ws(input);
        if !input.starts_with(',') {
            ctx.leave_nesting();
            return Err(nom::Err::Failure(ParseError::new("expected ',' in range(start, end)")));
        }
        let input = ws(&input[1..]);
        let (input, end) = expr::parse_expr(input, ctx)?;
        let input = ws(input);
        if !input.starts_with(')') {
            ctx.leave_nesting();
            return Err(nom::Err::Failure(ParseError::new("expected ')' after range arguments")));
        }
        let input = ws(&input[1..]);
        if !input.starts_with('{') {
            ctx.leave_nesting();
            return Err(nom::Err::Failure(ParseError::new("expected '{' after for range")));
        }
        let (input, body) = parse_block(&input[1..], ctx)?;
        ctx.leave_nesting();
        return Ok((
            input,
            Stmt::new(StmtKind::ForRange {
                var_name: var_name.to_string(),
                start,
                end,
                body,
            }),
        ));
    }

    // for-in: parse iterable expression
    let (input, iterable) = expr::parse_expr(input, ctx)?;
    let input = ws(input);
    if !input.starts_with('{') {
        ctx.leave_nesting();
        return Err(nom::Err::Failure(ParseError::new("expected '{' after for-in iterable")));
    }
    let (input, body) = parse_block(&input[1..], ctx)?;
    ctx.leave_nesting();
    Ok((
        input,
        Stmt::new(StmtKind::ForIn {
            var_name: var_name.to_string(),
            iterable,
            body,
        }),
    ))
}

fn parse_while_stmt<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Stmt, ParseError> {
    ctx.add_node().map_err(nom::Err::Failure)?;
    ctx.enter_nesting().map_err(nom::Err::Failure)?;

    let input = ws(&input[5..]); // skip "while" + whitespace

    let (input, condition) = expr::parse_expr(input, ctx)?;
    let input = ws(input);
    if !input.starts_with('{') {
        ctx.leave_nesting();
        return Err(nom::Err::Failure(ParseError::new("expected '{' after while condition")));
    }
    let (input, body) = parse_block(&input[1..], ctx)?;
    ctx.leave_nesting();
    Ok((input, Stmt::new(StmtKind::While { condition, body })))
}

fn parse_return_stmt<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Stmt, ParseError> {
    ctx.add_node().map_err(nom::Err::Failure)?;

    let input = ws(&input[6..]); // skip "return" + whitespace

    if input.starts_with("next()") || input.starts_with("next ()") {
        let skip = if input.starts_with("next()") { 6 } else { 7 };
        return Ok((&input[skip..], Stmt::new(StmtKind::ReturnNext)));
    }

    if input.starts_with("terminate()") || input.starts_with("terminate ()") {
        let skip = if input.starts_with("terminate()") { 11 } else { 12 };
        return Ok((&input[skip..], Stmt::new(StmtKind::ReturnTerminate)));
    }

    if input.starts_with("grpc_error(") || input.starts_with("grpc_error (") {
        let input = input
            .strip_prefix("grpc_error(")
            .or_else(|| input.strip_prefix("grpc_error ("))
            .expect("guard ensures one of these matches");
        let input = ws(input);
        let (input, code) = expr::parse_expr(input, ctx)?;
        let input = ws(input);
        if !input.starts_with(',') {
            return Err(nom::Err::Failure(ParseError::new(
                "expected ',' in grpc_error(code, message)",
            )));
        }
        let input = ws(&input[1..]);
        let (input, message) = expr::parse_expr(input, ctx)?;
        let input = ws(input);
        if !input.starts_with(')') {
            return Err(nom::Err::Failure(ParseError::new(
                "expected ')' after grpc_error arguments",
            )));
        }
        return Ok((&input[1..], Stmt::new(StmtKind::ReturnGrpcError { code, message })));
    }

    if input.starts_with("deny(") || input.starts_with("deny (") {
        let input = input
            .strip_prefix("deny(")
            .or_else(|| input.strip_prefix("deny ("))
            .expect("guard ensures one of these matches");
        let input = ws(input);
        let (input, status) = expr::parse_expr(input, ctx)?;
        let input = ws(input);
        if !input.starts_with(',') {
            return Err(nom::Err::Failure(ParseError::new("expected ',' in deny(status, body)")));
        }
        let input = ws(&input[1..]);
        let (input, body) = expr::parse_expr(input, ctx)?;
        let input = ws(input);
        if !input.starts_with(')') {
            return Err(nom::Err::Failure(ParseError::new("expected ')' after deny arguments")));
        }
        return Ok((&input[1..], Stmt::new(StmtKind::ReturnDeny { status, body })));
    }

    Err(nom::Err::Failure(ParseError::new(
        "expected 'next()', 'deny(status, body)', 'terminate()', or 'grpc_error(code, message)' after 'return'",
    )))
}

fn parse_assign_or_expr_stmt<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Stmt, ParseError> {
    ctx.add_node().map_err(nom::Err::Failure)?;

    // Try to parse an expression first
    let (rest, expr) = expr::parse_expr(input, ctx)?;

    // Check if this is an assignment: identifier followed by '='
    let rest_ws = ws(rest);
    if rest_ws.starts_with('=') && !rest_ws.starts_with("==") {
        if let ExprKind::Ident(name) = &expr.kind {
            let rest_ws = ws(&rest_ws[1..]);
            let (rest_ws, value) = expr::parse_expr(rest_ws, ctx)?;
            return Ok((
                rest_ws,
                Stmt::new(StmtKind::Assign {
                    name: name.clone(),
                    value,
                }),
            ));
        }
        // Non-identifier left-hand side in assignment
        return Err(nom::Err::Failure(ParseError::new(
            "cannot assign to this expression; only simple variable names can be assigned",
        )));
    }

    Ok((rest, Stmt::new(StmtKind::ExprStmt { expr })))
}

// ==================== Block Parsing ====================

/// Parse statements until '}'. Input should start AFTER the opening '{'
pub(super) fn parse_block<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Vec<Stmt>, ParseError> {
    let mut stmts = Vec::new();
    let mut input = ws(input);

    while !input.is_empty() && !input.starts_with('}') {
        ctx.tick().map_err(nom::Err::Failure)?;
        let (rest, stmt) = parse_stmt(input, ctx)?;
        stmts.push(stmt);
        input = ws(rest);
    }

    if !input.starts_with('}') {
        return Err(nom::Err::Failure(ParseError::new("expected '}'")));
    }

    Ok((&input[1..], stmts))
}

// ==================== Tests ====================

#[cfg(test)]
mod tests {
    use super::super::super::ast::*;
    use super::super::parse_program;

    #[test]
    fn test_parse_let_simple() {
        let prog = parse_program(r#"let x = 42"#).unwrap();
        assert_eq!(prog.stmts.len(), 1);
        match &prog.stmts[0].kind {
            StmtKind::Let { name, mutable, value } => {
                assert_eq!(name, "x");
                assert!(!mutable);
                assert!(matches!(value.kind, ExprKind::IntLit(42)));
            }
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_let_mut() {
        let prog = parse_program(r#"let mut count = 0"#).unwrap();
        match &prog.stmts[0].kind {
            StmtKind::Let { mutable, .. } => assert!(mutable),
            _ => panic!("expected Let"),
        }
    }

    #[test]
    fn test_parse_if_simple() {
        let prog = parse_program(
            r#"
            if x == nil {
                return deny(403, "blocked")
            }
        "#,
        )
        .unwrap();
        assert_eq!(prog.stmts.len(), 1);
        match &prog.stmts[0].kind {
            StmtKind::If { branches, else_body } => {
                assert_eq!(branches.len(), 1);
                assert!(else_body.is_none());
            }
            _ => panic!("expected If"),
        }
    }

    #[test]
    fn test_parse_if_else() {
        let prog = parse_program(
            r#"
            if x == 1 {
                let a = 1
            } else {
                let b = 2
            }
        "#,
        )
        .unwrap();
        match &prog.stmts[0].kind {
            StmtKind::If { branches, else_body } => {
                assert_eq!(branches.len(), 1);
                assert!(else_body.is_some());
            }
            _ => panic!("expected If"),
        }
    }

    #[test]
    fn test_parse_for_range() {
        let prog = parse_program(
            r#"
            for i in range(0, 5) {
                ctx.set("key", "value")
            }
        "#,
        )
        .unwrap();
        match &prog.stmts[0].kind {
            StmtKind::ForRange {
                var_name,
                start,
                end,
                body,
            } => {
                assert_eq!(var_name, "i");
                assert!(matches!(start.kind, ExprKind::IntLit(0)));
                assert!(matches!(end.kind, ExprKind::IntLit(5)));
                assert_eq!(body.len(), 1);
            }
            _ => panic!("expected ForRange"),
        }
    }

    #[test]
    fn test_parse_while() {
        let prog = parse_program(
            r#"
            while x < 10 {
                x = x + 1
            }
        "#,
        )
        .unwrap();
        assert_eq!(prog.stmts.len(), 1);
        assert!(matches!(prog.stmts[0].kind, StmtKind::While { .. }));
    }

    #[test]
    fn test_parse_return_next() {
        let prog = parse_program("return next()").unwrap();
        assert!(matches!(prog.stmts[0].kind, StmtKind::ReturnNext));
    }

    #[test]
    fn test_parse_return_deny() {
        let prog = parse_program(r#"return deny(403, "forbidden")"#).unwrap();
        match &prog.stmts[0].kind {
            StmtKind::ReturnDeny { status, body } => {
                assert!(matches!(status.kind, ExprKind::IntLit(403)));
                assert!(matches!(body.kind, ExprKind::StringLit(ref s) if s == "forbidden"));
            }
            _ => panic!("expected ReturnDeny"),
        }
    }

    #[test]
    fn test_parse_complex_script() {
        let prog = parse_program(
            r#"
            let ip = req.header("X-Real-IP")
            if ip == nil {
                return deny(403, "Missing IP")
            } else if ip.starts_with("10.") {
                log("Internal request from " + ip)
            } else {
                req.set_header("X-Forwarded-For", ip)
            }
        "#,
        )
        .unwrap();
        assert_eq!(prog.stmts.len(), 2); // let + if
    }

    #[test]
    fn test_parse_for_in() {
        let prog = parse_program(
            r#"
            for name in req.header_names() {
                log(name)
            }
        "#,
        )
        .unwrap();
        match &prog.stmts[0].kind {
            StmtKind::ForIn {
                var_name,
                iterable: _,
                body,
            } => {
                assert_eq!(var_name, "name");
                assert_eq!(body.len(), 1);
            }
            _ => panic!("expected ForIn"),
        }
    }

    #[test]
    fn test_parse_assignment() {
        let prog = parse_program(
            r#"
            let mut x = 0
            x = x + 1
        "#,
        )
        .unwrap();
        assert_eq!(prog.stmts.len(), 2);
        assert!(matches!(prog.stmts[1].kind, StmtKind::Assign { .. }));
    }
}
