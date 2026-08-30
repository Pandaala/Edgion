//! Runtime value types for EdgionDSL
//!
//! User-visible types: Str, Int, Bool, Nil, Body, Map.
//! Plus an internal List type for iteration (not user-constructable directly).

use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;

/// Opaque host-backed body values exposed to the DSL.
///
/// The tag deliberately carries no bytes, cursor, file handle, or gateway
/// reference. Runtime operations resolve it against the current request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyValue {
    /// The proxied request body associated with the current DSL execution.
    Request,
}

/// A value that cannot participate in implicit string coercion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImplicitStringCoercionError {
    got: &'static str,
}

impl ImplicitStringCoercionError {
    fn new(got: &'static str) -> Self {
        Self { got }
    }

    pub fn got(&self) -> &'static str {
        self.got
    }
}

impl fmt::Display for ImplicitStringCoercionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} cannot be implicitly converted to Str", self.got)
    }
}

impl std::error::Error for ImplicitStringCoercionError {}

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
    /// Opaque request body token. Body bytes remain owned by the host runtime.
    Body(BodyValue),
    /// Internal: list of strings for iteration (not user-constructable directly)
    List(Vec<Arc<str>>),
    /// Structured map — returned by http.call and constructable via map literals.
    /// Boxed to keep `size_of::<Value>()` at 24 B; otherwise the inline IndexMap
    /// (~72 B) would dominate VmState's stack/locals SmallVec inline capacity.
    Map(Box<IndexMap<String, Value>>),
}

impl Value {
    // ========== Type coercion ==========

    /// Get a string reference using the legacy nil-friendly coercion rules.
    ///
    /// Non-string scalar/container values retain the previous empty-string
    /// fallback. Body is different: accessing its bytes requires an explicit
    /// bounded body operation, so implicit conversion is rejected.
    pub fn try_as_str(&self) -> Result<&str, ImplicitStringCoercionError> {
        match self {
            Value::Str(s) => Ok(s.as_ref()),
            Value::Body(_) => Err(ImplicitStringCoercionError::new(self.type_name())),
            _ => Ok(""),
        }
    }

    /// Convert to an owned string using the existing scalar coercion rules.
    ///
    /// Body conversion is intentionally fallible. Callers that need body text
    /// must use the explicit bounded `read` or `read_all` DSL operations.
    pub fn try_into_string(self) -> Result<String, ImplicitStringCoercionError> {
        match self {
            Value::Str(s) => Ok(s.to_string()),
            Value::Int(n) => Ok(n.to_string()),
            Value::Bool(b) => {
                if b {
                    Ok("true".to_string())
                } else {
                    Ok("false".to_string())
                }
            }
            Value::Nil => Ok("nil".to_string()),
            Value::Body(_) => Err(ImplicitStringCoercionError::new("Body")),
            Value::List(_) => Ok("[list]".to_string()),
            Value::Map(m) => Ok(format!("[map:{}]", m.len())),
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
            Value::Body(_) => "Body",
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
            Value::Body(BodyValue::Request) => write!(f, "[body:request]"),
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
            (Value::Body(a), Value::Body(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            _ => false, // different types are never equal
        }
    }
}

impl Eq for Value {}

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
        assert!(Value::Body(BodyValue::Request).is_truthy());
    }

    #[test]
    fn test_equality() {
        assert_eq!(Value::Nil, Value::Nil);
        assert_eq!(Value::Int(42), Value::Int(42));
        assert_ne!(Value::Int(1), Value::Str(Arc::from("1"))); // cross-type never equal
        assert_ne!(Value::Nil, Value::Bool(false)); // nil != false
        assert_eq!(Value::Body(BodyValue::Request), Value::Body(BodyValue::Request));
    }

    #[test]
    fn test_map_basic() {
        let mut m = IndexMap::new();
        m.insert("status".to_string(), Value::Int(200));
        m.insert("body".to_string(), Value::Str(Arc::from("ok")));
        let v = Value::Map(Box::new(m));
        assert!(v.is_truthy());
        assert_eq!(v.type_name(), "Map");
        assert_eq!(format!("{}", v), "[map:2]");
    }

    #[test]
    fn test_map_equality() {
        let mut a = IndexMap::new();
        a.insert("k".to_string(), Value::Int(1));
        let mut b = IndexMap::new();
        b.insert("k".to_string(), Value::Int(1));
        assert_eq!(Value::Map(Box::new(a)), Value::Map(Box::new(b)));
    }

    #[test]
    fn test_map_clone() {
        let mut m = IndexMap::new();
        m.insert("x".to_string(), Value::Bool(true));
        let v = Value::Map(Box::new(m));
        let v2 = v.clone();
        assert_eq!(v, v2);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Value::Str(Arc::from("hello"))), "hello");
        assert_eq!(format!("{}", Value::Int(42)), "42");
        assert_eq!(format!("{}", Value::Bool(true)), "true");
        assert_eq!(format!("{}", Value::Nil), "nil");
        assert_eq!(format!("{}", Value::Body(BodyValue::Request)), "[body:request]");
    }

    /// Lock the layout: any change pushing `Value` past 24 B blows up
    /// VmState's SmallVec inline capacity. If this fails, evaluate boxing
    /// the new variant rather than relaxing the bound.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn assert_value_size_invariant() {
        assert_eq!(std::mem::size_of::<Value>(), 24);
    }

    #[test]
    fn test_try_into_string() {
        assert_eq!(Value::Str(Arc::from("hello")).try_into_string().unwrap(), "hello");
        assert_eq!(Value::Int(42).try_into_string().unwrap(), "42");
        assert_eq!(Value::Bool(true).try_into_string().unwrap(), "true");
        assert_eq!(Value::Bool(false).try_into_string().unwrap(), "false");
        assert_eq!(Value::Nil.try_into_string().unwrap(), "nil");
    }

    #[test]
    fn body_rejects_implicit_string_coercion() {
        let body = Value::Body(BodyValue::Request);
        let err = body.clone().try_into_string().unwrap_err();
        assert_eq!(err.got(), "Body");
        assert!(body.try_as_str().is_err());
    }
}
