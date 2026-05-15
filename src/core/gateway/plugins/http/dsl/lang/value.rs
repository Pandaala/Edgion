//! Runtime value types for EdgionDSL
//!
//! User-visible types: Str, Int, Bool, Nil, Map.
//! Plus an internal List type for iteration (not user-constructable directly).

use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;

/// Runtime value — the types of EdgionDSL
#[derive(Debug, Clone)]
pub enum Value {
    /// String value — primary type, all HTTP data
    Str(Arc<str>),
    /// Integer value — status codes, lengths, counters
    Int(i64),
    /// Boolean value — condition results
    Bool(bool),
    /// Nil — missing/absent value (maps to Option::None)
    Nil,
    /// Internal: list of strings for iteration (not user-constructable directly)
    List(Vec<Arc<str>>),
    /// Structured map — returned by http.call and constructable via map literals
    Map(IndexMap<String, Value>),
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

    #[inline]
    pub fn is_map(&self) -> bool {
        matches!(self, Value::Map(_))
    }

    // ========== Type coercion ==========

    /// Get string ref; returns empty string for non-Str types.
    /// Used when script passes a value to PluginSession string APIs.
    pub fn as_str(&self) -> &str {
        match self {
            Value::Str(s) => s.as_ref(),
            _ => "",
        }
    }

    /// Convert to owned String representation.
    /// Str → itself, Int → decimal, Bool → "true"/"false", Nil → "nil"
    pub fn into_string(self) -> String {
        match self {
            Value::Str(s) => s.to_string(),
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
            Value::Map(m) => format!("[map:{}]", m.len()),
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
    pub fn as_list(&self) -> Option<&Vec<Arc<str>>> {
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
            Value::Map(_) => "Map",
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
            Value::Map(m) => write!(f, "[map:{}]", m.len()),
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
            (Value::Map(a), Value::Map(b)) => a == b,
            _ => false, // different types are never equal
        }
    }
}

impl Eq for Value {}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(Arc::from(s))
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(Arc::from(s))
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
            Some(s) => Value::Str(Arc::from(s)),
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
        assert!(Value::Str(Arc::from("")).is_truthy()); // empty string IS truthy (like Lua)
        assert!(Value::Str(Arc::from("hello")).is_truthy());
        assert!(Value::Int(0).is_truthy()); // zero IS truthy (like Lua)
        assert!(Value::Int(42).is_truthy());
    }

    #[test]
    fn test_equality() {
        assert_eq!(Value::Nil, Value::Nil);
        assert_eq!(Value::Int(42), Value::Int(42));
        assert_ne!(Value::Int(1), Value::Str(Arc::from("1"))); // cross-type never equal
        assert_ne!(Value::Nil, Value::Bool(false)); // nil != false
    }

    #[test]
    fn test_from_option() {
        let v: Value = Some("hello".to_string()).into();
        assert_eq!(v, Value::Str(Arc::from("hello")));
        let v: Value = None::<String>.into();
        assert_eq!(v, Value::Nil);
    }

    #[test]
    fn test_map_basic() {
        let mut m = IndexMap::new();
        m.insert("status".to_string(), Value::Int(200));
        m.insert("body".to_string(), Value::Str(Arc::from("ok")));
        let v = Value::Map(m);
        assert!(v.is_truthy());
        assert!(v.is_map());
        assert_eq!(v.type_name(), "Map");
        assert_eq!(format!("{}", v), "[map:2]");
    }

    #[test]
    fn test_map_equality() {
        let mut a = IndexMap::new();
        a.insert("k".to_string(), Value::Int(1));
        let mut b = IndexMap::new();
        b.insert("k".to_string(), Value::Int(1));
        assert_eq!(Value::Map(a), Value::Map(b));
    }

    #[test]
    fn test_map_clone() {
        let mut m = IndexMap::new();
        m.insert("x".to_string(), Value::Bool(true));
        let v = Value::Map(m);
        let v2 = v.clone();
        assert_eq!(v, v2);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Value::Str(Arc::from("hello"))), "hello");
        assert_eq!(format!("{}", Value::Int(42)), "42");
        assert_eq!(format!("{}", Value::Bool(true)), "true");
        assert_eq!(format!("{}", Value::Nil), "nil");
    }

    #[test]
    fn test_into_string() {
        assert_eq!(Value::Str(Arc::from("hello")).into_string(), "hello");
        assert_eq!(Value::Int(42).into_string(), "42");
        assert_eq!(Value::Bool(true).into_string(), "true");
        assert_eq!(Value::Bool(false).into_string(), "false");
        assert_eq!(Value::Nil.into_string(), "nil");
    }
}
