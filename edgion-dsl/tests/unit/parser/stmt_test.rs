use super::super::super::ast::*;
use super::super::parse_program;

#[test]
fn test_parse_let_simple() {
    let prog = parse_program(r#"let x = 42"#).unwrap();
    assert_eq!(prog.stmts.len(), 1);
    match &prog.stmts[0].kind {
        StmtKind::Let { name, mutable, value } => {
            assert_eq!(name, "x");
            assert!(!mutable);
            assert!(matches!(value.kind, ExprKind::IntLit(42)));
        }
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_let_mut() {
    let prog = parse_program(r#"let mut count = 0"#).unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { mutable, .. } => assert!(mutable),
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_if_simple() {
    let prog = parse_program(
        r#"
            if x == nil {
                return deny(403, "blocked")
            }
        "#,
    )
    .unwrap();
    assert_eq!(prog.stmts.len(), 1);
    match &prog.stmts[0].kind {
        StmtKind::If { branches, else_body } => {
            assert_eq!(branches.len(), 1);
            assert!(else_body.is_none());
        }
        _ => panic!("expected If"),
    }
}

#[test]
fn test_parse_if_else() {
    let prog = parse_program(
        r#"
            if x == 1 {
                let a = 1
            } else {
                let b = 2
            }
        "#,
    )
    .unwrap();
    match &prog.stmts[0].kind {
        StmtKind::If { branches, else_body } => {
            assert_eq!(branches.len(), 1);
            assert!(else_body.is_some());
        }
        _ => panic!("expected If"),
    }
}

#[test]
fn test_parse_for_range() {
    let prog = parse_program(
        r#"
            for i in range(0, 5) {
                ctx.set("key", "value")
            }
        "#,
    )
    .unwrap();
    match &prog.stmts[0].kind {
        StmtKind::ForRange {
            var_name,
            start,
            end,
            body,
        } => {
            assert_eq!(var_name, "i");
            assert!(matches!(start.kind, ExprKind::IntLit(0)));
            assert!(matches!(end.kind, ExprKind::IntLit(5)));
            assert_eq!(body.len(), 1);
        }
        _ => panic!("expected ForRange"),
    }
}

#[test]
fn test_parse_while() {
    let prog = parse_program(
        r#"
            while x < 10 {
                x = x + 1
            }
        "#,
    )
    .unwrap();
    assert_eq!(prog.stmts.len(), 1);
    assert!(matches!(prog.stmts[0].kind, StmtKind::While { .. }));
}

#[test]
fn test_parse_return_next() {
    let prog = parse_program("return next()").unwrap();
    assert!(matches!(prog.stmts[0].kind, StmtKind::ReturnNext));
}

#[test]
fn test_parse_return_deny() {
    let prog = parse_program(r#"return deny(403, "forbidden")"#).unwrap();
    match &prog.stmts[0].kind {
        StmtKind::ReturnDeny { status, body } => {
            assert!(matches!(status.kind, ExprKind::IntLit(403)));
            assert!(matches!(body.kind, ExprKind::StringLit(ref s) if s == "forbidden"));
        }
        _ => panic!("expected ReturnDeny"),
    }
}

#[test]
fn test_parse_complex_script() {
    let prog = parse_program(
        r#"
            let ip = req.header("X-Real-IP")
            if ip == nil {
                return deny(403, "Missing IP")
            } else if ip.starts_with("10.") {
                log("Internal request from " + ip)
            } else {
                req.set_header("X-Forwarded-For", ip)
            }
        "#,
    )
    .unwrap();
    assert_eq!(prog.stmts.len(), 2); // let + if
}

#[test]
fn test_parse_for_in() {
    let prog = parse_program(
        r#"
            for name in req.header_names() {
                log(name)
            }
        "#,
    )
    .unwrap();
    match &prog.stmts[0].kind {
        StmtKind::ForIn {
            var_name,
            iterable: _,
            body,
        } => {
            assert_eq!(var_name, "name");
            assert_eq!(body.len(), 1);
        }
        _ => panic!("expected ForIn"),
    }
}

#[test]
fn test_parse_assignment() {
    let prog = parse_program(
        r#"
            let mut x = 0
            x = x + 1
        "#,
    )
    .unwrap();
    assert_eq!(prog.stmts.len(), 2);
    assert!(matches!(prog.stmts[1].kind, StmtKind::Assign { .. }));
}
