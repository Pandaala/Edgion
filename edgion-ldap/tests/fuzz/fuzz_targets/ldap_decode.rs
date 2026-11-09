#![no_main]

//! Drives arbitrary server bytes through the same frame-header and decoder
//! path `BindSession` uses. The only property is: no panic, no abort, no
//! unbounded recursion.

use edgion_ldap::codec::{frame_content_len, length_octets};
use edgion_ldap::proto::{decode_bind_response, decode_extended_response};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, frame)) = data.split_first() else {
        return;
    };
    let extended = selector & 1 == 1;
    let expected_id = u32::from((selector >> 1) & 0x07);
    let [tag, first, rest @ ..] = frame else {
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
    let _ = if extended {
        decode_extended_response(content, expected_id)
    } else {
        decode_bind_response(content, expected_id)
    };
});
