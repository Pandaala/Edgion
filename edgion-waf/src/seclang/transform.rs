use sha1::{Digest, Sha1};
use thiserror::Error;

use super::TransformationKind;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransformError {
    #[error("transformation output exceeds the configured byte limit of {limit}")]
    OutputLimit { limit: usize },
    #[error("transformation work exceeds the configured byte limit of {limit}")]
    WorkLimit { limit: usize },
    #[error("cannot allocate bounded transformation output")]
    Allocation,
    #[error("transformation accounting arithmetic overflowed")]
    Arithmetic,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TransformLimits {
    pub max_output_bytes: usize,
    pub max_work_bytes: usize,
}

pub(super) fn apply_transformations(
    input: &[u8],
    transformations: &[TransformationKind],
    limits: TransformLimits,
) -> Result<Vec<u8>, TransformError> {
    if input.len() > limits.max_output_bytes {
        return Err(TransformError::OutputLimit {
            limit: limits.max_output_bytes,
        });
    }
    let mut current = Vec::new();
    current
        .try_reserve_exact(input.len())
        .map_err(|_| TransformError::Allocation)?;
    current.extend_from_slice(input);
    let mut output = Vec::new();
    let mut work = 0_usize;
    for transformation in transformations {
        work = work.checked_add(current.len()).ok_or(TransformError::Arithmetic)?;
        if work > limits.max_work_bytes {
            return Err(TransformError::WorkLimit {
                limit: limits.max_work_bytes,
            });
        }
        output.clear();
        apply_one(*transformation, &current, &mut output, limits.max_output_bytes)?;
        if output.len() > limits.max_output_bytes {
            return Err(TransformError::OutputLimit {
                limit: limits.max_output_bytes,
            });
        }
        work = work.checked_add(output.len()).ok_or(TransformError::Arithmetic)?;
        if work > limits.max_work_bytes {
            return Err(TransformError::WorkLimit {
                limit: limits.max_work_bytes,
            });
        }
        std::mem::swap(&mut current, &mut output);
    }
    Ok(current)
}

pub(super) fn apply_transformations_multimatch(
    input: &[u8],
    transformations: &[TransformationKind],
    limits: TransformLimits,
) -> Result<Vec<Vec<u8>>, TransformError> {
    if input.len() > limits.max_output_bytes {
        return Err(TransformError::OutputLimit {
            limit: limits.max_output_bytes,
        });
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(transformations.len().saturating_add(1))
        .map_err(|_| TransformError::Allocation)?;
    let mut current = copy_bytes(input)?;
    values.push(copy_bytes(input)?);
    let mut output = Vec::new();
    let mut work = 0_usize;
    for transformation in transformations {
        work = work.checked_add(current.len()).ok_or(TransformError::Arithmetic)?;
        output.clear();
        apply_one(*transformation, &current, &mut output, limits.max_output_bytes)?;
        work = work.checked_add(output.len()).ok_or(TransformError::Arithmetic)?;
        if work > limits.max_work_bytes {
            return Err(TransformError::WorkLimit {
                limit: limits.max_work_bytes,
            });
        }
        if output != current {
            values.push(copy_bytes(&output)?);
            std::mem::swap(&mut current, &mut output);
        }
    }
    Ok(values)
}

fn copy_bytes(input: &[u8]) -> Result<Vec<u8>, TransformError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(input.len())
        .map_err(|_| TransformError::Allocation)?;
    output.extend_from_slice(input);
    Ok(output)
}

fn apply_one(
    transformation: TransformationKind,
    input: &[u8],
    output: &mut Vec<u8>,
    limit: usize,
) -> Result<(), TransformError> {
    let projected = projected_output(transformation, input.len())?;
    reserve_output(output, projected.min(limit))?;
    match transformation {
        TransformationKind::Base64Decode => base64_decode(input, output, limit),
        TransformationKind::CmdLine => cmd_line(input, output, limit),
        TransformationKind::CompressWhitespace => compress_whitespace(input, output, limit),
        TransformationKind::CssDecode => css_decode(input, output, limit),
        TransformationKind::EscapeSeqDecode => escape_sequence_decode(input, output, limit),
        TransformationKind::HexEncode => hex_encode(input, output, limit),
        TransformationKind::HtmlEntityDecode => html_entity_decode(input, output, limit),
        TransformationKind::JsDecode => js_decode(input, output, limit),
        TransformationKind::Length => push_decimal(input.len(), output, limit),
        TransformationKind::Lowercase => lowercase(input, output, limit),
        TransformationKind::None => append(output, input, limit),
        TransformationKind::NormalizePath => normalize_path(input, output, false, limit),
        TransformationKind::NormalizePathWin => normalize_path(input, output, true, limit),
        TransformationKind::RemoveCommentsChar => remove_comment_markers(input, output, limit),
        TransformationKind::RemoveNulls => filter_bytes(input, output, |byte| byte != 0, limit),
        TransformationKind::RemoveWhitespace => filter_bytes(input, output, |byte| !is_latin_space(byte), limit),
        TransformationKind::ReplaceComments => replace_comments(input, output, limit),
        TransformationKind::Sha1 => sha1(input, output, limit),
        TransformationKind::UrlDecodeUni => url_decode_uni(input, output, limit),
        TransformationKind::Utf8ToUnicode => utf8_to_unicode(input, output, limit),
    }
}

fn projected_output(transformation: TransformationKind, input: usize) -> Result<usize, TransformError> {
    match transformation {
        TransformationKind::HexEncode => input.checked_mul(2),
        TransformationKind::Utf8ToUnicode => input.checked_mul(6),
        TransformationKind::Length => Some(40),
        TransformationKind::Sha1 => Some(20),
        _ => Some(input),
    }
    .ok_or(TransformError::Arithmetic)
}

fn reserve_output(output: &mut Vec<u8>, additional: usize) -> Result<(), TransformError> {
    output
        .try_reserve(additional.saturating_sub(output.capacity()))
        .map_err(|_| TransformError::Allocation)
}

fn append(output: &mut Vec<u8>, value: &[u8], limit: usize) -> Result<(), TransformError> {
    let actual = output
        .len()
        .checked_add(value.len())
        .ok_or(TransformError::Arithmetic)?;
    if actual > limit {
        return Err(TransformError::OutputLimit { limit });
    }
    output.extend_from_slice(value);
    Ok(())
}

fn push_byte(output: &mut Vec<u8>, value: u8, limit: usize) -> Result<(), TransformError> {
    if output.len() == limit {
        return Err(TransformError::OutputLimit { limit });
    }
    output.push(value);
    Ok(())
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn base64_decode(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut accumulator = 0_u32;
    let mut count = 0_u8;
    for byte in input.iter().copied() {
        if matches!(byte, b'\r' | b'\n') {
            continue;
        }
        let Some(value) = base64_value(byte) else {
            break;
        };
        accumulator = (accumulator << 6) | u32::from(value);
        count += 1;
        if count == 4 {
            push_byte(output, (accumulator >> 16) as u8, limit)?;
            push_byte(output, (accumulator >> 8) as u8, limit)?;
            push_byte(output, accumulator as u8, limit)?;
            accumulator = 0;
            count = 0;
        }
    }
    if count == 2 {
        push_byte(output, (accumulator << 12 >> 16) as u8, limit)?;
    } else if count == 3 {
        let value = accumulator << 6;
        push_byte(output, (value >> 16) as u8, limit)?;
        push_byte(output, (value >> 8) as u8, limit)?;
    }
    Ok(())
}

fn cmd_line(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut pending_space = false;
    for mut byte in input.iter().copied() {
        match byte {
            b'"' | b'\'' | b'\\' | b'^' => continue,
            b' ' | b',' | b';' | b'\t' | b'\r' | b'\n' => {
                if !pending_space {
                    push_byte(output, b' ', limit)?;
                    pending_space = true;
                }
                continue;
            }
            b'/' | b'(' => {
                if pending_space && output.last() == Some(&b' ') {
                    output.pop();
                }
            }
            b'A'..=b'Z' => byte = byte.to_ascii_lowercase(),
            _ => {}
        }
        push_byte(output, byte, limit)?;
        pending_space = false;
    }
    Ok(())
}

fn is_latin_space(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ' | 0x85 | 0xa0)
}

fn compress_whitespace(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut in_space = false;
    for byte in input.iter().copied() {
        if is_latin_space(byte) {
            if !in_space {
                push_byte(output, b' ', limit)?;
                in_space = true;
            }
        } else {
            push_byte(output, byte, limit)?;
            in_space = false;
        }
    }
    Ok(())
}

fn css_decode(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut index = 0_usize;
    while index < input.len() {
        if input[index] != b'\\' {
            push_byte(output, input[index], limit)?;
            index += 1;
            continue;
        }
        index += 1;
        if index == input.len() {
            break;
        }
        let mut digits = 0_usize;
        while digits < 6 && index + digits < input.len() && hex(input[index + digits]).is_some() {
            digits += 1;
        }
        if digits == 0 {
            if input[index] == b'\n' {
                index += 1;
            } else {
                push_byte(output, input[index], limit)?;
                index += 1;
            }
            continue;
        }
        let mut value = 0_u32;
        for digit in &input[index..index + digits] {
            value = (value << 4) | u32::from(hex(*digit).unwrap_or_default());
        }
        let mut byte = value as u8;
        if (0xff01..=0xff5e).contains(&value) {
            byte = byte.wrapping_add(0x20);
        }
        push_byte(output, byte, limit)?;
        index += digits;
        if index < input.len() && is_ascii_space(input[index]) {
            index += 1;
        }
    }
    Ok(())
}

fn escape_sequence_decode(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut index = 0_usize;
    while index < input.len() {
        if input[index] != b'\\' || index + 1 == input.len() {
            push_byte(output, input[index], limit)?;
            index += 1;
            continue;
        }
        let escaped = input[index + 1];
        let simple = match escaped {
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0c),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'v' => Some(0x0b),
            b'\\' | b'?' | b'\'' | b'"' => Some(escaped),
            _ => None,
        };
        if let Some(value) = simple {
            push_byte(output, value, limit)?;
            index += 2;
        } else if matches!(escaped, b'x' | b'X')
            && index + 3 < input.len()
            && hex(input[index + 2]).is_some()
            && hex(input[index + 3]).is_some()
        {
            push_byte(output, decode_hex_pair(input[index + 2], input[index + 3]), limit)?;
            index += 4;
        } else if matches!(escaped, b'0'..=b'7') {
            let mut length = 1_usize;
            while length < 3 && index + 1 + length < input.len() && matches!(input[index + 1 + length], b'0'..=b'7') {
                length += 1;
            }
            let value = input[index + 1..index + 1 + length]
                .iter()
                .fold(0_u16, |value, digit| (value << 3) | u16::from(*digit - b'0'));
            push_byte(output, value as u8, limit)?;
            index += 1 + length;
        } else {
            push_byte(output, escaped, limit)?;
            index += 2;
        }
    }
    Ok(())
}

fn hex_encode(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in input.iter().copied() {
        push_byte(output, DIGITS[(byte >> 4) as usize], limit)?;
        push_byte(output, DIGITS[(byte & 0x0f) as usize], limit)?;
    }
    Ok(())
}

fn html_entity_decode(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut index = 0_usize;
    while index < input.len() {
        if input[index] != b'&' {
            push_byte(output, input[index], limit)?;
            index += 1;
            continue;
        }
        let candidate = &input[index + 1..];
        let Some(relative_end) = candidate.iter().position(|byte| *byte == b';' || *byte == b'&') else {
            push_byte(output, b'&', limit)?;
            index += 1;
            continue;
        };
        if candidate[relative_end] != b';' {
            push_byte(output, b'&', limit)?;
            index += 1;
            continue;
        }
        let end = index + 1 + relative_end;
        let entity = &input[index + 1..end];
        let decoded = decode_entity(entity);
        if let Some(value) = decoded {
            append_codepoint(output, value, limit)?;
            index = end + 1;
        } else {
            push_byte(output, b'&', limit)?;
            index += 1;
        }
    }
    Ok(())
}

fn decode_entity(entity: &[u8]) -> Option<u32> {
    if let Some(number) = entity.strip_prefix(b"#x").or_else(|| entity.strip_prefix(b"#X")) {
        return parse_radix(number, 16);
    }
    if let Some(number) = entity.strip_prefix(b"#") {
        return parse_radix(number, 10);
    }
    match entity {
        b"amp" => Some(u32::from(b'&')),
        b"apos" => Some(u32::from(b'\'')),
        b"colon" => Some(u32::from(b':')),
        b"gt" => Some(u32::from(b'>')),
        b"lt" => Some(u32::from(b'<')),
        b"nbsp" => Some(0xa0),
        b"newline" => Some(u32::from(b'\n')),
        b"quot" => Some(u32::from(b'"')),
        b"sol" => Some(u32::from(b'/')),
        b"tab" => Some(u32::from(b'\t')),
        _ => None,
    }
}

fn parse_radix(value: &[u8], radix: u32) -> Option<u32> {
    if value.is_empty() {
        return None;
    }
    let significant = value.iter().position(|byte| *byte != b'0').unwrap_or(value.len());
    let value = &value[significant..];
    if value.is_empty() {
        return Some(0);
    }
    value.iter().try_fold(0_u32, |result, byte| {
        let digit = match byte {
            b'0'..=b'9' => u32::from(*byte - b'0'),
            b'a'..=b'f' if radix == 16 => u32::from(*byte - b'a' + 10),
            b'A'..=b'F' if radix == 16 => u32::from(*byte - b'A' + 10),
            _ => return None,
        };
        if digit >= radix {
            return None;
        }
        result.checked_mul(radix)?.checked_add(digit)
    })
}

fn append_codepoint(output: &mut Vec<u8>, value: u32, limit: usize) -> Result<(), TransformError> {
    let value = char::from_u32(value).unwrap_or('\u{fffd}');
    let mut buffer = [0_u8; 4];
    append(output, value.encode_utf8(&mut buffer).as_bytes(), limit)
}

fn js_decode(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut index = 0_usize;
    while index < input.len() {
        if input[index] != b'\\' || index + 1 == input.len() {
            push_byte(output, input[index], limit)?;
            index += 1;
            continue;
        }
        if index + 5 < input.len()
            && input[index + 1] == b'u'
            && input[index + 2..index + 6].iter().all(|byte| hex(*byte).is_some())
        {
            let mut value = input[index + 2..index + 6].iter().fold(0_u16, |value, byte| {
                (value << 4) | u16::from(hex(*byte).unwrap_or_default())
            });
            if (0xff01..=0xff5e).contains(&value) {
                value += 0x20;
            }
            push_byte(output, value as u8, limit)?;
            index += 6;
        } else if index + 3 < input.len()
            && input[index + 1] == b'x'
            && hex(input[index + 2]).is_some()
            && hex(input[index + 3]).is_some()
        {
            push_byte(output, decode_hex_pair(input[index + 2], input[index + 3]), limit)?;
            index += 4;
        } else if matches!(input[index + 1], b'0'..=b'7') {
            let mut length = 1_usize;
            while length < 3 && index + 1 + length < input.len() && matches!(input[index + 1 + length], b'0'..=b'7') {
                length += 1;
            }
            if length == 3 && input[index + 1] > b'3' {
                length = 2;
            }
            let value = input[index + 1..index + 1 + length]
                .iter()
                .fold(0_u16, |value, digit| (value << 3) | u16::from(*digit - b'0'));
            push_byte(output, value as u8, limit)?;
            index += 1 + length;
        } else {
            let value = match input[index + 1] {
                b'a' => 0x07,
                b'b' => 0x08,
                b'f' => 0x0c,
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'v' => 0x0b,
                byte => byte,
            };
            push_byte(output, value, limit)?;
            index += 2;
        }
    }
    Ok(())
}

fn lowercase(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    for &byte in input {
        push_byte(output, byte.to_ascii_lowercase(), limit)?;
    }
    Ok(())
}

fn normalize_path(input: &[u8], output: &mut Vec<u8>, windows: bool, limit: usize) -> Result<(), TransformError> {
    let absolute = input
        .first()
        .is_some_and(|byte| *byte == b'/' || (windows && *byte == b'\\'));
    let trailing = input
        .last()
        .is_some_and(|byte| *byte == b'/' || (windows && *byte == b'\\'));
    let mut components: Vec<&[u8]> = Vec::new();
    components
        .try_reserve(input.len().div_ceil(2))
        .map_err(|_| TransformError::Allocation)?;
    for component in input.split(|byte| *byte == b'/' || (windows && *byte == b'\\')) {
        match component {
            b"" | b"." => {}
            b".." if components.last().is_some_and(|last| *last != b"..") => {
                components.pop();
            }
            b".." if !absolute => components.push(component),
            b".." => {}
            _ => components.push(component),
        }
    }
    if absolute {
        push_byte(output, b'/', limit)?;
    }
    for (index, component) in components.iter().enumerate() {
        if index != 0 {
            push_byte(output, b'/', limit)?;
        }
        append(output, component, limit)?;
    }
    if trailing && !output.is_empty() && output.last() != Some(&b'/') {
        push_byte(output, b'/', limit)?;
    }
    Ok(())
}

fn remove_comment_markers(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut index = 0_usize;
    while index < input.len() {
        let removed = if input[index..].starts_with(b"<!--") {
            4
        } else if input[index..].starts_with(b"-->") {
            3
        } else if input[index..].starts_with(b"/*")
            || input[index..].starts_with(b"*/")
            || input[index..].starts_with(b"--")
        {
            2
        } else if input[index] == b'#' {
            1
        } else {
            0
        };
        if removed == 0 {
            push_byte(output, input[index], limit)?;
            index += 1;
        } else {
            index += removed;
        }
    }
    Ok(())
}

fn filter_bytes(
    input: &[u8],
    output: &mut Vec<u8>,
    keep: impl Fn(u8) -> bool,
    limit: usize,
) -> Result<(), TransformError> {
    for byte in input.iter().copied().filter(|byte| keep(*byte)) {
        push_byte(output, byte, limit)?;
    }
    Ok(())
}

fn replace_comments(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut index = 0_usize;
    while index < input.len() {
        if !input[index..].starts_with(b"/*") {
            push_byte(output, input[index], limit)?;
            index += 1;
            continue;
        }
        index += 2;
        while index < input.len() && !input[index..].starts_with(b"*/") {
            index += 1;
        }
        if index < input.len() {
            index += 2;
        }
        push_byte(output, b' ', limit)?;
    }
    Ok(())
}

fn sha1(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let digest = Sha1::digest(input);
    append(output, digest.as_slice(), limit)
}

fn url_decode_uni(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut index = 0_usize;
    while index < input.len() {
        if input[index] == b'+' {
            push_byte(output, b' ', limit)?;
            index += 1;
        } else if input[index] == b'%'
            && index + 5 < input.len()
            && matches!(input[index + 1], b'u' | b'U')
            && input[index + 2..index + 6].iter().all(|byte| hex(*byte).is_some())
        {
            let high = decode_hex_pair(input[index + 2], input[index + 3]);
            let mut low = decode_hex_pair(input[index + 4], input[index + 5]);
            if high == 0xff && (1..0x5f).contains(&low) {
                low = low.wrapping_add(0x20);
            }
            push_byte(output, low, limit)?;
            index += 6;
        } else if input[index] == b'%'
            && index + 2 < input.len()
            && hex(input[index + 1]).is_some()
            && hex(input[index + 2]).is_some()
        {
            push_byte(output, decode_hex_pair(input[index + 1], input[index + 2]), limit)?;
            index += 3;
        } else {
            push_byte(output, input[index], limit)?;
            index += 1;
        }
    }
    Ok(())
}

fn utf8_to_unicode(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut index = 0_usize;
    while index < input.len() {
        if input[index].is_ascii() {
            push_byte(output, input[index], limit)?;
            index += 1;
            continue;
        }
        let width = utf8_width(input[index]);
        if width == 0 || index + width > input.len() {
            push_byte(output, input[index], limit)?;
            index += 1;
            continue;
        }
        let candidate = &input[index..index + width];
        let Ok(text) = std::str::from_utf8(candidate) else {
            push_byte(output, input[index], limit)?;
            index += 1;
            continue;
        };
        let Some(value) = text.chars().next() else {
            return Err(TransformError::Arithmetic);
        };
        append(output, b"%u", limit)?;
        let number = value as u32;
        let width = hex_digit_count(number).max(4);
        for shift in (0..width).rev() {
            let digit = ((number >> (shift * 4)) & 0x0f) as usize;
            push_byte(output, b"0123456789abcdef"[digit], limit)?;
        }
        index += candidate.len();
    }
    Ok(())
}

fn utf8_width(first: u8) -> usize {
    match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 0,
    }
}

fn hex_digit_count(mut value: u32) -> usize {
    let mut count = 1_usize;
    while value >= 16 {
        value >>= 4;
        count += 1;
    }
    count
}

fn push_decimal(mut value: usize, output: &mut Vec<u8>, limit: usize) -> Result<(), TransformError> {
    let mut digits = [0_u8; 40];
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return append(output, &digits[start..], limit);
        }
    }
}

fn is_ascii_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn decode_hex_pair(high: u8, low: u8) -> u8 {
    (hex(high).unwrap_or_default() << 4) | hex(low).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(kind: TransformationKind, input: &[u8]) -> Vec<u8> {
        apply_transformations(
            input,
            &[kind],
            TransformLimits {
                max_output_bytes: 4_096,
                max_work_bytes: 16_384,
            },
        )
        .unwrap()
    }

    #[test]
    fn selected_crs_transformations_have_frozen_byte_behavior() {
        let cases: &[(TransformationKind, &[u8], &[u8])] = &[
            (TransformationKind::Base64Decode, b"PFRFU1Q+", b"<TEST>"),
            (TransformationKind::CmdLine, b"C^OMMAND /C DIR", b"command/c dir"),
            (TransformationKind::CompressWhitespace, b"a\t  b", b"a b"),
            (TransformationKind::CssDecode, b"a\\000062 c", b"abc"),
            (TransformationKind::EscapeSeqDecode, br"a\x62\143", b"abc"),
            (TransformationKind::HexEncode, b"A\0", b"4100"),
            (TransformationKind::HtmlEntityDecode, b"&lt;&#x41;&#66;", b"<AB"),
            (TransformationKind::JsDecode, br"a\x62\143", b"abc"),
            (TransformationKind::Length, b"four", b"4"),
            (TransformationKind::Lowercase, b"AbC", b"abc"),
            (TransformationKind::NormalizePath, b"/a/./b/../c/", b"/a/c/"),
            (TransformationKind::NormalizePathWin, br"C:\a\..\b", b"C:/b"),
            (TransformationKind::RemoveCommentsChar, b"a/*b*/c#d", b"abcd"),
            (TransformationKind::RemoveNulls, b"a\0b", b"ab"),
            (TransformationKind::RemoveWhitespace, b"a \tb\nc", b"abc"),
            (TransformationKind::ReplaceComments, b"a/*private*/b", b"a b"),
            (TransformationKind::UrlDecodeUni, b"%uFF1C+%41", b"< A"),
            (TransformationKind::Utf8ToUnicode, "é".as_bytes(), b"%u00e9"),
        ];
        for (kind, input, expected) in cases {
            assert_eq!(transform(*kind, input), *expected, "{kind:?}");
        }
        assert_eq!(transform(TransformationKind::Sha1, b"abc").len(), 20);
        assert_eq!(
            transform(
                TransformationKind::HtmlEntityDecode,
                b"&#00000000000000000000000000000000000000000000000123;"
            ),
            b"{"
        );
    }

    #[test]
    fn output_and_work_limits_cover_exact_and_first_excess() {
        assert_eq!(
            apply_transformations(
                b"ab",
                &[TransformationKind::HexEncode],
                TransformLimits {
                    max_output_bytes: 4,
                    max_work_bytes: 6,
                }
            )
            .unwrap(),
            b"6162"
        );
        assert_eq!(
            apply_transformations(
                b"ab",
                &[TransformationKind::HexEncode],
                TransformLimits {
                    max_output_bytes: 3,
                    max_work_bytes: 6,
                }
            ),
            Err(TransformError::OutputLimit { limit: 3 })
        );
        assert_eq!(
            apply_transformations(
                b"ab",
                &[TransformationKind::HexEncode],
                TransformLimits {
                    max_output_bytes: 4,
                    max_work_bytes: 5,
                }
            ),
            Err(TransformError::WorkLimit { limit: 5 })
        );
    }

    #[test]
    fn errors_never_render_transformed_content() {
        let error = apply_transformations(
            b"private-value",
            &[TransformationKind::HexEncode],
            TransformLimits {
                max_output_bytes: 1,
                max_work_bytes: usize::MAX,
            },
        )
        .unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("private"));
    }
}
