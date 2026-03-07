//! Runtime value types for EdgionDSL
//!
//! Only 4 user-visible types: Str, Int, Bool, Nil — matching the HTTP domain.
//! Plus an internal List type for iteration (not user-constructable).

use std::fmt;

/// Runtime value — the types of EdgionDSL
#[derive(Debug, Clone)]
pub enum Value {
    /// String value — primary type, all HTTP data
    Str(String),
    /// Integer value — status codes, lengths, counters
    Int(i64),
    /// Boolean value — condition results
    Bool(bool),
    /// Nil — missing/absent value (maps to Option::None)
    Nil,
    /// Internal: list of strings for iteration (not user-constructable)
    List(Vec<String>),
}

impl Value {
    // ========== Type checking ==========

    #[inline]
    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    #[inline]
    pub fn is_str(&self) -> bool {
        matches!(self, Value::Str(_))
    }

    #[inline]
    pub fn is_int(&self) -> bool {
        matches!(self, Value::Int(_))
    }

    #[inline]
    pub fn is_bool(&self) -> bool {
        matches!(self, Value::Bool(_))
    }

    #[inline]
    pub fn is_list(&self) -> bool {
        matches!(self, Value::List(_))
    }

    // ========== Type coercion ==========

    /// Get string ref; returns empty string for non-Str types.
    /// Used when script passes a value to PluginSession string APIs.
    pub fn as_str(&self) -> &str {
        match self {
            Value::Str(s) => s.as_str(),
            _ => "",
        }
    }

    /// Convert to owned String representation.
    /// Str → itself, Int → decimal, Bool → "true"/"false", Nil → "nil"
    pub fn into_string(self) -> String {
        match self {
            Value::Str(s) => s,
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => {
                if b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            Value::Nil => "nil".to_string(),
            Value::List(_) => "[list]".to_string(),
        }
    }

    /// Get integer value, or None
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Get boolean value, or None
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Get list ref, or None
    pub fn as_list(&self) -> Option<&Vec<String>> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    // ========== Truthiness (Lua-style) ==========

    /// Nil and false are falsy, everything else is truthy.
    /// Used for `if` conditions and logical operators.
    #[inline]
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            _ => true, // non-empty strings, all ints (including 0), lists are truthy
        }
    }

    // ========== Type name (for error messages) ==========

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Str(_) => "Str",
            Value::Int(_) => "Int",
            Value::Bool(_) => "Bool",
            Value::Nil => "Nil",
            Value::List(_) => "List",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Str(s) => write!(f, "{}", s),
            Value::Int(n) => write!(f, "{}", n),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Nil => write!(f, "nil"),
            Value::List(l) => write!(f, "[list:{}]", l.len()),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Nil, Value::Nil) => true,
            (Value::List(a), Value::List(b)) => a == b,
            _ => false, // different types are never equal
        }
    }
}

impl Eq for Value {}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

/// Convert Option<String> → Value::Str or Value::Nil
/// This is the primary bridge from PluginSession return values
impl From<Option<String>> for Value {
    fn from(opt: Option<String>) -> Self {
        match opt {
            Some(s) => Value::Str(s),
            None => Value::Nil,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truthiness() {
        assert!(!Value::Nil.is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Bool(true).is_truthy());
        assert!(Value::Str("".to_string()).is_truthy()); // empty string IS truthy (like Lua)
        assert!(Value::Str("hello".to_string()).is_truthy());
        assert!(Value::Int(0).is_truthy()); // zero IS truthy (like Lua)
        assert!(Value::Int(42).is_truthy());
    }

    #[test]
    fn test_equality() {
        assert_eq!(Value::Nil, Value::Nil);
        assert_eq!(Value::Int(42), Value::Int(42));
        assert_ne!(Value::Int(1), Value::Str("1".to_string())); // cross-type never equal
        assert_ne!(Value::Nil, Value::Bool(false)); // nil != false
    }

    #[test]
    fn test_from_option() {
        let v: Value = Some("hello".to_string()).into();
        assert_eq!(v, Value::Str("hello".to_string()));
        let v: Value = None::<String>.into();
        assert_eq!(v, Value::Nil);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Value::Str("hello".to_string())), "hello");
        assert_eq!(format!("{}", Value::Int(42)), "42");
        assert_eq!(format!("{}", Value::Bool(true)), "true");
        assert_eq!(format!("{}", Value::Nil), "nil");
    }

    #[test]
    fn test_into_string() {
        assert_eq!(Value::Str("hello".to_string()).into_string(), "hello");
        assert_eq!(Value::Int(42).into_string(), "42");
        assert_eq!(Value::Bool(true).into_string(), "true");
        assert_eq!(Value::Bool(false).into_string(), "false");
        assert_eq!(Value::Nil.into_string(), "nil");
    }
}
