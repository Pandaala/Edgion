use super::*;

#[test]
fn route_hostname_lists_are_all_or_nothing() {
    for required in [false, true] {
        assert_eq!(validate_route_hostnames(None, required).is_ok(), !required);
        assert_eq!(validate_route_hostnames(Some(&[]), required).is_ok(), !required);
        assert!(validate_route_hostnames(Some(&vec!["a".into(); 16]), required).is_ok());
        assert_eq!(
            validate_route_hostnames(Some(&vec!["a".into(); 17]), required).is_ok(),
            required
        );
        assert!(validate_route_hostnames(Some(&vec!["a".into(); 1025]), required).is_err());
        assert!(validate_route_hostnames(Some(&["a".into(), "*".into()]), required).is_err());
    }
}
