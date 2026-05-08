//! EdgionDSL parser — nom-based parser with input-layer defenses
//!
//! Security:
//!   - Source size limit (Layer 0)
//!   - Parse budget (prevents pathological backtracking)
//!   - Nesting depth limit (prevents stack overflow)
//!   - AST node count limit

mod expr;
mod lit;
mod stmt;

use super::ast::*;
use super::error::ParseError;

/// Maximum source code length (64KB)
const MAX_SOURCE_LEN: usize = 64 * 1024;

// ==================== Parse Context (Safety) ====================

/// Parser safety context — threaded through all parser functions.
pub(super) struct ParseCtx {
    /// Global parse budget — decremented on every combinator entry
    pub(super) parse_budget: u32,
    /// Current nesting depth
    pub(super) nesting_depth: u16,
    /// Max allowed nesting depth
    pub(super) max_nesting_depth: u16,
    /// Total AST nodes created
    pub(super) ast_node_count: u32,
    /// Max allowed AST nodes
    pub(super) max_ast_nodes: u32,
    /// Original source (for span calculation)
    pub(super) _source_start: usize,
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
    pub(super) fn tick(&mut self) -> Result<(), ParseError> {
        if self.parse_budget == 0 {
            return Err(ParseError::new("parse budget exceeded"));
        }
        self.parse_budget -= 1;
        Ok(())
    }

    pub(super) fn enter_nesting(&mut self) -> Result<(), ParseError> {
        self.nesting_depth += 1;
        if self.nesting_depth > self.max_nesting_depth {
            return Err(ParseError::new(format!(
                "nesting too deep: {} (max {})",
                self.nesting_depth, self.max_nesting_depth
            )));
        }
        Ok(())
    }

    pub(super) fn leave_nesting(&mut self) {
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
    }

    pub(super) fn add_node(&mut self) -> Result<(), ParseError> {
        self.ast_node_count += 1;
        if self.ast_node_count > self.max_ast_nodes {
            return Err(ParseError::new(format!(
                "too many AST nodes: {} (max {})",
                self.ast_node_count, self.max_ast_nodes
            )));
        }
        Ok(())
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
        let (rest, stmt) = stmt::parse_stmt(input, &mut ctx).map_err(|e| match e {
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
    "let", "mut", "if", "else", "for", "while", "in", "return", "true", "false", "nil", "break", "continue",
];

/// Check if `name` is a reserved keyword
pub(super) fn is_reserved_keyword(name: &str) -> bool {
    RESERVED_KEYWORDS.contains(&name)
}

/// Check if input starts with a keyword followed by whitespace (space, tab, newline)
/// or a specific delimiter like '(' for if/while.
pub(super) fn starts_with_keyword(input: &str, keyword: &str) -> bool {
    if !input.starts_with(keyword) {
        return false;
    }
    let rest = &input[keyword.len()..];
    // Must be followed by whitespace or end of input
    rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace())
}

// ==================== Whitespace & Comments ====================

pub(super) fn skip_ws_comments(mut input: &str) -> &str {
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

pub(super) fn ws(input: &str) -> &str {
    skip_ws_comments(input)
}

// ==================== Tests ====================

#[cfg(test)]
mod tests {
    use super::parse_program;

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
    fn test_source_too_large() {
        let source = "a".repeat(super::MAX_SOURCE_LEN + 1);
        assert!(parse_program(&source).is_err());
    }
}
