#[cfg(test)]
fn body_bytes_to_value(bytes: &[u8], limit: usize) -> Result<Value, RuntimeError> {
    checked_value_str(String::from_utf8_lossy(bytes).into_owned(), limit)
}
