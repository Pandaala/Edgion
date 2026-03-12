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

impl Span {
    pub fn new(offset: usize, line: u32, col: u32) -> Self {
        Self { offset, line, col }
    }
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
    pub fn with_span(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span: Some(span),
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
    pub fn with_span(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span: Some(span),
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
    /// String concatenation result too long
    StringTooLong { len: usize, limit: usize },
    /// Access to uninitialized local variable slot
    UndefinedLocal { slot: u16 },
    /// PluginSession API returned error
    ApiError { function: String, message: String },
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
                write!(f, "string too long ({} > {})", len, limit)
            }
            Self::UndefinedLocal { slot } => write!(f, "undefined local (slot {})", slot),
            Self::ApiError { function, message } => {
                write!(f, "API error in {}: {}", function, message)
            }
            Self::RegexError { message } => write!(f, "regex error: {}", message),
            Self::Internal { message } => write!(f, "internal error: {}", message),
        }
    }
}

impl std::error::Error for RuntimeError {}

// ========== Unified DSL Error ==========

#[derive(Debug)]
pub enum DslError {
    Parse(ParseError),
    Compile(CompileError),
    Validation(ValidationError),
    Runtime(RuntimeError),
}

impl fmt::Display for DslError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "{}", e),
            Self::Compile(e) => write!(f, "{}", e),
            Self::Validation(e) => write!(f, "{}", e),
            Self::Runtime(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for DslError {}

impl From<ParseError> for DslError {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}
impl From<CompileError> for DslError {
    fn from(e: CompileError) -> Self {
        Self::Compile(e)
    }
}
impl From<ValidationError> for DslError {
    fn from(e: ValidationError) -> Self {
        Self::Validation(e)
    }
}
impl From<RuntimeError> for DslError {
    fn from(e: RuntimeError) -> Self {
        Self::Runtime(e)
    }
}
