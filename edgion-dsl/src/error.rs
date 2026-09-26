//! Error types for EdgionDSL — parse, compile, validation, and runtime errors

use std::fmt;

/// Source position for error reporting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Byte offset from start of source
    pub offset: usize,
    /// Line number (1-based)
    pub line: u32,
    /// Column number (1-based)
    pub col: u32,
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, col {}", self.line, self.col)
    }
}

// ========== Parse Error ==========

/// Error during parsing (nom → AST)
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Option<Span>,
}

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(span) = &self.span {
            write!(f, "parse error at {}: {}", span, self.message)
        } else {
            write!(f, "parse error: {}", self.message)
        }
    }
}

impl std::error::Error for ParseError {}

// ========== Compile Error ==========

#[derive(Debug, Clone)]
pub struct CompileError {
    pub message: String,
    pub span: Option<Span>,
}

impl CompileError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
        }
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(span) = &self.span {
            write!(f, "compile error at {}: {}", span, self.message)
        } else {
            write!(f, "compile error: {}", self.message)
        }
    }
}

impl std::error::Error for CompileError {}

// ========== Validation Error ==========

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub message: String,
    pub span: Option<Span>,
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(span) = &self.span {
            write!(f, "validation error at {}: {}", span, self.message)
        } else {
            write!(f, "validation error: {}", self.message)
        }
    }
}

impl std::error::Error for ValidationError {}

// ========== Runtime Error ==========

/// Error during VM execution (gateway side).
/// Kept lightweight — no source spans needed at runtime.
/// Each variant maps to a specific safety limit or runtime condition.
#[derive(Debug, Clone)]
pub enum RuntimeError {
    /// Type mismatch in an operation (e.g., Int + Bool)
    TypeError {
        expected: &'static str,
        got: &'static str,
        operation: &'static str,
    },
    /// Division by zero
    DivisionByZero,
    /// Integer arithmetic overflow
    IntegerOverflow { operation: &'static str },
    /// step_budget exhausted — total instructions exceeded limit
    StepLimitExceeded { limit: u32 },
    /// loop_budget exhausted — single loop iterations exceeded limit
    LoopLimitExceeded { limit: u32 },
    /// call_budget exhausted — builtin API calls exceeded limit
    CallLimitExceeded { limit: u32 },
    /// Stack depth exceeded limit
    StackOverflow { limit: usize },
    /// String concatenation result too long.
    ///
    /// `len` and `limit` are both **UTF-8 byte counts** — this is a memory-budget
    /// guard for the sandbox, not a logical character-count guard. The DSL `len()`
    /// builtin returns Unicode character count and is *not* the same unit; see the
    /// `len` entry in `docs/.../dsl/api-reference.md` for the contract.
    StringTooLong { len: usize, limit: usize },
    /// List exceeded element-count limit
    ListTooLong { len: usize, limit: usize },
    /// Map exceeded entry-count limit
    MapTooLarge { size: usize, limit: usize },
    /// A Map contains too many nested Map levels.
    MapTooDeep { limit: usize },
    /// A Value contains too many Map entries and List elements in total.
    ValueTooManyItems { limit: usize },
    /// A Value contains too many Map-key bytes in total.
    ValueKeysTooLong { limit: usize },
    /// Value traversal, cloning, and comparison exhausted the execution budget.
    ValueWorkLimitExceeded { limit: u64 },
    /// PluginSession API returned error
    ApiError { function: String, message: String },
    /// Request-body capture, storage, scan, or mutation failed.
    ///
    /// The gateway treats this typed condition as fixed fail-close instead of
    /// routing it through the configurable DSL API-error policy.
    BodyAccess { message: String },
    /// Regex execution error
    RegexError { message: String },
    /// Internal VM error (should not happen)
    Internal { message: String },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypeError {
                expected,
                got,
                operation,
            } => write!(f, "type error in {}: expected {}, got {}", operation, expected, got),
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::IntegerOverflow { operation } => {
                write!(f, "integer overflow in {}", operation)
            }
            Self::StepLimitExceeded { limit } => write!(f, "step budget exceeded ({})", limit),
            Self::LoopLimitExceeded { limit } => write!(f, "loop budget exceeded ({})", limit),
            Self::CallLimitExceeded { limit } => write!(f, "call budget exceeded ({})", limit),
            Self::StackOverflow { limit } => write!(f, "stack overflow (depth {})", limit),
            Self::StringTooLong { len, limit } => {
                write!(f, "string too long ({} bytes > {} bytes)", len, limit)
            }
            Self::ListTooLong { len, limit } => {
                write!(f, "list too long ({} > {})", len, limit)
            }
            Self::MapTooLarge { size, limit } => {
                write!(f, "map too large ({} > {})", size, limit)
            }
            Self::MapTooDeep { limit } => write!(f, "map depth exceeded ({})", limit),
            Self::ValueTooManyItems { limit } => write!(f, "value item limit exceeded ({})", limit),
            Self::ValueKeysTooLong { limit } => write!(f, "value map-key byte limit exceeded ({})", limit),
            Self::ValueWorkLimitExceeded { limit } => write!(f, "value work budget exceeded ({})", limit),
            Self::ApiError { function, message } => {
                write!(f, "API error in {}: {}", function, message)
            }
            Self::BodyAccess { message } => write!(f, "body access failed: {}", message),
            Self::RegexError { message } => write!(f, "regex error: {}", message),
            Self::Internal { message } => write!(f, "internal error: {}", message),
        }
    }
}

impl std::error::Error for RuntimeError {}

#[cfg(test)]
#[path = "../tests/unit/error_test.rs"]
mod tests;
