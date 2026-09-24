use super::{checked_value_opt_str_ref, RuntimeError, Value};
use crate::test_allocation_counter::count_allocations;
use crate::EdgionHttpContext;

#[test]
fn present_value_allocates_only_the_vm_string_and_survives_context_mutation() {
    let mut ctx = EdgionHttpContext::new();
    ctx.set_ctx_var("key".into(), "original".into());

    let (value, allocations) = count_allocations(|| checked_value_opt_str_ref(ctx.get_ctx_var("key"), 8));
    let Value::Str(value) = value.unwrap() else {
        panic!("present context value must produce a DSL string");
    };
    assert_eq!(allocations, 1);

    ctx.set_ctx_var("key".into(), "replacement".into());
    assert_eq!(ctx.get_ctx_var("key"), Some("replacement"));
    assert_eq!(&*value, "original");
    assert_eq!(ctx.remove_ctx_var("key").as_deref(), Some("replacement"));
    assert_eq!(ctx.get_ctx_var("key"), None);
    drop(ctx);
    assert_eq!(&*value, "original");
}

#[test]
fn missing_value_returns_nil_without_allocating_plugin_state() {
    let ctx = EdgionHttpContext::new();
    let (value, allocations) = count_allocations(|| checked_value_opt_str_ref(ctx.get_ctx_var("missing"), 0));

    assert!(matches!(value, Ok(Value::Nil)));
    assert_eq!(allocations, 0);
    assert!(!ctx.has_plugin_aux());
}

#[test]
fn empty_value_is_a_string_at_zero_limit() {
    assert!(matches!(checked_value_opt_str_ref(Some(""), 0), Ok(Value::Str(s)) if s.is_empty()));
}

#[test]
fn utf8_limit_is_checked_in_bytes_before_allocating() {
    let mut ctx = EdgionHttpContext::new();
    ctx.set_ctx_var("key".into(), "é€".into());

    let (value, allocations) = count_allocations(|| checked_value_opt_str_ref(ctx.get_ctx_var("key"), 5));
    assert!(matches!(value, Ok(Value::Str(s)) if &*s == "é€"));
    assert_eq!(allocations, 1);

    let (value, allocations) = count_allocations(|| checked_value_opt_str_ref(ctx.get_ctx_var("key"), 4));
    assert!(matches!(value, Err(RuntimeError::StringTooLong { len: 5, limit: 4 })));
    assert_eq!(allocations, 0);
}
