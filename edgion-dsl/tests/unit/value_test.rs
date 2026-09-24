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
