use super::*;

fn canonical(raw: &str) -> String {
    let number = JsonNumber::from_validated_lexeme(raw).unwrap();
    let mut output = Vec::new();
    number.write_canonical(&mut output).unwrap();
    String::from_utf8(output).unwrap()
}

#[test]
fn checked_integer_boundaries_preserve_exact_spelling() {
    for raw in [
        "0",
        "9223372036854775807",
        "18446744073709551615",
        "-9223372036854775808",
        "-1",
    ] {
        assert_eq!(canonical(raw), raw);
    }
    assert_eq!(canonical("18446744073709551616"), "1.8446744073709552e+19");
    assert_eq!(canonical("-9223372036854775809"), "-9.223372036854776e+18");
}

#[test]
fn float_profile_uses_explicit_correct_rounding_vectors() {
    for (raw, bits, output) in [
        ("9007199254740993.0", 0x4340000000000000, "9007199254740992.0"),
        (
            "1.00000000000000011102230246251565404236316680908203125",
            0x3ff0000000000000,
            "1.0",
        ),
        ("2.4703282292062327e-324", 0, "0.0"),
        ("2.4703282292062328e-324", 1, "5e-324"),
        ("1.7976931348623157e308", 0x7fefffffffffffff, "1.7976931348623157e+308"),
    ] {
        let number = JsonNumber::from_validated_lexeme(raw).unwrap();
        assert!(
            matches!(number.repr, Representation::FloatBits(actual) if actual == bits),
            "{raw}"
        );
        assert_eq!(canonical(raw), output);
    }
}

#[test]
fn signed_zero_extreme_exponents_and_nonfinite_rejection() {
    for raw in [
        "-0",
        "-0.0",
        "-0e99999999999999999999999999999",
        "-0e-99999999999999999999999999999",
        "-1e-99999999999999999999999999999",
    ] {
        let number = JsonNumber::from_validated_lexeme(raw).unwrap();
        assert!(matches!(number.repr, Representation::FloatBits(0x8000000000000000)));
        assert_eq!(canonical(raw), "-0.0");
    }
    assert_eq!(canonical("0e99999999999999999999999999999"), "0.0");
    for raw in ["1e309", "-1e309", "1e99999999999999999999999999999"] {
        assert!(JsonNumber::from_validated_lexeme(raw).is_err());
    }
}

#[test]
fn numeric_debug_redacts_payload_and_output_storage_is_bounded() {
    let number = JsonNumber::from_validated_lexeme("123456789").unwrap();
    assert_eq!(format!("{number:?}"), "JsonNumber { .. }");
    let mut buffer = [0; 2];
    assert!(number.write_canonical(&mut &mut buffer[..]).is_err());
}
