use super::is_valid_final_http_status;

#[test]
fn ordinary_final_status_range_excludes_informational_and_invalid_values() {
    for status in [100, 101, 102, 103, 199, 600, 700] {
        assert!(!is_valid_final_http_status(status), "{status}");
    }
    for status in [200, 204, 399, 499, 599] {
        assert!(is_valid_final_http_status(status), "{status}");
    }
}
