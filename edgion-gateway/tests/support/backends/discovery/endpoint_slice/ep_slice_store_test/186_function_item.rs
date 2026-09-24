#[cfg(test)]
pub fn extract_service_key(slice: &EndpointSlice) -> String {
    super::super::coordinator::endpoint_slice_service_key(slice)
}
