//! AST node definitions for EdgionDSL
//!
//! Two main enums: Stmt (statements) and Expr (expressions).
//! Program = Vec<Stmt> — a script is a flat list of statements.

use super::error::Span;

/// A complete DSL program — a list of top-level statements
#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

// ==================== Statements ====================

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Option<Span>,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    /// `let x = expr` or `let mut x = expr`
    Let { name: String, mutable: bool, value: Expr },

    /// `x = expr` (assignment to mutable variable)
    Assign { name: String, value: Expr },

    /// `if cond { body } else if cond { body } else { body }`
    If {
        branches: Vec<(Expr, Vec<Stmt>)>, // (condition, body) pairs
        else_body: Option<Vec<Stmt>>,
    },

    /// `for name in iterable { body }`
    ForIn {
        var_name: String,
        iterable: Expr,
        body: Vec<Stmt>,
    },

    /// `for name in range(start, end) { body }`
    ForRange {
        var_name: String,
        start: Expr,
        end: Expr,
        body: Vec<Stmt>,
    },

    /// `while condition { body }`
    While { condition: Expr, body: Vec<Stmt> },

    /// `return deny(status, body)`
    ReturnDeny { status: Expr, body: Expr },

    /// `return next()`
    ReturnNext,

    /// Expression statement — result is discarded
    ExprStmt { expr: Expr },
}

// ==================== Expressions ====================

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Option<Span>,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    /// String literal: `"hello"`
    StringLit(String),
    /// Integer literal: `42`, `-1`
    IntLit(i64),
    /// Boolean literal: `true`, `false`
    BoolLit(bool),
    /// Nil literal: `nil`
    NilLit,
    /// Variable reference: `x`, `ip`
    Ident(String),
    /// Binary operation: `a + b`, `a == b`, `a && b`
    BinaryOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Unary operation: `!x`, `-n`
    UnaryOp { op: UnaryOp, operand: Box<Expr> },
    /// Method call: `req.header("X")`, `s.starts_with("/")`
    MethodCall {
        object: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },
    /// Free function call: `log("msg")`, `len(s)`
    FnCall { name: String, args: Vec<Expr> },
    /// Field access: `req.path` (sugar for 0-arg method call)
    FieldAccess { object: Box<Expr>, field: String },
}

// ==================== Operators ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div, // arithmetic (+ also string concat)
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge, // comparison
    And,
    Or, // logical
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not, // !
    Neg, // - (negative)
}

// ==================== Operator Precedence ====================

impl BinOp {
    /// Precedence level (higher = binds tighter).
    /// Used by Pratt parser / precedence climbing.
    pub fn precedence(&self) -> u8 {
        match self {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::Eq | BinOp::Ne => 3,
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => 4,
            BinOp::Add | BinOp::Sub => 5,
            BinOp::Mul | BinOp::Div => 6,
        }
    }
}

// ==================== AST Construction Helpers ====================

impl Stmt {
    pub fn new(kind: StmtKind) -> Self {
        Self { kind, span: None }
    }
    pub fn with_span(kind: StmtKind, span: Span) -> Self {
        Self { kind, span: Some(span) }
    }
}

impl Expr {
    pub fn new(kind: ExprKind) -> Self {
        Self { kind, span: None }
    }
    pub fn with_span(kind: ExprKind, span: Span) -> Self {
        Self { kind, span: Some(span) }
    }
    pub fn string(s: impl Into<String>) -> Self {
        Self::new(ExprKind::StringLit(s.into()))
    }
    pub fn int(n: i64) -> Self {
        Self::new(ExprKind::IntLit(n))
    }
    pub fn ident(name: impl Into<String>) -> Self {
        Self::new(ExprKind::Ident(name.into()))
    }
    pub fn nil() -> Self {
        Self::new(ExprKind::NilLit)
    }
    pub fn bool_val(b: bool) -> Self {
        Self::new(ExprKind::BoolLit(b))
    }
}
