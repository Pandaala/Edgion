use super::*;

#[test]
fn exact_byte_boundary_counts_json_escaping() {
    assert_eq!(serialize(&"abc", 5, true).unwrap().bytes.unwrap(), b"\"abc\"");
    assert!(matches!(serialize(&"abc", 4, true), Err(StatusCode::PAYLOAD_TOO_LARGE)));
    assert_eq!(serialize(&"\n", 4, false).unwrap().length, 4);
    assert!(matches!(serialize(&"\n", 3, false), Err(StatusCode::PAYLOAD_TOO_LARGE)));
    let measured = serialize(&vec!["data"; 100], 1000, false).unwrap();
    assert!(measured.bytes.is_none());
}

#[tokio::test]
async fn router_returns_fixed_error_without_partial_json() {
    use axum::{routing::get, Router};
    use tower::ServiceExt;
    // Fits the source byte budget but escaped JSON exceeds the response budget.
    let source = "\n".repeat(6 * 1024 * 1024);
    let router = Router::new().route(
        "/resource",
        get(move || {
            let source = source.clone();
            async { BoundedJson(source) }
        }),
    );
    let response = router
        .oneshot(
            http::Request::builder()
                .uri("/resource")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = axum::body::to_bytes(response.into_body(), 100).await.unwrap();
    assert_eq!(body.as_ref(), b"JSON response exceeds limit");
}

#[tokio::test]
async fn serializer_error_does_not_expose_partial_output_or_error() {
    struct Broken;
    impl Serialize for Broken {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("sensitive serializer detail"))
        }
    }
    let response = BoundedJson(Broken).into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(response.into_body(), 100).await.unwrap();
    assert_eq!(body.as_ref(), b"JSON response serialization failed");
}

#[test]
fn shared_projection_budget_bounds_rows_and_bytes_before_retention() {
    let mut budget = ProjectionBudget::new();
    for _ in 0..MAX_PROJECTION_ROWS {
        budget.reserve(&0).unwrap();
    }
    assert_eq!(budget.reserve(&0), Err(StatusCode::PAYLOAD_TOO_LARGE));
    let mut budget = ProjectionBudget::new();
    let row = "x".repeat(MAX_PROJECTION_BYTES / 2);
    budget.reserve(&row).unwrap();
    assert_eq!(budget.reserve(&row), Err(StatusCode::PAYLOAD_TOO_LARGE));
}

#[test]
fn full_envelope_preflight_combines_separately_valid_parts() {
    #[derive(Serialize)]
    struct Envelope<'a> {
        success: bool,
        data: (&'a str, &'a str),
    }
    let field = "x".repeat(6 * 1024 * 1024);
    check(&field).unwrap();
    assert_eq!(
        check(&Envelope {
            success: true,
            data: (&field, &field)
        }),
        Err(StatusCode::PAYLOAD_TOO_LARGE)
    );
}
