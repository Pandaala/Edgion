//! Independently defined JSON numeric values, without document deserialization.

use std::fmt;
use std::io::Write;

/// A validated JSON number. Float payloads are finite and preserve signed zero.
/// Debug deliberately omits the request-controlled numeric payload.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct JsonNumber {
    repr: Representation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Representation {
    Unsigned(u64),
    Signed(i64),
    FloatBits(u64),
}

impl fmt::Debug for JsonNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JsonNumber { .. }")
    }
}

impl JsonNumber {
    /// The caller has already accepted the complete spelling with the JSON
    /// number grammar FSM. This function is not a standalone syntax validator.
    pub(super) fn from_validated_lexeme(raw: &str) -> Result<Self, ()> {
        if memchr::memchr3(b'.', b'e', b'E', raw.as_bytes()).is_none() {
            let negative = raw.starts_with('-');
            let digits = if negative { &raw.as_bytes()[1..] } else { raw.as_bytes() };
            let magnitude = digits.iter().try_fold(0u64, |value, byte| {
                value.checked_mul(10)?.checked_add(u64::from(*byte - b'0'))
            });
            if let Some(magnitude) = magnitude {
                if !negative {
                    return Ok(Self {
                        repr: Representation::Unsigned(magnitude),
                    });
                }
                if magnitude != 0 && magnitude <= (1u64 << 63) {
                    let signed = if magnitude == (1u64 << 63) {
                        i64::MIN
                    } else {
                        -(magnitude as i64)
                    };
                    return Ok(Self {
                        repr: Representation::Signed(signed),
                    });
                }
            }
        }
        let value = raw.parse::<f64>().map_err(|_| ())?;
        if !value.is_finite() {
            return Err(());
        }
        Ok(Self {
            repr: Representation::FloatBits(value.to_bits()),
        })
    }

    pub(super) fn write_canonical(self, output: &mut impl Write) -> std::io::Result<()> {
        match self.repr {
            Representation::Unsigned(value) => write!(output, "{value}"),
            Representation::Signed(value) => write!(output, "{value}"),
            Representation::FloatBits(bits) => {
                let mut formatter = zmij::Buffer::new();
                output.write_all(formatter.format(f64::from_bits(bits)).as_bytes())
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_number_test.rs"]
mod tests;
