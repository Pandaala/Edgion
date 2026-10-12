use super::*;

#[test]
fn both_modes_reject_form_feed_outside_strings() {
    for input in [
        b"\x0c{\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":9}}".as_slice(),
        b"{\"usage\":\x0c{\"prompt_tokens\":7,\"completion_tokens\":9}}".as_slice(),
        b"{\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":9}}\x0c".as_slice(),
    ] {
        assert!(Parser::new(input).parse().is_err(), "eager input={input:?}");
        assert!(
            Parser::new_without_decoding(input).parse().is_err(),
            "offset-only input={input:?}"
        );
    }
}

#[test]
fn both_modes_accept_all_json_whitespace() {
    let input = b" \t\n\r{ \t\n\r\"value\" \t\n\r: \t\n\r[ \t\n\r\"ok\" \t\n\r, \t\n\r1 \t\n\r] \t\n\r} \t\n\r";
    for parser in [Parser::new(input), Parser::new_without_decoding(input)] {
        let root = parser.parse().unwrap();
        let Field::Unique(value) = field(&root, "value") else {
            panic!("legal whitespace must preserve the selected field");
        };
        let values = value.array().unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[1].u64(), Some(1));
    }
}

#[test]
fn escaped_form_feed_preserves_decoding_and_offsets_in_both_modes() {
    let input = b" \t\"left\\fright\"\r\n";
    for (parser, eager) in [(Parser::new(input), true), (Parser::new_without_decoding(input), false)] {
        let root = parser.parse().unwrap();
        let token = root.string_token().unwrap();
        assert_eq!(token.start, 2);
        assert_eq!(token.end, input.len() - 2);
        let raw = &input[token.start..token.end];
        assert_eq!(raw, b"\"left\\fright\"");
        assert_eq!(decode_json_string(raw).as_deref(), Some("left\x0cright"));
        assert_eq!(token.value, if eager { "left\x0cright" } else { "" });
    }
}
