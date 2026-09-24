use super::super::super::ast::*;
use super::super::parse_program;

#[test]
fn test_parse_string_literal() {
    let prog = parse_program(r#"let s = "hello world""#).unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { value, .. } => match &value.kind {
            ExprKind::StringLit(s) => assert_eq!(s, "hello world"),
            _ => panic!("expected StringLit"),
        },
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_method_call() {
    let prog = parse_program(r#"let ip = req.header("X-Real-IP")"#).unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { value, .. } => match &value.kind {
            ExprKind::MethodCall { object, method, args } => {
                assert!(matches!(object.kind, ExprKind::Ident(ref s) if s == "req"));
                assert_eq!(method, "header");
                assert_eq!(args.len(), 1);
            }
            _ => panic!("expected MethodCall"),
        },
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_binary_ops() {
    let prog = parse_program("let x = 1 + 2 * 3").unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { value, .. } => {
            // Should be 1 + (2 * 3) due to precedence
            assert!(matches!(value.kind, ExprKind::BinaryOp { op: BinOp::Add, .. }));
        }
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_logical_ops() {
    let prog = parse_program("let x = a && b || c").unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { value, .. } => {
            // Should be (a && b) || c due to precedence
            assert!(matches!(value.kind, ExprKind::BinaryOp { op: BinOp::Or, .. }));
        }
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_unary_not() {
    let prog = parse_program("let x = !true").unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { value, .. } => {
            assert!(matches!(value.kind, ExprKind::UnaryOp { op: UnaryOp::Not, .. }));
        }
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_fn_call() {
    let prog = parse_program(r#"log("hello")"#).unwrap();
    match &prog.stmts[0].kind {
        StmtKind::ExprStmt { expr } => match &expr.kind {
            ExprKind::FnCall { name, args } => {
                assert_eq!(name, "log");
                assert_eq!(args.len(), 1);
            }
            _ => panic!("expected FnCall"),
        },
        _ => panic!("expected ExprStmt"),
    }
}

#[test]
fn test_parse_map_literal() {
    let prog = parse_program(r#"let m = {"a": "1", "b": "2"}"#).unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { value, .. } => match &value.kind {
            ExprKind::MapLiteral(pairs) => assert_eq!(pairs.len(), 2),
            _ => panic!("expected MapLiteral"),
        },
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_empty_map() {
    let prog = parse_program(r#"let m = {}"#).unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { value, .. } => match &value.kind {
            ExprKind::MapLiteral(pairs) => assert_eq!(pairs.len(), 0),
            _ => panic!("expected MapLiteral"),
        },
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_index_access() {
    let prog = parse_program(r#"let v = m["key"]"#).unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { value, .. } => match &value.kind {
            ExprKind::Index { .. } => {}
            _ => panic!("expected Index"),
        },
        _ => panic!("expected Let"),
    }
}

#[test]
fn test_parse_chained_index() {
    let prog = parse_program(r#"let v = resp["headers"]["x-token"]"#).unwrap();
    match &prog.stmts[0].kind {
        StmtKind::Let { value, .. } => match &value.kind {
            ExprKind::Index { object, .. } => match &object.kind {
                ExprKind::Index { .. } => {}
                _ => panic!("expected nested Index"),
            },
            _ => panic!("expected Index"),
        },
        _ => panic!("expected Let"),
    }
}
