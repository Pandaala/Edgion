/// Replace the matched number of complete path elements with a literal prefix.
///
/// Gateway API prefix replacement ignores a trailing slash in the configured
/// match. The route pattern may also contain parameters whose byte length does
/// not match the request value, so the suffix must be found by element count
/// rather than by slicing at the pattern's byte length.
pub(super) fn replace_prefix_match(request_path: &str, matched_prefix: &str, replacement: &str) -> String {
    let matched_elements = matched_prefix.split('/').filter(|element| !element.is_empty()).count();
    let suffix = suffix_after_elements(request_path, matched_elements).unwrap_or("");

    if suffix.is_empty() {
        let mut result = String::with_capacity(replacement.len().max(1));
        if replacement.is_empty() {
            result.push('/');
        } else {
            result.push_str(replacement);
        }
        return result;
    }

    let replacement = replacement.trim_end_matches('/');
    let suffix = suffix.trim_start_matches('/');
    let mut result = String::with_capacity(replacement.len() + suffix.len() + 1);
    result.push_str(replacement);
    result.push('/');
    result.push_str(suffix);
    result
}

fn suffix_after_elements(path: &str, element_count: usize) -> Option<&str> {
    if element_count == 0 {
        return Some(if path == "/" { "" } else { path });
    }

    let mut consumed = 0;
    let mut in_element = false;
    for (index, byte) in path.bytes().enumerate() {
        if byte == b'/' {
            if in_element {
                consumed += 1;
                if consumed == element_count {
                    return Some(&path[index..]);
                }
                in_element = false;
            }
        } else {
            in_element = true;
        }
    }

    if in_element {
        consumed += 1;
    }
    (consumed == element_count).then_some("")
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/gateway_api_filters/path_modifier_test.rs"]
mod tests;
