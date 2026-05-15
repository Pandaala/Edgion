//! Tests for range(), trim(), split() and their combinations.
//! Also covers Add opcode mixed-type coercion and Arc alias correctness for
//! string constants loaded multiple times.

use super::*;

// ---------------------------------------------------------------------------
// Helper: assert that a DSL source produces `X-Out: expected`
// ---------------------------------------------------------------------------

fn assert_output(source: &str, expected: &str) {
    let expected = expected.to_string();
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(move |h, v| h == "X-Out" && v == expected)
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

// ---------------------------------------------------------------------------
// Group A — Add opcode: mixed-type string concatenation branches
// ---------------------------------------------------------------------------

// Str + Str — explicit baseline
#[test]
fn test_add_str_str() {
    assert_output(r#"req.set_header("X-Out", "hello" + " world")"#, "hello world");
}

// Str + Int — Int coerces to decimal string
#[test]
fn test_add_str_int() {
    assert_output(r#"req.set_header("X-Out", "val=" + 42)"#, "val=42");
}

// Int + Str — Int on the left coerces to string
#[test]
fn test_add_int_str() {
    assert_output(r#"req.set_header("X-Out", 7 + " items")"#, "7 items");
}

// Str + Bool(true) — Bool coerces to "true"
#[test]
fn test_add_str_bool_true() {
    assert_output(r#"req.set_header("X-Out", "flag=" + true)"#, "flag=true");
}

// Str + Bool(false) — Bool coerces to "false"
#[test]
fn test_add_str_bool_false() {
    assert_output(r#"req.set_header("X-Out", "flag=" + false)"#, "flag=false");
}

// Str + Nil — Nil coerces to "nil"
#[test]
fn test_add_str_nil() {
    assert_output(r#"req.set_header("X-Out", "v=" + nil)"#, "v=nil");
}

// Bool + Bool — neither operand is Str or Int → TypeError → ErrResponse 500
#[test]
fn test_add_bool_bool_type_error() {
    let session = MockPluginSession::new();
    let result = run(r#"let x = true + false"#, session);
    assert!(
        matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }),
        "Bool + Bool should produce a type error (got {:?})",
        result
    );
}

// ---------------------------------------------------------------------------
// Group B — Same string constant loaded multiple times: Arc alias correctness
// ---------------------------------------------------------------------------

// Same literal used in two separate req.set_header calls — both see the right value.
#[test]
fn test_same_constant_two_headers() {
    let source = r#"
        let s = "shared"
        req.set_header("X-A", s)
        req.set_header("X-B", s)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-A" && v == "shared")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-B" && v == "shared")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

// Same literal used in two Add operations — results are independent strings.
#[test]
fn test_same_constant_two_add_ops() {
    // "pre" constant loaded twice; each concatenation must produce an independent value.
    let source = r#"
        let prefix = "pre"
        let a = prefix + "-A"
        let b = prefix + "-B"
        req.set_header("X-A", a)
        req.set_header("X-B", b)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-A" && v == "pre-A")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-B" && v == "pre-B")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

// Same literal compared with == — Arc compares by content, must be true.
#[test]
fn test_same_constant_equality() {
    let source = r#"
        let a = "same"
        let b = "same"
        if a == b {
            req.set_header("X-Out", "equal")
        }
    "#;
    assert_output(source, "equal");
}

// Same constant used three times in a loop — each iteration reads a fresh Arc clone.
#[test]
fn test_same_constant_multiple_loads() {
    // Assign the same literal to three locals and concatenate them.
    // This exercises Arc::clone being called 3× on the same const_str_cache slot.
    let source = r#"
        let x = "abc"
        let y = "abc"
        let z = "abc"
        req.set_header("X-Out", x + y + z)
    "#;
    assert_output(source, "abcabcabc");
}

#[test]
fn test_range_as_expression() {
    let source = r#"
        let r = range(0, 3)
        let total = len(r)
        req.set_header("X-Total", to_str(total))
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Total" && v == "3")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_range_empty() {
    let source = r#"
        let r = range(5, 3)
        req.set_header("X-Len", to_str(len(r)))
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Len" && v == "0")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_range_iterate() {
    let source = r#"
        let r = range(1, 4)
        let mut sum = 0
        for item in r {
            sum = sum + to_int(item)
        }
        req.set_header("X-Sum", to_str(sum))
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Sum" && v == "6")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_trim() {
    let source = r#"
        let s = trim("  hello  ")
        req.set_header("X-Trimmed", s)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Trimmed" && v == "hello")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_trim_empty() {
    let source = r#"
        let s = trim("")
        req.set_header("X-Empty", s)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Empty" && v.is_empty())
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_split_basic() {
    let source = r#"
        let parts = split("a,b,c", ",")
        req.set_header("X-Count", to_str(len(parts)))
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Count" && v == "3")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_split_iterate() {
    let source = r#"
        let parts = split("admin,user,guest", ",")
        let mut found = "no"
        for role in parts {
            if role == "admin" {
                found = "yes"
                break
            }
        }
        req.set_header("X-Found", found)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Found" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_split_no_delimiter_found() {
    let source = r#"
        let parts = split("hello", ",")
        req.set_header("X-Count", to_str(len(parts)))
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Count" && v == "1")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

// Direct index access on a list — exercises the IndexGet Arc::clone path.
#[test]
fn test_split_index_get() {
    let source = r#"
        let parts = split("x:y:z", ":")
        req.set_header("X-Zero", parts[0])
        req.set_header("X-One", parts[1])
        req.set_header("X-Two", parts[2])
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Zero" && v == "x")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-One" && v == "y")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Two" && v == "z")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

// Out-of-bounds index returns Nil, not panic.
#[test]
fn test_list_index_oob_returns_nil() {
    let source = r#"
        let parts = split("a:b", ":")
        let v = parts[99]
        if v == nil {
            req.set_header("X-Out", "nil")
        }
    "#;
    assert_output(source, "nil");
}

// join() reassembles a split list correctly.
#[test]
fn test_join_basic() {
    let source = r#"
        let parts = split("a,b,c", ",")
        let joined = join(parts, "-")
        req.set_header("X-Out", joined)
    "#;
    assert_output(source, "a-b-c");
}

// range() items are accessible by direct index.
#[test]
fn test_range_index_get() {
    let source = r#"
        let r = range(10, 13)
        req.set_header("X-Out", r[0] + "-" + r[2])
    "#;
    assert_output(source, "10-12");
}

#[test]
fn test_trim_split_combo() {
    let source = r#"
        let roles = "admin, user, guest"
        let parts = split(roles, ",")
        let mut found = "no"
        for role in parts {
            let trimmed = trim(role)
            if trimmed == "user" {
                found = "yes"
                break
            }
        }
        req.set_header("X-Found", found)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Found" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}
