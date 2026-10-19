//! Admit the reflected protobuf shape before dependency visitors allocate it.
use super::{TranscodingError as Error, TranscodingLimits};
use prost_reflect::{Kind, MessageDescriptor};
use serde_json::Value;

struct Budget {
    limits: TranscodingLimits,
    nodes: u64,
}
impl Budget {
    fn charge(&mut self, depth: usize, count: u64) -> Result<(), Error> {
        self.nodes = self.nodes.checked_add(count).ok_or(Error::Invalid)?;
        if depth > self.limits.depth || self.nodes > self.limits.nodes {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
pub(super) fn normalize(desc: &MessageDescriptor, value: &mut Value, limits: TranscodingLimits) -> Result<(), Error> {
    message(desc, value, 1, &mut Budget { limits, nodes: 0 })
}
fn message(desc: &MessageDescriptor, value: &mut Value, depth: usize, budget: &mut Budget) -> Result<(), Error> {
    budget.charge(depth, 1)?;
    match desc.full_name() {
        "google.protobuf.Value" => return wkt_value(value, depth, budget),
        "google.protobuf.Struct" => return wkt_struct(value, depth, budget),
        "google.protobuf.ListValue" => return wkt_list(value, depth, budget),
        "google.protobuf.FieldMask" => {
            let text = value.as_str().ok_or(Error::Invalid)?;
            for _ in text.split(',').filter(|p| !p.is_empty()) {
                budget.charge(depth, 1)?;
            }
            return Ok(());
        }
        "google.protobuf.Timestamp" | "google.protobuf.Duration" => return budget.charge(depth, 2),
        "google.protobuf.DoubleValue"
        | "google.protobuf.FloatValue"
        | "google.protobuf.Int64Value"
        | "google.protobuf.UInt64Value"
        | "google.protobuf.Int32Value"
        | "google.protobuf.UInt32Value"
        | "google.protobuf.BoolValue"
        | "google.protobuf.StringValue"
        | "google.protobuf.BytesValue" => {
            let field = desc.get_field_by_name("value").ok_or(Error::Invalid)?;
            return scalar(&field.kind(), value, depth, budget);
        }
        _ => {}
    }
    let object = value.as_object_mut().ok_or(Error::Invalid)?;
    // Validate every name, including nulls, before removing fields. Simultaneous
    // aliases remain invalid even when one of their values is null.
    let mut fields = std::collections::BTreeSet::new();
    for name in object.keys() {
        let field = desc
            .get_field_by_json_name(name)
            .or_else(|| desc.get_field_by_name(name))
            .ok_or(Error::Invalid)?;
        if !fields.insert(field.number()) {
            return Err(Error::Invalid);
        }
    }
    object.retain(|name, value| {
        if !value.is_null() {
            return true;
        }
        let field = desc
            .get_field_by_json_name(name)
            .or_else(|| desc.get_field_by_name(name))
            .expect("validated field");
        !field.is_list() && !field.is_map() && sentinel(&field.kind())
    });
    for (name, value) in object {
        let field = desc
            .get_field_by_json_name(name)
            .or_else(|| desc.get_field_by_name(name))
            .ok_or(Error::Invalid)?;
        let kind = field.kind();
        if field.is_map() {
            budget.charge(depth, 1)?; // Map container.
            let Kind::Message(entry) = kind else {
                return Err(Error::Invalid);
            };
            let kind = entry.map_entry_value_field().kind();
            for value in value.as_object_mut().ok_or(Error::Invalid)?.values_mut() {
                budget.charge(depth + 1, 2)?; // Entry message and its key.
                scalar(&kind, value, depth + 1, budget)?;
            }
        } else if field.is_list() {
            budget.charge(depth, 1)?; // List container, including spare capacity.
            for value in value.as_array_mut().ok_or(Error::Invalid)? {
                if value.is_null() && !sentinel(&kind) {
                    return Err(Error::Invalid);
                }
                scalar(&kind, value, depth, budget)?;
            }
        } else {
            scalar(&kind, value, depth, budget)?;
        }
    }
    Ok(())
}
fn sentinel(kind: &Kind) -> bool {
    matches!(kind, Kind::Message(m) if m.full_name() == "google.protobuf.Value")
        || matches!(kind, Kind::Enum(e) if e.full_name() == "google.protobuf.NullValue")
}
fn wkt_struct(value: &mut Value, depth: usize, budget: &mut Budget) -> Result<(), Error> {
    budget.charge(depth, 1)?; // fields map.
    for value in value.as_object_mut().ok_or(Error::Invalid)?.values_mut() {
        budget.charge(depth + 1, 3)?; // map entry, key, value field.
        budget.charge(depth + 2, 1)?; // Value message.
        wkt_value(value, depth + 2, budget)?;
    }
    Ok(())
}
fn wkt_list(value: &mut Value, depth: usize, budget: &mut Budget) -> Result<(), Error> {
    budget.charge(depth, 1)?;
    for value in value.as_array_mut().ok_or(Error::Invalid)? {
        budget.charge(depth + 1, 2)?; // repeated field and Value message.
        wkt_value(value, depth + 1, budget)?;
    }
    Ok(())
}
fn wkt_value(value: &mut Value, depth: usize, budget: &mut Budget) -> Result<(), Error> {
    budget.charge(depth, 1)?; // selected oneof field.
    match value {
        Value::Object(_) => {
            budget.charge(depth + 1, 1)?;
            wkt_struct(value, depth + 1, budget)
        }
        Value::Array(_) => {
            budget.charge(depth + 1, 1)?;
            wkt_list(value, depth + 1, budget)
        }
        _ => Ok(()),
    }
}
fn scalar(kind: &Kind, value: &mut Value, depth: usize, budget: &mut Budget) -> Result<(), Error> {
    budget.charge(depth, 1)?;
    match kind {
        Kind::Message(desc) => message(desc, value, depth + 1, budget),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => integer(value, true, 32),
        Kind::Uint32 | Kind::Fixed32 => integer(value, false, 32),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => integer(value, true, 64),
        Kind::Uint64 | Kind::Fixed64 => integer(value, false, 64),
        Kind::Float | Kind::Double => {
            if let Some(s) = value.as_str() {
                if matches!(s, "NaN" | "Infinity" | "-Infinity") {
                    return Ok(());
                }
                let finite = if matches!(kind, Kind::Float) {
                    s.parse::<f32>().is_ok_and(f32::is_finite)
                } else {
                    s.parse::<f64>().is_ok_and(f64::is_finite)
                };
                if !finite {
                    return Err(Error::Invalid);
                }
            } else if matches!(kind, Kind::Float) && value.as_f64().is_some_and(|v| !(v as f32).is_finite()) {
                return Err(Error::Invalid);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
fn integer(value: &mut Value, signed: bool, bits: u32) -> Result<(), Error> {
    let n: i128 = match value {
        Value::String(s) => exact_integer(s)?,
        Value::Number(n) if n.is_i64() => n.as_i64().ok_or(Error::Invalid)? as i128,
        Value::Number(n) if n.is_u64() => n.as_u64().ok_or(Error::Invalid)? as i128,
        Value::Number(n) => {
            let f = n.as_f64().ok_or(Error::Invalid)?;
            let upper = 2_f64.powi(if signed { bits - 1 } else { bits } as i32);
            let lower = if signed { -upper } else { 0.0 };
            if !f.is_finite() || f.fract() != 0.0 || f < lower || f >= upper {
                return Err(Error::Invalid);
            }
            f as i128
        }
        _ => return Err(Error::Invalid),
    };
    let upper = 1_i128 << if signed { bits - 1 } else { bits };
    if n < if signed { -upper } else { 0 } || n >= upper {
        return Err(Error::Invalid);
    }
    // Plain decimal strings retain all 64 bits through reflection's visitors.
    *value = Value::String(n.to_string());
    Ok(())
}

// Scan at most the already bounded input, keeping only 20 significant integral
// digits. No floating conversion, exponent-sized buffer, or arbitrary precision.
fn exact_integer(text: &str) -> Result<i128, Error> {
    let (negative, text) = text
        .strip_prefix('-')
        .map_or((false, text.strip_prefix('+').unwrap_or(text)), |s| (true, s));
    let (mantissa, exponent) = text.split_once(['e', 'E']).map_or((text, "0"), |p| p);
    let (exp_negative, exponent) = exponent
        .strip_prefix('-')
        .map_or((false, exponent.strip_prefix('+').unwrap_or(exponent)), |s| (true, s));
    if exponent.is_empty() || !exponent.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid);
    }
    // Values beyond this exponent cannot change a nonzero bounded mantissa's
    // range decision. Saturation also accepts zero with arbitrarily long exponents.
    let mut exp = 0_i64;
    let bound = super::MAX_JSON_BYTES as i64 + 21;
    for digit in exponent.bytes() {
        exp = (exp * 10 + (digit - b'0') as i64).min(bound);
    }
    let exponent = if exp_negative { -exp } else { exp };
    let (whole, fraction) = mantissa.split_once('.').map_or((mantissa, ""), |p| p);
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (mantissa.contains('.') && fraction.is_empty())
    {
        return Err(Error::Invalid);
    }
    let digits = whole.bytes().chain(fraction.bytes());
    let total = whole.len() + fraction.len();
    let end = (total as i64)
        .checked_add(exponent)
        .and_then(|v| v.checked_sub(fraction.len() as i64))
        .ok_or(Error::Invalid)?;
    let mut n = 0_i128;
    let mut significant = 0;
    for (i, b) in digits.enumerate() {
        if i as i64 >= end {
            if b != b'0' {
                return Err(Error::Invalid);
            }
        } else if significant != 0 || b != b'0' {
            significant += 1;
            if significant > 20 {
                return Err(Error::Invalid);
            }
            n = n * 10 + (b - b'0') as i128;
        }
    }
    if n != 0 && end > total as i64 {
        let zeros = end - total as i64;
        if zeros > 20 - significant {
            return Err(Error::Invalid);
        }
        n *= 10_i128.pow(zeros as u32);
    }
    Ok(if negative { -n } else { n })
}

#[cfg(test)]
#[path = "../../tests/unit/grpc_transcoding/normalize_test.rs"]
mod tests;
