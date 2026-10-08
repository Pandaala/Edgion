use super::*;

#[tokio::test]
async fn xml_body_admission_exhaustion_fails_closed_before_session_access() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_dsl::value::{BodyValue, MAX_VALUE_WORK_UNITS};
    use edgion_dsl::xml::XmlValue;

    let vm = Vm::new(script_with_constants(vec![]), VmLimits::default()).unwrap();
    for remaining in [0, 15] {
        for id in [BuiltinId::XmlParse, BuiltinId::XmlSetBody] {
            let mut state = VmState::new(0, &vm.limits);
            let argument = if id == BuiltinId::XmlParse {
                Value::Body(BodyValue::Request)
            } else {
                Value::Xml(XmlValue::element("", "r", vec![], vec![], &mut ValueWorkBudget::default()).unwrap())
            };
            state.push(argument, vm.limits.max_stack_depth).unwrap();
            state
                .value_work_budget
                .charge(MAX_VALUE_WORK_UNITS - remaining)
                .unwrap();
            let error = vm
                .call_builtin_xml(id, &mut state, &mut MockPluginSession::new(), &DslRuntime::default())
                .await
                .unwrap_err();
            assert!(matches!(error, RuntimeError::BodyAccess { .. }), "{id:?}: {error:?}");
        }
        let mut state = VmState::new(0, &vm.limits);
        state
            .push(Value::Str(Arc::from("<r/>")), vm.limits.max_stack_depth)
            .unwrap();
        state
            .value_work_budget
            .charge(MAX_VALUE_WORK_UNITS - remaining)
            .unwrap();
        assert!(matches!(
            vm.call_builtin_xml(
                BuiltinId::XmlParse,
                &mut state,
                &mut MockPluginSession::new(),
                &DslRuntime::default()
            )
            .await,
            Err(RuntimeError::ValueWorkLimitExceeded { .. })
        ));
    }
}

/// Build a minimal CompiledScript with the given constants and no real code.
fn script_with_constants(constants: Vec<Constant>) -> CompiledScript {
    CompiledScript {
        version: super::super::bytecode::BYTECODE_VERSION,
        code: vec![OpCode::ReturnNext],
        constants,
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    }
}

// -----------------------------------------------------------------------
// Group C — const_str_cache alignment invariant
// -----------------------------------------------------------------------

/// Empty constant pool: cache length is 0, no panics.
#[test]
fn test_const_str_cache_empty_pool() {
    let script = script_with_constants(vec![]);
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    assert_eq!(vm.const_str_cache_ref().len(), 0);
}

/// Mixed pool: cache is parallel to constants; only Str slots are Some.
#[test]
fn test_const_str_cache_alignment() {
    let constants = vec![
        Constant::Str("hello".to_string()),
        Constant::Int(42),
        Constant::Str("world".to_string()),
    ];
    let script = script_with_constants(constants.clone());
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let cache = vm.const_str_cache_ref();

    // Length must match the constant pool exactly.
    assert_eq!(
        cache.len(),
        constants.len(),
        "cache length must equal constant pool size"
    );

    // Index 0: Str("hello") → Some("hello")
    assert!(cache[0].is_some(), "Str constant at index 0 should be Some");
    assert_eq!(
        cache[0].as_deref(),
        Some("hello"),
        "Str constant content mismatch at index 0"
    );

    // Index 1: Int(42) → None
    assert!(cache[1].is_none(), "Int constant at index 1 should be None");

    // Index 2: Str("world") → Some("world")
    assert!(cache[2].is_some(), "Str constant at index 2 should be Some");
    assert_eq!(
        cache[2].as_deref(),
        Some("world"),
        "Str constant content mismatch at index 2"
    );
}

/// Regex constants must produce None in the str cache and Some in regex_cache.
#[test]
fn test_const_str_cache_regex_is_none() {
    let constants = vec![
        Constant::Str("prefix".to_string()),
        Constant::Regex("\\d+".to_string()),
        Constant::Int(0),
        Constant::Str("suffix".to_string()),
    ];
    let script = script_with_constants(constants.clone());
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let cache = vm.const_str_cache_ref();

    assert_eq!(cache.len(), constants.len());

    // Str slots
    assert!(cache[0].is_some());
    assert_eq!(cache[0].as_deref(), Some("prefix"));
    assert!(cache[3].is_some());
    assert_eq!(cache[3].as_deref(), Some("suffix"));

    // Regex slot → None in str cache
    assert!(cache[1].is_none(), "Regex constant should be None in const_str_cache");

    // Int slot → None
    assert!(cache[2].is_none(), "Int constant should be None in const_str_cache");
}

/// All-Str pool: every slot is Some with the correct content.
#[test]
fn test_const_str_cache_all_str() {
    let strings = ["alpha", "beta", "gamma", "delta"];
    let constants: Vec<Constant> = strings.iter().map(|s| Constant::Str(s.to_string())).collect();
    let script = script_with_constants(constants);
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let cache = vm.const_str_cache_ref();

    assert_eq!(cache.len(), strings.len());
    for (i, expected) in strings.iter().enumerate() {
        assert!(cache[i].is_some(), "index {} should be Some", i);
        assert_eq!(cache[i].as_deref(), Some(*expected), "content mismatch at index {}", i);
    }
}

/// All-Int pool: every slot is None.
#[test]
fn test_const_str_cache_all_int() {
    let constants = vec![Constant::Int(1), Constant::Int(2), Constant::Int(3)];
    let script = script_with_constants(constants.clone());
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let cache = vm.const_str_cache_ref();

    assert_eq!(cache.len(), constants.len());
    for (i, entry) in cache.iter().enumerate() {
        assert!(entry.is_none(), "Int constant at index {} should be None", i);
    }
}

/// Arc content equality: two Vm instances built from the same script
/// must produce independently-owned but content-equal Arcs.
#[test]
fn test_const_str_cache_arc_content_equality() {
    let constants = vec![Constant::Str("shared".to_string())];
    let script1 = script_with_constants(constants.clone());
    let script2 = script_with_constants(constants);
    let vm1 = Vm::new(script1, VmLimits::default()).expect("vm1 creation failed");
    let vm2 = Vm::new(script2, VmLimits::default()).expect("vm2 creation failed");

    let arc1 = vm1.const_str_cache_ref()[0].as_ref().unwrap();
    let arc2 = vm2.const_str_cache_ref()[0].as_ref().unwrap();

    // Content must match even though the Arcs are independent allocations.
    assert_eq!(arc1.as_ref(), arc2.as_ref(), "Arc content should be equal");
    // Pointer identity is NOT asserted — they are independent Vms.
}
