use super::super::ast::*;
use super::parse_program;

#[test]
fn test_parse_comments() {
    let prog = parse_program(
        r#"
            // This is a comment
            let x = 42
            // Another comment
            let y = 10
        "#,
    )
    .unwrap();
    assert_eq!(prog.stmts.len(), 2);
}

#[test]
fn test_source_too_large() {
    let source = format!("nil{}", " ".repeat(super::MAX_SOURCE_LEN - 3));
    assert_eq!(source.len(), super::MAX_SOURCE_LEN);
    assert_eq!(parse_program(&source).unwrap().stmts.len(), 1);

    let oversized = format!("{source} ");
    let err = parse_program(&oversized).unwrap_err();
    assert_eq!(
        err.message,
        format!(
            "source too large: {} bytes (max {})",
            oversized.len(),
            super::MAX_SOURCE_LEN
        )
    );
}

#[test]
fn test_ast_node_limit() {
    // Each expression statement contributes one statement and one literal node.
    let source = "nil\n".repeat(1_000);
    assert_eq!(parse_program(&source).unwrap().stmts.len(), 1_000);

    let oversized = "nil\n".repeat(1_001);
    let err = parse_program(&oversized).unwrap_err();
    assert_eq!(err.message, "too many AST nodes: 2001 (max 2000)");
}

#[test]
fn test_span_line_numbers() {
    let prog = parse_program("let x = 1\nlet y = 2\n").unwrap();
    assert_eq!(prog.stmts[0].span.unwrap().line, 1);
    assert_eq!(prog.stmts[1].span.unwrap().line, 2);
}

#[test]
fn test_span_binary_op_position() {
    // The BinaryOp span should point to the operator token.
    let prog = parse_program("2 / 0").unwrap();
    match &prog.stmts[0].kind {
        StmtKind::ExprStmt { expr } => match &expr.kind {
            ExprKind::BinaryOp { op: BinOp::Div, .. } => {
                assert_eq!(expr.span.unwrap().line, 1);
                assert_eq!(expr.span.unwrap().offset, 2); // '/' is at byte offset 2
            }
            _ => panic!("expected BinaryOp Div"),
        },
        _ => panic!("expected ExprStmt"),
    }
}
