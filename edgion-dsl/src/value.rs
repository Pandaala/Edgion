//! Runtime value types for EdgionDSL
//!
//! User-visible types: Str, Int, Bool, Nil, Body, Map.
//! Plus an internal List type for iteration (not user-constructable directly).

use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::RuntimeError;
use crate::xml::XmlValue;

/// Fixed VM value limits, independent of the configurable per-container limits.
pub const MAX_VALUE_MAP_DEPTH: usize = 64;
pub const MAX_VALUE_CONTAINER_ITEMS: usize = 32_768;
pub const MAX_VALUE_MAP_KEY_BYTES: usize = 262_144;
pub const MAX_VALUE_WORK_UNITS: u64 = 16 * 1024 * 1024;

const ITEM_WORK_UNITS: u64 = 32;

/// The bounded shape of one Value. Strings share their bytes when cloned, but
/// comparisons can visit their contents, so both costs are tracked separately.
#[derive(Clone, Copy, Debug, Default)]
pub struct ValueShape {
    map_depth: usize,
    container_items: usize,
    map_key_bytes: usize,
    string_bytes: u64,
}

impl ValueShape {
    pub fn map_root() -> Self {
        Self {
            map_depth: 1,
            ..Self::default()
        }
    }

    pub fn add_map_entry(&mut self, key_bytes: usize, child: Self) -> Result<(), RuntimeError> {
        self.container_items = self
            .container_items
            .saturating_add(1)
            .saturating_add(child.container_items);
        if self.container_items > MAX_VALUE_CONTAINER_ITEMS {
            return Err(RuntimeError::ValueTooManyItems {
                limit: MAX_VALUE_CONTAINER_ITEMS,
            });
        }
        self.map_key_bytes = self
            .map_key_bytes
            .saturating_add(key_bytes)
            .saturating_add(child.map_key_bytes);
        if self.map_key_bytes > MAX_VALUE_MAP_KEY_BYTES {
            return Err(RuntimeError::ValueKeysTooLong {
                limit: MAX_VALUE_MAP_KEY_BYTES,
            });
        }
        self.map_depth = self.map_depth.max(child.map_depth.saturating_add(1));
        if self.map_depth > MAX_VALUE_MAP_DEPTH {
            return Err(RuntimeError::MapTooDeep {
                limit: MAX_VALUE_MAP_DEPTH,
            });
        }
        self.string_bytes = self.string_bytes.saturating_add(child.string_bytes);
        Ok(())
    }

    fn clone_work(self) -> u64 {
        (self.container_items as u64)
            .saturating_mul(ITEM_WORK_UNITS)
            .saturating_add(self.map_key_bytes as u64)
    }

    fn compare_work(self) -> u64 {
        self.clone_work().saturating_add(self.string_bytes)
    }
}

/// Per-execution accounting for recursive Value operations. Shape inspection
/// itself spends budget as it visits each container item and Map key.
pub struct ValueWorkBudget {
    remaining: u64,
}

impl Default for ValueWorkBudget {
    fn default() -> Self {
        Self {
            remaining: MAX_VALUE_WORK_UNITS,
        }
    }
}

impl ValueWorkBudget {
    /// Placeholder while an owned scanner exclusively owns the live account.
    pub fn exhausted() -> Self {
        Self { remaining: 0 }
    }

    pub fn charge(&mut self, units: u64) -> Result<(), RuntimeError> {
        self.remaining = self
            .remaining
            .checked_sub(units)
            .ok_or(RuntimeError::ValueWorkLimitExceeded {
                limit: MAX_VALUE_WORK_UNITS,
            })?;
        Ok(())
    }

    pub fn charge_clone(&mut self, shape: ValueShape) -> Result<(), RuntimeError> {
        self.charge(shape.clone_work())
    }

    pub fn charge_comparison(&mut self, left: ValueShape, right: ValueShape) -> Result<(), RuntimeError> {
        self.charge(left.compare_work().saturating_add(right.compare_work()))
    }
}

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
    /// Immutable bounded XML node; copies share the node without traversal.
    Xml(XmlValue),
    /// Internal: list of strings for iteration (not user-constructable directly)
    List(Vec<Arc<str>>),
    /// Structured map — returned by http.call and constructable via map literals.
    /// Boxed to keep `size_of::<Value>()` at 24 B; otherwise the inline IndexMap
    /// (~72 B) would dominate VmState's stack/locals SmallVec inline capacity.
    Map(Box<IndexMap<String, Value>>),
}

impl Value {
    /// Inspect a Value without allocating or descending beyond the fixed Map
    /// depth. The traversal is charged before each container item is visited.
    pub fn inspect_shape(&self, budget: &mut ValueWorkBudget) -> Result<ValueShape, RuntimeError> {
        fn walk(value: &Value, depth: usize, budget: &mut ValueWorkBudget) -> Result<ValueShape, RuntimeError> {
            match value {
                Value::Map(map) => {
                    let next_depth = depth.saturating_add(1);
                    if next_depth > MAX_VALUE_MAP_DEPTH {
                        return Err(RuntimeError::MapTooDeep {
                            limit: MAX_VALUE_MAP_DEPTH,
                        });
                    }
                    let mut shape = ValueShape::map_root();
                    for (key, value) in map.iter() {
                        budget.charge(ITEM_WORK_UNITS.saturating_add(key.len() as u64))?;
                        let child = walk(value, next_depth, budget)?;
                        shape.add_map_entry(key.len(), child)?;
                    }
                    Ok(shape)
                }
                Value::List(items) => {
                    if items.len() > MAX_VALUE_CONTAINER_ITEMS {
                        return Err(RuntimeError::ValueTooManyItems {
                            limit: MAX_VALUE_CONTAINER_ITEMS,
                        });
                    }
                    budget.charge((items.len() as u64).saturating_mul(ITEM_WORK_UNITS))?;
                    let mut string_bytes = 0u64;
                    for item in items {
                        string_bytes = string_bytes.saturating_add(item.len() as u64);
                    }
                    Ok(ValueShape {
                        container_items: items.len(),
                        string_bytes,
                        ..ValueShape::default()
                    })
                }
                Value::Str(value) => Ok(ValueShape {
                    string_bytes: value.len() as u64,
                    ..ValueShape::default()
                }),
                _ => Ok(ValueShape::default()),
            }
        }

        walk(self, 0, budget)
    }

    /// Byte length of the existing implicit string representation, without
    /// cloning a Map or allocating a potentially large String.
    pub fn implicit_string_len(&self) -> Result<usize, ImplicitStringCoercionError> {
        match self {
            Value::Str(value) => Ok(value.len()),
            Value::Int(value) => Ok(value.to_string().len()),
            Value::Bool(true) => Ok(4),
            Value::Bool(false) => Ok(5),
            Value::Nil => Ok(3),
            Value::Body(_) => Err(ImplicitStringCoercionError::new("Body")),
            Value::Xml(_) => Err(ImplicitStringCoercionError::new("Xml")),
            Value::List(_) => Ok(6),
            Value::Map(map) => Ok(6 + map.len().to_string().len()),
        }
    }

    // ========== Type coercion ==========

    /// Get a string reference using the legacy nil-friendly coercion rules.
    ///
    /// Non-string scalar/container values retain the previous empty-string
    /// fallback. Body is different: accessing its bytes requires an explicit
    /// bounded body operation, so implicit conversion is rejected.
    pub fn try_as_str(&self) -> Result<&str, ImplicitStringCoercionError> {
        match self {
            Value::Str(s) => Ok(s.as_ref()),
            Value::Body(_) | Value::Xml(_) => Err(ImplicitStringCoercionError::new(self.type_name())),
            _ => Ok(""),
        }
    }

    /// Convert to an owned string using the existing scalar coercion rules.
    ///
    /// Body conversion is intentionally fallible. Callers that need body text
    /// must use the explicit bounded `read(offset, max_bytes)` DSL operation.
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
            Value::Xml(_) => Err(ImplicitStringCoercionError::new("Xml")),
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
            Value::Xml(_) => "Xml",
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
            Value::Xml(_) => write!(f, "[xml]"),
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
            (Value::Xml(a), Value::Xml(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            _ => false, // different types are never equal
        }
    }
}

impl Eq for Value {}

#[cfg(test)]
#[path = "../tests/unit/value_test.rs"]
mod tests;
