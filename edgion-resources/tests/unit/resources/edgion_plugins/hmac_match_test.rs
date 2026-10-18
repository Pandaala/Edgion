use super::HmacAlgorithm;

#[test]
fn algorithm_parser_accepts_every_ascii_case_variant() {
    for (name, expected) in [
        ("hmac-sha256", HmacAlgorithm::HmacSha256),
        ("hmac-sha384", HmacAlgorithm::HmacSha384),
        ("hmac-sha512", HmacAlgorithm::HmacSha512),
    ] {
        let letters = name.bytes().filter(u8::is_ascii_alphabetic).count();
        for mask in 0..(1usize << letters) {
            let mut letter = 0;
            let variant: String = name
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphabetic() {
                        let uppercase = mask & (1 << letter) != 0;
                        letter += 1;
                        if uppercase {
                            return ch.to_ascii_uppercase();
                        }
                    }
                    ch
                })
                .collect();
            assert_eq!(HmacAlgorithm::parse(&variant), Some(expected));
        }
    }
}

#[test]
fn algorithm_parser_rejects_noncanonical_names_without_trimming() {
    for value in [
        "",
        "hmac",
        "hmac-sha",
        "hmac-sha1",
        "hmac-sha256x",
        "xhmac-sha256",
        " hmac-sha256",
        "hmac-sha256 ",
        "hmac-sha256\n",
        "hmac_sha256",
        "hmac-sha0256",
        "hmac-sha256\0",
        "hmác-sha256",
        "hmac-sha256é",
    ] {
        assert_eq!(HmacAlgorithm::parse(value), None, "unexpected algorithm: {value:?}");
    }
}
