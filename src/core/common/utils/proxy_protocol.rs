//! Proxy Protocol v2 encoder
//!
//! Implements the HAProxy Proxy Protocol v2 binary format for encoding
//! connection metadata (source/destination addresses) and TLV extensions.
//! See: <https://www.haproxy.org/download/2.2/doc/proxy-protocol.txt>

use std::net::{IpAddr, SocketAddr};

/// PP2 12-byte signature that starts every v2 header
pub const PP2_SIGNATURE: [u8; 12] = [
    0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
];

pub const PP2_VERSION: u8 = 0x20;
pub const PP2_CMD_PROXY: u8 = 0x01;

/// AF_INET (0x1) + SOCK_STREAM (0x1)
pub const PP2_AF_INET_STREAM: u8 = 0x11;
/// AF_INET6 (0x2) + SOCK_STREAM (0x1)
pub const PP2_AF_INET6_STREAM: u8 = 0x21;

pub const PP2_TYPE_ALPN: u8 = 0x01;
/// SNI hostname (HAProxy standard: "server_name" extension / authority)
pub const PP2_TYPE_AUTHORITY: u8 = 0x02;
pub const PP2_TYPE_CRC32C: u8 = 0x03;
pub const PP2_TYPE_NOOP: u8 = 0x04;
pub const PP2_TYPE_UNIQUE_ID: u8 = 0x05;
pub const PP2_TYPE_SSL: u8 = 0x20;

/// A single Type-Length-Value entry
pub struct Tlv {
    pub tlv_type: u8,
    pub value: Vec<u8>,
}

impl Tlv {
    pub fn new(tlv_type: u8, value: Vec<u8>) -> Self {
        Self { tlv_type, value }
    }

    fn encoded_len(&self) -> usize {
        // type(1) + length(2) + value
        3 + self.value.len()
    }

    fn encode(&self, buf: &mut Vec<u8>) {
        buf.push(self.tlv_type);
        let len = self.value.len() as u16;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&self.value);
    }
}

/// Builder for Proxy Protocol v2 headers
pub struct ProxyProtocolV2Builder {
    src: SocketAddr,
    dst: SocketAddr,
    tlvs: Vec<Tlv>,
}

impl ProxyProtocolV2Builder {
    pub fn new(src: SocketAddr, dst: SocketAddr) -> Self {
        Self {
            src,
            dst,
            tlvs: Vec::new(),
        }
    }

    pub fn add_tlv(&mut self, tlv_type: u8, value: Vec<u8>) -> &mut Self {
        self.tlvs.push(Tlv::new(tlv_type, value));
        self
    }

    /// Convenience: add PP2_TYPE_AUTHORITY TLV containing the SNI hostname
    pub fn add_authority(&mut self, hostname: &str) -> &mut Self {
        self.add_tlv(PP2_TYPE_AUTHORITY, hostname.as_bytes().to_vec())
    }

    /// Encode the complete PP2 binary header
    pub fn build(&self) -> Vec<u8> {
        let (af_proto, addr_len, addr_bytes) = match (self.src.ip(), self.dst.ip()) {
            (IpAddr::V4(src_ip), IpAddr::V4(dst_ip)) => {
                let mut addr = Vec::with_capacity(12);
                addr.extend_from_slice(&src_ip.octets()); // 4 bytes
                addr.extend_from_slice(&dst_ip.octets()); // 4 bytes
                addr.extend_from_slice(&self.src.port().to_be_bytes()); // 2 bytes
                addr.extend_from_slice(&self.dst.port().to_be_bytes()); // 2 bytes
                (PP2_AF_INET_STREAM, 12u16, addr)
            }
            (IpAddr::V6(src_ip), IpAddr::V6(dst_ip)) => {
                let mut addr = Vec::with_capacity(36);
                addr.extend_from_slice(&src_ip.octets()); // 16 bytes
                addr.extend_from_slice(&dst_ip.octets()); // 16 bytes
                addr.extend_from_slice(&self.src.port().to_be_bytes()); // 2 bytes
                addr.extend_from_slice(&self.dst.port().to_be_bytes()); // 2 bytes
                (PP2_AF_INET6_STREAM, 36u16, addr)
            }
            // Mixed address families: map v4 to v4-mapped v6
            (IpAddr::V4(src_v4), IpAddr::V6(dst_ip)) => {
                let src_ip = src_v4.to_ipv6_mapped();
                let mut addr = Vec::with_capacity(36);
                addr.extend_from_slice(&src_ip.octets());
                addr.extend_from_slice(&dst_ip.octets());
                addr.extend_from_slice(&self.src.port().to_be_bytes());
                addr.extend_from_slice(&self.dst.port().to_be_bytes());
                (PP2_AF_INET6_STREAM, 36u16, addr)
            }
            (IpAddr::V6(src_ip), IpAddr::V4(dst_v4)) => {
                let dst_ip = dst_v4.to_ipv6_mapped();
                let mut addr = Vec::with_capacity(36);
                addr.extend_from_slice(&src_ip.octets());
                addr.extend_from_slice(&dst_ip.octets());
                addr.extend_from_slice(&self.src.port().to_be_bytes());
                addr.extend_from_slice(&self.dst.port().to_be_bytes());
                (PP2_AF_INET6_STREAM, 36u16, addr)
            }
        };

        let tlv_total: usize = self.tlvs.iter().map(|t| t.encoded_len()).sum();
        let payload_len = addr_len as usize + tlv_total;

        // 16-byte header + address block + TLVs
        let mut buf = Vec::with_capacity(16 + payload_len);

        // Signature (12 bytes)
        buf.extend_from_slice(&PP2_SIGNATURE);
        // Version (0x2) | Command (PROXY = 0x1)
        buf.push(PP2_VERSION | PP2_CMD_PROXY);
        // Address family + protocol
        buf.push(af_proto);
        // Payload length (address block + TLVs) in network byte order
        buf.extend_from_slice(&(payload_len as u16).to_be_bytes());
        // Address block
        buf.extend_from_slice(&addr_bytes);
        // TLVs
        for tlv in &self.tlvs {
            tlv.encode(&mut buf);
        }

        buf
    }
}

#[cfg(test)]
mod tests {
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

        let hostname = "FFJEK0X-443.sandbox.com";
        let mut builder = ProxyProtocolV2Builder::new(src, dst);
        builder.add_authority(hostname);
        let header = builder.build();

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
    }

    #[test]
    fn test_multiple_tlvs() {
        let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 1, 5), 52341));
        let dst = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 244, 1, 23), 8443));

        let hostname = "test.example.com";
        let alpn = b"h2";
        let mut builder = ProxyProtocolV2Builder::new(src, dst);
        builder.add_authority(hostname);
        builder.add_tlv(PP2_TYPE_ALPN, alpn.to_vec());
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
            0x21,
            // fam: 0x11 (AF_INET, STREAM)
            0x11,
            // length: 12
            0x00, 0x0C,
            // src IP: 127.0.0.1
            127, 0, 0, 1,
            // dst IP: 127.0.0.1
            127, 0, 0, 1,
            // src port: 1000 = 0x03E8
            0x03, 0xE8,
            // dst port: 2000 = 0x07D0
            0x07, 0xD0,
        ];

        assert_eq!(header, expected);
    }
}
