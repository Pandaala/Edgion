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

#[test]
fn remaining_limit_tracks_accepted_bytes_only() {
    for limit in [0, 3, usize::MAX] {
        let mut writer = CappedWriter::new(limit);
        assert_eq!(writer.remaining_limit(), limit);
        writer.write_all(b"").unwrap();
        assert_eq!(writer.remaining_limit(), limit);
        if limit != 0 {
            writer.write_all(b"a").unwrap();
            assert_eq!(writer.remaining_limit(), limit - 1);
        }
    }
    let mut writer = CappedWriter::new(3);
    writer.write_all(b"a").unwrap();
    assert!(writer.write_all(b"bcd").is_err());
    assert_eq!(writer.remaining_limit(), 2);
    assert!(writer.overflowed());
    writer.write_all(b"bc").unwrap();
    assert_eq!(writer.remaining_limit(), 0);
    assert!(writer.write_all(b"d").is_err());
    assert_eq!(writer.remaining_limit(), 0);
    assert_eq!(writer.into_inner(), b"abc");
}
