use super::*;

#[test]
fn accepts_exact_limit_and_rejects_limit_plus_one() {
    let mut exact = CappedWriter::new(3);
    exact.write_all(b"abc").unwrap();
    assert_eq!(exact.into_inner(), b"abc");

    let mut over = CappedWriter::new(3);
    assert!(over.write_all(b"abcd").is_err());
    assert!(over.overflowed());
    assert!(over.into_inner().is_empty());
}
