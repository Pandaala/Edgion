use super::*;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

#[test]
fn test_ipv4_header_structure() {
    let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 100), 52341));
    let dst = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 244, 1, 23), 8443));

    let header = ProxyProtocolV2Builder::new(src, dst).build();

    // 16 header + 12 addr = 28 bytes
    assert_eq!(header.len(), 28);

    // Signature
    assert_eq!(&header[0..12], &PP2_SIGNATURE);
    // ver_cmd: version 2 + PROXY
    assert_eq!(header[12], 0x21);
    // fam: AF_INET + STREAM
    assert_eq!(header[13], 0x11);
    // length: 12 (address block only, no TLVs)
    assert_eq!(&header[14..16], &[0x00, 0x0C]);

    // Source IP: 192.168.1.100
    assert_eq!(&header[16..20], &[192, 168, 1, 100]);
    // Destination IP: 10.244.1.23
    assert_eq!(&header[20..24], &[10, 244, 1, 23]);
    // Source port: 52341 = 0xCC75
    assert_eq!(&header[24..26], &[0xCC, 0x75]);
    // Destination port: 8443 = 0x20FB
    assert_eq!(&header[26..28], &[0x20, 0xFB]);
}

#[test]
fn test_ipv6_header_structure() {
    let src = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 12345, 0, 0));
    let dst = SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1),
        443,
        0,
        0,
    ));

    let header = ProxyProtocolV2Builder::new(src, dst).build();

    // 16 header + 36 addr = 52 bytes
    assert_eq!(header.len(), 52);

    assert_eq!(&header[0..12], &PP2_SIGNATURE);
    assert_eq!(header[12], 0x21); // ver 2 + PROXY
    assert_eq!(header[13], 0x21); // AF_INET6 + STREAM
    assert_eq!(&header[14..16], &[0x00, 0x24]); // length: 36

    // Source: ::1
    let mut expected_src = [0u8; 16];
    expected_src[15] = 1;
    assert_eq!(&header[16..32], &expected_src);

    // Destination: 2001:db8::1
    let mut expected_dst = [0u8; 16];
    expected_dst[0] = 0x20;
    expected_dst[1] = 0x01;
    expected_dst[2] = 0x0d;
    expected_dst[3] = 0xb8;
    expected_dst[15] = 1;
    assert_eq!(&header[32..48], &expected_dst);

    // Source port: 12345
    assert_eq!(&header[48..50], &12345u16.to_be_bytes());
    // Destination port: 443
    assert_eq!(&header[50..52], &443u16.to_be_bytes());
}

#[test]
fn test_authority_tlv() {
    let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 1, 5), 52341));
    let dst = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 244, 1, 23), 8443));

    let hostname = "FFJEK0X-443.sandbox.com".to_string();
    let mut builder = ProxyProtocolV2Builder::new(src, dst);
    builder.add_authority(&hostname);
    assert!(matches!(builder.tlvs[0].value, Cow::Borrowed(_)));
    assert_eq!(builder.tlvs[0].value.as_ptr(), hostname.as_ptr());
    let header = builder.build();
    assert_eq!(builder.build(), header);

    // 16 header + 12 addr + 3 tlv-header + 23 hostname = 54
    let expected_len = 16 + 12 + 3 + hostname.len();
    assert_eq!(header.len(), expected_len);

    // Payload length = 12 addr + 3 + 23 = 38
    let payload_len = u16::from_be_bytes([header[14], header[15]]);
    assert_eq!(payload_len as usize, 12 + 3 + hostname.len());

    // TLV starts at offset 28 (16 header + 12 addr)
    let tlv_offset = 28;
    assert_eq!(header[tlv_offset], PP2_TYPE_AUTHORITY);
    let tlv_len = u16::from_be_bytes([header[tlv_offset + 1], header[tlv_offset + 2]]);
    assert_eq!(tlv_len as usize, hostname.len());
    assert_eq!(
        &header[tlv_offset + 3..tlv_offset + 3 + hostname.len()],
        hostname.as_bytes()
    );
    drop(builder);
    drop(hostname);
    assert_eq!(&header[tlv_offset + 3..], b"FFJEK0X-443.sandbox.com");
}

#[test]
fn test_multiple_tlvs() {
    let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 1, 5), 52341));
    let dst = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 244, 1, 23), 8443));

    let hostname = "test.example.com";
    let alpn = b"h2";
    let mut builder = ProxyProtocolV2Builder::new(src, dst);
    builder.add_authority(hostname);
    let owned_alpn = alpn.to_vec();
    let alpn_ptr = owned_alpn.as_ptr();
    builder.add_tlv(PP2_TYPE_ALPN, owned_alpn);
    assert!(matches!(builder.tlvs[1].value, Cow::Owned(_)));
    assert_eq!(builder.tlvs[1].value.as_ptr(), alpn_ptr);
    let header = builder.build();

    let expected_len = 16 + 12 + (3 + hostname.len()) + (3 + alpn.len());
    assert_eq!(header.len(), expected_len);

    // First TLV: AUTHORITY
    let tlv1_offset = 28;
    assert_eq!(header[tlv1_offset], PP2_TYPE_AUTHORITY);

    // Second TLV: ALPN
    let tlv2_offset = tlv1_offset + 3 + hostname.len();
    assert_eq!(header[tlv2_offset], PP2_TYPE_ALPN);
    let tlv2_len = u16::from_be_bytes([header[tlv2_offset + 1], header[tlv2_offset + 2]]);
    assert_eq!(tlv2_len, 2);
    assert_eq!(&header[tlv2_offset + 3..tlv2_offset + 5], b"h2");
}

#[test]
fn test_mixed_v4_v6_maps_to_v6() {
    let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 1234));
    let dst = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 5678, 0, 0));

    let header = ProxyProtocolV2Builder::new(src, dst).build();

    // Should be AF_INET6 (mapped)
    assert_eq!(header[13], PP2_AF_INET6_STREAM);
    // 16 header + 36 addr = 52
    assert_eq!(header.len(), 52);

    // Source should be v4-mapped: ::ffff:192.168.1.1
    assert_eq!(&header[16..26], &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&header[26..28], &[0xFF, 0xFF]);
    assert_eq!(&header[28..32], &[192, 168, 1, 1]);
}

#[test]
fn test_mixed_v6_v4_maps_destination_to_v6() {
    let src = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 1234, 0, 0));
    let dst = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 5678));

    let header = ProxyProtocolV2Builder::new(src, dst).build();

    assert_eq!(header[13], PP2_AF_INET6_STREAM);
    assert_eq!(header.len(), 52);
    assert_eq!(&header[16..32], &Ipv6Addr::LOCALHOST.octets());
    assert_eq!(&header[32..42], &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&header[42..44], &[0xFF, 0xFF]);
    assert_eq!(&header[44..48], &[192, 0, 2, 1]);
    assert_eq!(&header[48..50], &1234u16.to_be_bytes());
    assert_eq!(&header[50..52], &5678u16.to_be_bytes());
}

#[test]
fn test_known_pp2_header_bytes() {
    // Validate against a known-good PP2 header:
    // src=127.0.0.1:1000 dst=127.0.0.1:2000, no TLVs
    let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1000));
    let dst = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 2000));

    let header = ProxyProtocolV2Builder::new(src, dst).build();

    let expected: Vec<u8> = vec![
        // Signature
        0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
        // ver_cmd: 0x21 (version 2, PROXY)
        0x21, // fam: 0x11 (AF_INET, STREAM)
        0x11, // length: 12
        0x00, 0x0C, // src IP: 127.0.0.1
        127, 0, 0, 1, // dst IP: 127.0.0.1
        127, 0, 0, 1, // src port: 1000 = 0x03E8
        0x03, 0xE8, // dst port: 2000 = 0x07D0
        0x07, 0xD0,
    ];

    assert_eq!(header, expected);
}

#[test]
fn empty_authority_preserves_zero_length_tlv() {
    let src = "127.0.0.1:1000".parse().unwrap();
    let dst = "127.0.0.1:2000".parse().unwrap();
    let mut builder = ProxyProtocolV2Builder::new(src, dst);
    builder.add_authority("");
    let header = builder.build();
    assert_eq!(header.len(), 31);
    assert_eq!(&header[14..16], &15u16.to_be_bytes());
    assert_eq!(&header[28..], &[PP2_TYPE_AUTHORITY, 0, 0]);
}
