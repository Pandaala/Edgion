//! EdgionDSL parser — nom-based parser with input-layer defenses
//!
//! Security:
//!   - Source size limit (Layer 0)
//!   - Parse budget (prevents pathological backtracking)
//!   - Nesting depth limit (prevents stack overflow)
//!   - AST node count limit

use nom::IResult;

use super::ast::*;
use super::error::{ParseError, Span};

/// Maximum source code length (64KB)
const MAX_SOURCE_LEN: usize = 64 * 1024;

// ==================== Parse Context (Safety) ====================

/// Parser safety context — threaded through all parser functions.
struct ParseCtx {
    /// Global parse budget — decremented on every combinator entry
    parse_budget: u32,
    /// Current nesting depth
    nesting_depth: u16,
    /// Max allowed nesting depth
    max_nesting_depth: u16,
    /// Total AST nodes created
    ast_node_count: u32,
    /// Max allowed AST nodes
    max_ast_nodes: u32,
    /// Original source (for span calculation)
    _source_start: usize,
}

impl Default for ParseCtx {
    fn default() -> Self {
        Self {
            parse_budget: 50_000,
            nesting_depth: 0,
            max_nesting_depth: 64,
            ast_node_count: 0,
            max_ast_nodes: 2_000,
            _source_start: 0,
        }
    }
}

impl ParseCtx {
    fn tick(&mut self) -> Result<(), ParseError> {
        if self.parse_budget == 0 {
            return Err(ParseError::new("parse budget exceeded"));
        }
        self.parse_budget -= 1;
        Ok(())
    }

    fn enter_nesting(&mut self) -> Result<(), ParseError> {
        self.nesting_depth += 1;
        if self.nesting_depth > self.max_nesting_depth {
            return Err(ParseError::new(format!(
                "nesting too deep: {} (max {})",
                self.nesting_depth, self.max_nesting_depth
            )));
        }
        Ok(())
    }

    fn leave_nesting(&mut self) {
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
    }

    fn add_node(&mut self) -> Result<(), ParseError> {
        self.ast_node_count += 1;
        if self.ast_node_count > self.max_ast_nodes {
            return Err(ParseError::new(format!(
                "too many AST nodes: {} (max {})",
                self.ast_node_count, self.max_ast_nodes
            )));
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn span_at(&self, input: &str) -> Span {
        let offset = input.as_ptr() as usize - self._source_start;
        // Simple line/col calculation would need source; use offset for now
        Span::new(offset, 1, offset as u32 + 1)
    }
}

// ==================== Public API ====================

/// Parse a complete DSL program from source code
pub fn parse_program(source: &str) -> Result<Program, ParseError> {
    // Layer 0: source size limit
    if source.len() > MAX_SOURCE_LEN {
        return Err(ParseError::new(format!(
            "source too large: {} bytes (max {})",
            source.len(),
            MAX_SOURCE_LEN
        )));
    }

    let mut ctx = ParseCtx {
        _source_start: source.as_ptr() as usize,
        ..Default::default()
    };

    let mut input = source;
    let mut stmts = Vec::new();

    // Skip leading whitespace and comments
    input = skip_ws_comments(input);

    while !input.is_empty() {
        ctx.tick()?;
        let (rest, stmt) = parse_stmt(input, &mut ctx).map_err(|e| match e {
            nom::Err::Error(e) | nom::Err::Failure(e) => e,
            nom::Err::Incomplete(_) => ParseError::new("unexpected end of input"),
        })?;
        stmts.push(stmt);
        input = skip_ws_comments(rest);
    }

    Ok(Program { stmts })
}

// ==================== Keyword Helpers ====================

/// Reserved keywords that cannot be used as variable names
const RESERVED_KEYWORDS: &[&str] = &[
    "let", "mut", "if", "else", "for", "while", "in", "return", "true", "false", "nil",
];

/// Check if `name` is a reserved keyword
fn is_reserved_keyword(name: &str) -> bool {
    RESERVED_KEYWORDS.contains(&name)
}

/// Check if input starts with a keyword followed by whitespace (space, tab, newline)
/// or a specific delimiter like '(' for if/while.
fn starts_with_keyword(input: &str, keyword: &str) -> bool {
    if !input.starts_with(keyword) {
        return false;
    }
    let rest = &input[keyword.len()..];
    // Must be followed by whitespace or end of input
    rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace())
}

// ==================== Whitespace & Comments ====================

fn skip_ws_comments(mut input: &str) -> &str {
    loop {
        // Skip whitespace
        let trimmed = input.trim_start();
        if trimmed.starts_with("//") {
            // Skip line comment
            if let Some(pos) = trimmed.find('\n') {
                input = &trimmed[pos + 1..];
            } else {
                return ""; // comment extends to end
            }
        } else {
            return trimmed;
        }
    }
}

fn ws(input: &str) -> &str {
    skip_ws_comments(input)
}

// ==================== Statement Parsing ====================

fn parse_stmt<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Stmt, ParseError> {
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
    let (input, name) = parse_identifier(input)
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
    let (input, value) = parse_expr(input, ctx)?;

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

    let (rest, condition) = parse_expr(current_input_trimmed, ctx)?;
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

            let (rest, condition) = parse_expr(trimmed, ctx)?;
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
    let (input, var_name) = parse_identifier(input)
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
        let (input, start) = parse_expr(input, ctx)?;
        let input = ws(input);
        if !input.starts_with(',') {
            ctx.leave_nesting();
            return Err(nom::Err::Failure(ParseError::new("expected ',' in range(start, end)")));
        }
        let input = ws(&input[1..]);
        let (input, end) = parse_expr(input, ctx)?;
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
    let (input, iterable) = parse_expr(input, ctx)?;
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

    let (input, condition) = parse_expr(input, ctx)?;
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

    if input.starts_with("deny(") || input.starts_with("deny (") {
        let input = input
            .strip_prefix("deny(")
            .or_else(|| input.strip_prefix("deny ("))
            .expect("guard ensures one of these matches");
        let input = ws(input);
        let (input, status) = parse_expr(input, ctx)?;
        let input = ws(input);
        if !input.starts_with(',') {
            return Err(nom::Err::Failure(ParseError::new("expected ',' in deny(status, body)")));
        }
        let input = ws(&input[1..]);
        let (input, body) = parse_expr(input, ctx)?;
        let input = ws(input);
        if !input.starts_with(')') {
            return Err(nom::Err::Failure(ParseError::new("expected ')' after deny arguments")));
        }
        return Ok((&input[1..], Stmt::new(StmtKind::ReturnDeny { status, body })));
    }

    Err(nom::Err::Failure(ParseError::new(
        "expected 'next()' or 'deny(status, body)' after 'return'",
    )))
}

fn parse_assign_or_expr_stmt<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Stmt, ParseError> {
    ctx.add_node().map_err(nom::Err::Failure)?;

    // Try to parse an expression first
    let (rest, expr) = parse_expr(input, ctx)?;

    // Check if this is an assignment: identifier followed by '='
    let rest_ws = ws(rest);
    if rest_ws.starts_with('=') && !rest_ws.starts_with("==") {
        if let ExprKind::Ident(name) = &expr.kind {
            let rest_ws = ws(&rest_ws[1..]);
            let (rest_ws, value) = parse_expr(rest_ws, ctx)?;
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
fn parse_block<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Vec<Stmt>, ParseError> {
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

// ==================== Expression Parsing (Pratt / Precedence Climbing) ====================

fn parse_expr<'a>(input: &'a str, ctx: &mut ParseCtx) -> IResult<&'a str, Expr, ParseError> {
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
        return parse_string_literal(input, ctx);
    }

    // Integer literal
    if input.starts_with(|c: char| c.is_ascii_digit()) {
        return parse_int_literal(input);
    }

    // Keywords and identifiers
    if input.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        let (rest, ident) =
            parse_identifier(input).map_err(|_| nom::Err::Failure(ParseError::new("expected identifier")))?;

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

    Err(nom::Err::Failure(ParseError::new(format!(
        "unexpected token: '{}'",
        &input[..input.len().min(20)]
    ))))
}

// ==================== Dot Access (Method Call / Field Access) ====================

fn parse_dot_access<'a>(input: &'a str, object: Expr, ctx: &mut ParseCtx) -> IResult<&'a str, Expr, ParseError> {
    ctx.tick().map_err(nom::Err::Failure)?;
    ctx.add_node().map_err(nom::Err::Failure)?;

    let input = &input[1..]; // skip '.'
    let (input, method) = parse_identifier(input)
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

// ==================== Literals ====================

fn parse_string_literal<'a>(input: &'a str, _ctx: &mut ParseCtx) -> IResult<&'a str, Expr, ParseError> {
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

fn parse_int_literal(input: &str) -> IResult<&str, Expr, ParseError> {
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

fn parse_identifier(input: &str) -> IResult<&str, &str, ParseError> {
    if input.is_empty() || (!input.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')) {
        return Err(nom::Err::Error(ParseError::new("expected identifier")));
    }
    let end = input
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(input.len());
    Ok((&input[end..], &input[..end]))
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
    use super::*;

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
    fn test_parse_comments() {
        let prog = parse_program(
            r#"
            // This is a comment
            let x = 42
            // Another comment
            let y = 10
        "#,
        )
        .unwrap();
        assert_eq!(prog.stmts.len(), 2);
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
    fn test_source_too_large() {
        let source = "a".repeat(MAX_SOURCE_LEN + 1);
        assert!(parse_program(&source).is_err());
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
}
