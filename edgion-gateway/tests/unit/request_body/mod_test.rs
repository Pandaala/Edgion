use super::EMPTY_SHA256;
use sha2::Digest;

#[test]
fn empty_sha256_constant_matches_the_algorithm() {
    let computed: [u8; 32] = sha2::Sha256::digest([]).into();
    assert_eq!(EMPTY_SHA256, computed);
}
