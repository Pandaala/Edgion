use super::*;
#[test]
fn public_page_defaults_and_bounds_are_explicit() {
    assert_eq!(
        PaginationQuery::default().into_list_options().unwrap().unwrap().limit,
        10
    );
    for limit in [1, 10, 20, 50, 100] {
        assert_eq!(
            PaginationQuery {
                limit: Some(limit),
                continue_token: None
            }
            .into_list_options()
            .unwrap()
            .unwrap()
            .limit,
            limit
        );
    }
    for limit in [0, 101, u32::MAX] {
        assert_eq!(
            PaginationQuery {
                limit: Some(limit),
                continue_token: None
            }
            .into_list_options()
            .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }
    for token in [String::new(), "x".repeat(8193)] {
        assert!(PaginationQuery {
            limit: None,
            continue_token: Some(token)
        }
        .into_list_options()
        .is_err());
    }
    for value in [
        serde_json::json!({"offset":1}),
        serde_json::json!({"filter":"x"}),
        serde_json::json!({"limit":-1}),
        serde_json::json!({"limit":"ten"}),
    ] {
        assert!(serde_json::from_value::<PaginationQuery>(value).is_err());
    }
}
