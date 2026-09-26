use super::super::super::ast::*;
use super::super::super::bytecode::OpCode;
use super::super::super::compiler::Compiler;
use super::super::parse_program;
use super::super::ParseCtx;

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

#[test]
fn test_expression_nesting_boundary() {
    let cases = [
        ("!".repeat(64) + "true", "!".repeat(65) + "true"),
        (
            format!("{}1{}", "len(".repeat(64), ")".repeat(64)),
            format!("{}1{}", "len(".repeat(65), ")".repeat(65)),
        ),
        (
            format!("{}m{}", "m[".repeat(64), "]".repeat(64)),
            format!("{}m{}", "m[".repeat(65), "]".repeat(65)),
        ),
    ];

    for (allowed, rejected) in cases {
        parse_program(&format!("let x = {allowed}")).unwrap();
        let error = parse_program(&format!("let x = {rejected}")).unwrap_err();
        assert!(error.to_string().contains("nesting too deep"), "{error}");
    }
}

#[test]
fn test_mixed_expression_nesting_counts_statement_depth() {
    let source = |index_depth| {
        format!(
            "if true {{ let x = {}{}{}m{}{} }}",
            "!".repeat(21),
            "len(".repeat(21),
            "m[".repeat(index_depth),
            "]".repeat(index_depth),
            ")".repeat(21),
        )
    };

    parse_program(&source(21)).unwrap();
    let error = parse_program(&source(22)).unwrap_err();
    assert!(error.to_string().contains("nesting too deep"), "{error}");
}

#[test]
fn test_expression_nesting_depth_restored() {
    for source in ["f(1)", "!true", "m[1]", "(1)", "{}"] {
        let mut ctx = ParseCtx::new(source);
        ctx.nesting_depth = 2;
        assert!(super::parse_expr(source, &mut ctx).is_ok(), "{source}");
        assert_eq!(ctx.nesting_depth, 2, "{source}");
    }

    for source in ["f(?)", "!?", "m[?]", "(1", "{\"x\": ?}"] {
        let mut ctx = ParseCtx::new(source);
        ctx.nesting_depth = 2;
        assert!(
            matches!(super::parse_expr(source, &mut ctx), Err(nom::Err::Failure(_))),
            "{source}"
        );
        assert_eq!(ctx.nesting_depth, 2, "{source}");
    }

    let source = "f(1)";
    let mut ctx = ParseCtx::new(source);
    ctx.nesting_depth = ctx.max_nesting_depth;
    assert!(matches!(super::parse_expr(source, &mut ctx), Err(nom::Err::Failure(_))));
    assert_eq!(ctx.nesting_depth, ctx.max_nesting_depth);
}

#[test]
fn test_valid_unary_ast_and_bytecode() {
    let source = "let x = !-1";
    let program = parse_program(source).unwrap();
    let StmtKind::Let { value, .. } = &program.stmts[0].kind else {
        panic!("expected Let");
    };
    let ExprKind::UnaryOp {
        op: UnaryOp::Not,
        operand,
    } = &value.kind
    else {
        panic!("expected Not");
    };
    assert!(matches!(operand.kind, ExprKind::UnaryOp { op: UnaryOp::Neg, .. }));

    let script = Compiler::new().compile_with_source(&program, source).unwrap();
    assert!(script
        .code
        .windows(3)
        .any(|ops| matches!(ops, [OpCode::LoadConst(_), OpCode::Neg, OpCode::Not])));
}
