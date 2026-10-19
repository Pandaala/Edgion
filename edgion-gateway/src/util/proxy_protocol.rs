//! Proxy Protocol v2 encoder
//!
//! Implements the HAProxy Proxy Protocol v2 binary format for encoding
//! connection metadata (source/destination addresses) and TLV extensions.
//! See: <https://www.haproxy.org/download/2.2/doc/proxy-protocol.txt>

use std::borrow::Cow;
use std::net::{IpAddr, SocketAddr};

/// PP2 12-byte signature that starts every v2 header
pub const PP2_SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];

pub const PP2_VERSION: u8 = 0x20;
pub const PP2_CMD_PROXY: u8 = 0x01;

/// AF_INET (0x1) + SOCK_STREAM (0x1)
pub const PP2_AF_INET_STREAM: u8 = 0x11;
/// AF_INET6 (0x2) + SOCK_STREAM (0x1)
pub const PP2_AF_INET6_STREAM: u8 = 0x21;

pub const PP2_TYPE_ALPN: u8 = 0x01;
/// SNI hostname (HAProxy standard: "server_name" extension / authority)
pub const PP2_TYPE_AUTHORITY: u8 = 0x02;

/// A single Type-Length-Value entry
pub struct Tlv<'a> {
    pub tlv_type: u8,
    pub value: Cow<'a, [u8]>,
}

impl Tlv<'_> {
    pub fn new(tlv_type: u8, value: Vec<u8>) -> Self {
        Self {
            tlv_type,
            value: Cow::Owned(value),
        }
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
pub struct ProxyProtocolV2Builder<'a> {
    src: SocketAddr,
    dst: SocketAddr,
    tlvs: Vec<Tlv<'a>>,
}

impl<'a> ProxyProtocolV2Builder<'a> {
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
    /// The hostname remains borrowed until the builder's last use; `build`
    /// returns an independently owned header.
    pub fn add_authority(&mut self, hostname: &'a str) -> &mut Self {
        self.tlvs.push(Tlv {
            tlv_type: PP2_TYPE_AUTHORITY,
            value: Cow::Borrowed(hostname.as_bytes()),
        });
        self
    }

    fn encode_address_block(&self, buf: &mut Vec<u8>) {
        match (self.src.ip(), self.dst.ip()) {
            (IpAddr::V4(src_ip), IpAddr::V4(dst_ip)) => {
                buf.extend_from_slice(&src_ip.octets());
                buf.extend_from_slice(&dst_ip.octets());
            }
            (src_ip, dst_ip) => {
                let src_ip = match src_ip {
                    IpAddr::V4(ip) => ip.to_ipv6_mapped(),
                    IpAddr::V6(ip) => ip,
                };
                let dst_ip = match dst_ip {
                    IpAddr::V4(ip) => ip.to_ipv6_mapped(),
                    IpAddr::V6(ip) => ip,
                };
                buf.extend_from_slice(&src_ip.octets());
                buf.extend_from_slice(&dst_ip.octets());
            }
        }
        buf.extend_from_slice(&self.src.port().to_be_bytes());
        buf.extend_from_slice(&self.dst.port().to_be_bytes());
    }

    /// Encode the complete PP2 binary header
    pub fn build(&self) -> Vec<u8> {
        let (af_proto, addr_len) = match (self.src.ip(), self.dst.ip()) {
            (IpAddr::V4(_), IpAddr::V4(_)) => (PP2_AF_INET_STREAM, 12u16),
            _ => (PP2_AF_INET6_STREAM, 36u16),
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
        self.encode_address_block(&mut buf);
        // TLVs
        for tlv in &self.tlvs {
            tlv.encode(&mut buf);
        }

        buf
    }
}

#[cfg(test)]
#[path = "../../tests/unit/util/proxy_protocol_test.rs"]
mod tests;
