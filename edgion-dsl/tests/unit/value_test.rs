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

fn map_chain(depth: usize) -> Value {
    let mut value = Value::Nil;
    for _ in 0..depth {
        let mut map = IndexMap::new();
        map.insert("x".to_string(), value);
        value = Value::Map(Box::new(map));
    }
    value
}

#[test]
fn value_shape_enforces_map_depth_boundary() {
    let mut work = ValueWorkBudget::default();
    assert!(map_chain(MAX_VALUE_MAP_DEPTH).inspect_shape(&mut work).is_ok());
    let mut work = ValueWorkBudget::default();
    assert!(matches!(
        map_chain(MAX_VALUE_MAP_DEPTH + 1).inspect_shape(&mut work),
        Err(RuntimeError::MapTooDeep {
            limit: MAX_VALUE_MAP_DEPTH
        })
    ));
}

#[test]
fn value_shape_counts_all_container_items_and_key_bytes() {
    let items = Value::List(vec![Arc::from("x"); MAX_VALUE_CONTAINER_ITEMS]);
    assert!(items.inspect_shape(&mut ValueWorkBudget::default()).is_ok());
    let items = Value::List(vec![Arc::from("x"); MAX_VALUE_CONTAINER_ITEMS + 1]);
    assert!(matches!(
        items.inspect_shape(&mut ValueWorkBudget::default()),
        Err(RuntimeError::ValueTooManyItems { .. })
    ));

    let mut map = IndexMap::new();
    map.insert("k".repeat(MAX_VALUE_MAP_KEY_BYTES), Value::Nil);
    assert!(Value::Map(Box::new(map))
        .inspect_shape(&mut ValueWorkBudget::default())
        .is_ok());
    let mut map = IndexMap::new();
    map.insert("k".repeat(MAX_VALUE_MAP_KEY_BYTES + 1), Value::Nil);
    assert!(matches!(
        Value::Map(Box::new(map)).inspect_shape(&mut ValueWorkBudget::default()),
        Err(RuntimeError::ValueKeysTooLong { .. })
    ));
}

#[test]
fn value_shape_counts_branching_maps_and_charges_each_walk() {
    let mut value = Value::Map(Box::new(IndexMap::new()));
    for _ in 0..14 {
        let mut map = IndexMap::new();
        map.insert("a".to_string(), value.clone());
        map.insert("b".to_string(), value);
        value = Value::Map(Box::new(map));
    }
    assert!(value.inspect_shape(&mut ValueWorkBudget::default()).is_ok());
    let mut map = IndexMap::new();
    map.insert("a".to_string(), value.clone());
    map.insert("b".to_string(), value);
    assert!(matches!(
        Value::Map(Box::new(map)).inspect_shape(&mut ValueWorkBudget::default()),
        Err(RuntimeError::ValueTooManyItems { .. })
    ));

    let wide = Value::List(vec![Arc::from("x"); 1024]);
    let mut work = ValueWorkBudget::default();
    loop {
        match wide.inspect_shape(&mut work) {
            Ok(shape) => {
                if let Err(error) = work.charge_clone(shape) {
                    assert!(matches!(error, RuntimeError::ValueWorkLimitExceeded { .. }));
                    break;
                }
            }
            Err(error) => {
                assert!(matches!(error, RuntimeError::ValueWorkLimitExceeded { .. }));
                break;
            }
        }
    }
}

#[test]
fn implicit_string_len_matches_owned_coercion_for_map_keys() {
    let values = [
        Value::List(vec![]),
        Value::List(vec![Arc::from("x"); 1000]),
        Value::Map(Box::new(IndexMap::new())),
        Value::Int(i64::MIN),
        Value::Bool(false),
        Value::Nil,
    ];
    for value in values {
        let expected = value.implicit_string_len().expect("value should coerce");
        let actual = value.try_into_string().expect("value should coerce");
        assert_eq!(expected, actual.len());
    }
    assert_eq!(Value::List(vec![]).implicit_string_len().unwrap(), 6);

    let mut shape = ValueShape::map_root();
    shape
        .add_map_entry(MAX_VALUE_MAP_KEY_BYTES - 6, ValueShape::default())
        .unwrap();
    shape
        .add_map_entry(
            Value::List(vec![]).implicit_string_len().unwrap(),
            ValueShape::default(),
        )
        .unwrap();
    assert!(matches!(
        shape.add_map_entry(1, ValueShape::default()),
        Err(RuntimeError::ValueKeysTooLong { .. })
    ));
}
