#![no_main]

//! Drives arbitrary bytes through the decoders and the filter compiler.
//!
//! The first byte selects the mode:
//! - bit 7 set: the rest is newline-separated group DNs (first line: the
//!   attribute; bit 0 selects nested) for `SearchFilter::group_requirement`;
//!   the pre-encoded filter must encode to `encoded_len` bytes;
//! - bit 6 set: the rest is a DN for `dn::is_valid` (strict admission),
//!   `dn::normalize` and `dn::equivalent` (reflexivity is asserted for every
//!   DN that normalizes, including directory spellings with `;`);
//! - bit 5 set: the rest is filter text for `SearchFilter::compile`, and a
//!   compiled filter is encoded with a fixed username;
//! - otherwise the rest is a server frame: bit 4 selects the search-frame
//!   decoder, else bit 0 selects ExtendedResponse vs BindResponse; bits 1-3
//!   give the expected message ID.
//!
//! The only property is: no panic, no abort, no unbounded recursion.

use edgion_ldap::codec::{frame_content_len, length_octets};
use edgion_ldap::filter::SearchFilter;
use edgion_ldap::proto::{decode_bind_response, decode_extended_response, decode_search_frame};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    if selector & 0x80 != 0 {
        if let Ok(text) = std::str::from_utf8(rest) {
            let mut lines = text.split('\n');
            let attribute = lines.next().unwrap_or_default();
            let dns: Vec<&str> = lines.collect();
            let (any_of, all_of) = dns.split_at(dns.len() / 2);
            if let Ok(filter) = SearchFilter::group_requirement(attribute, any_of, all_of, selector & 1 == 1) {
                let mut out = Vec::new();
                filter.encode(b"ignored", &mut out);
                assert_eq!(out.len(), filter.encoded_len(7));
            }
        }
        return;
    }
    if selector & 0x40 != 0 {
        if let Ok(text) = std::str::from_utf8(rest) {
            let _ = edgion_ldap::dn::is_valid(text);
            if edgion_ldap::dn::normalize(text).is_some() {
                assert!(edgion_ldap::dn::equivalent(text, text));
            }
            let _ = edgion_ldap::dn::equivalent(text, "cn=svc,dc=example,dc=com");
        }
        return;
    }
    if selector & 0x20 != 0 {
        if let Ok(text) = std::str::from_utf8(rest) {
            if let Ok(filter) = SearchFilter::compile(text) {
                let mut out = Vec::new();
                filter.encode(b"u*)(x", &mut out);
                assert_eq!(out.len(), filter.encoded_len(5));
            }
        }
        return;
    }
    let expected_id = u32::from((selector >> 1) & 0x07);
    let [tag, first, rest @ ..] = rest else {
        return;
    };
    let Ok(extra) = length_octets(*tag, *first) else {
        return;
    };
    let Some(octets) = rest.get(..extra) else {
        return;
    };
    let Ok(len) = frame_content_len(*first, octets) else {
        return;
    };
    let Some(content) = rest.get(extra..extra + len) else {
        return;
    };
    if selector & 0x10 != 0 {
        let _ = decode_search_frame(content, expected_id, "uid");
    } else if selector & 1 == 1 {
        let _ = decode_extended_response(content, expected_id);
    } else {
        let _ = decode_bind_response(content, expected_id);
    }
});
