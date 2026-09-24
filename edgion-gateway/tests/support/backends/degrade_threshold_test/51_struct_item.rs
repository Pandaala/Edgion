#[cfg(test)]
struct DegradeTestState {
    default: u8,
    overrides: Vec<(String, u8)>,
}
