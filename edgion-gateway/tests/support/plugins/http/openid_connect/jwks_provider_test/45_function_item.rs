#[cfg(test)]
pub fn no_op_validator<D: DiscoveryDoc>() -> DiscoveryValidator<D> {
    Arc::new(|_| Ok(()))
}
