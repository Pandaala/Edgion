//! Schema-aware allocation/work admission before reflection decoding.
use super::{TranscodingError, TranscodingLimits};
use prost_reflect::{Kind, MessageDescriptor};

pub(super) fn preflight(
    desc: &MessageDescriptor,
    bytes: &[u8],
    limits: TranscodingLimits,
) -> Result<(), TranscodingError> {
    let mut nodes = 0;
    walk::<false>(desc, bytes, limits, 1, &mut nodes)
}
pub(super) fn preflight_descriptor(
    desc: &MessageDescriptor,
    bytes: &[u8],
    limits: TranscodingLimits,
) -> Result<(), TranscodingError> {
    let mut nodes = 0;
    walk::<true>(desc, bytes, limits, 1, &mut nodes)
}

fn count(nodes: &mut u64, limits: TranscodingLimits) -> Result<(), TranscodingError> {
    *nodes = nodes.checked_add(1).ok_or(TranscodingError::Invalid)?;
    if *nodes > limits.nodes {
        return Err(TranscodingError::Invalid);
    }
    Ok(())
}
fn varint(bytes: &mut &[u8]) -> Result<u64, TranscodingError> {
    let mut result = 0;
    for shift in (0..70).step_by(7) {
        let (&first, rest) = bytes.split_first().ok_or(TranscodingError::Invalid)?;
        *bytes = rest;
        if shift == 63 && first > 1 {
            return Err(TranscodingError::Invalid);
        }
        result |= ((first & 127) as u64) << shift;
        if first < 128 {
            return Ok(result);
        }
    }
    Err(TranscodingError::Invalid)
}
fn take<'a>(bytes: &mut &'a [u8], len: usize) -> Result<&'a [u8], TranscodingError> {
    let output = bytes.get(..len).ok_or(TranscodingError::Invalid)?;
    *bytes = &bytes[len..];
    Ok(output)
}
fn walk<const DESCRIPTOR: bool>(
    desc: &MessageDescriptor,
    mut bytes: &[u8],
    limits: TranscodingLimits,
    depth: usize,
    nodes: &mut u64,
) -> Result<(), TranscodingError> {
    if depth > limits.depth {
        return Err(TranscodingError::Invalid);
    }
    count(nodes, limits)?;
    while !bytes.is_empty() {
        count(nodes, limits)?;
        let tag = varint(&mut bytes)?;
        let number = u32::try_from(tag >> 3).map_err(|_| TranscodingError::Invalid)?;
        if number == 0 || number > 0x1fffffff {
            return Err(TranscodingError::Invalid);
        }
        let wire = tag & 7;
        let field = desc.get_field(number);
        let shape = field.as_ref().map(|field| (field.kind(), field.is_list())).or_else(|| {
            if DESCRIPTOR {
                desc.get_extension(number)
                    .map(|extension| (extension.kind(), extension.is_list()))
            } else {
                None
            }
        });
        if DESCRIPTOR && (shape.is_none() || (number == 999 && desc.full_name().ends_with("Options"))) {
            // Arbitrary encoded or uninterpreted options are not part of this
            // profile. HTTP annotations use the trusted extension schema.
            return Err(TranscodingError::Invalid);
        }
        match wire {
            0 => {
                varint(&mut bytes)?;
            }
            1 => {
                take(&mut bytes, 8)?;
            }
            2 => {
                let len = usize::try_from(varint(&mut bytes)?).map_err(|_| TranscodingError::Invalid)?;
                let contents = take(&mut bytes, len)?;
                if let Some((kind, list)) = &shape {
                    match kind {
                        Kind::Message(message) => walk::<DESCRIPTOR>(message, contents, limits, depth + 1, nodes)?,
                        Kind::String | Kind::Bytes => {}
                        kind if *list => {
                            packed(kind, contents, limits, nodes)?;
                        }
                        _ => return Err(TranscodingError::Invalid),
                    }
                }
            }
            5 => {
                take(&mut bytes, 4)?;
            }
            _ => return Err(TranscodingError::Invalid),
        }
        if let Some((kind, list)) = shape {
            let expected = match kind {
                Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => 1,
                Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => 5,
                Kind::String | Kind::Bytes | Kind::Message(_) => 2,
                _ => 0,
            };
            if wire != expected && !(wire == 2 && list && expected != 2) {
                return Err(TranscodingError::Invalid);
            }
        }
    }
    Ok(())
}

fn packed(
    kind: &Kind,
    mut contents: &[u8],
    limits: TranscodingLimits,
    nodes: &mut u64,
) -> Result<(), TranscodingError> {
    let width = match kind {
        Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => 8,
        Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => 4,
        _ => 0,
    };
    if width != 0 && contents.len().is_multiple_of(width) {
        if let Some(next) = u64::try_from(contents.len() / width)
            .ok()
            .and_then(|elements| nodes.checked_add(elements))
            .filter(|next| *next <= limits.nodes)
        {
            *nodes = next;
            return Ok(());
        }
    }
    // Keep partial charges and failure ordering for truncated or over-budget input.
    while !contents.is_empty() {
        count(nodes, limits)?;
        match kind {
            Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => {
                take(&mut contents, 8)?;
            }
            Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => {
                take(&mut contents, 4)?;
            }
            _ => {
                varint(&mut contents)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/grpc_transcoding/wire_test.rs"]
mod tests;
