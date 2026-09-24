/// Pure test oracle for `select_gateway_certificate` ordering: given each
/// compiled certificate identity's key-type classification (`Some(true)` =
/// ECDSA, `Some(false)` = RSA/other, `None` = unknown) and whether ECDSA is
/// preferred, return the selected index. Prefers ECDSA when requested and
/// available; otherwise the first RSA/other certificate; otherwise the first
/// certificate. Returns `None` only for an empty list.
#[cfg(all(test, feature = "boringssl"))]
fn pick_secret_index(kinds: &[Option<bool>], prefer_ecdsa: bool) -> Option<usize> {
    if kinds.is_empty() {
        return None;
    }
    if prefer_ecdsa {
        if let Some(i) = kinds.iter().position(|k| *k == Some(true)) {
            return Some(i);
        }
    }
    Some(kinds.iter().position(|k| *k == Some(false)).unwrap_or(0))
}
