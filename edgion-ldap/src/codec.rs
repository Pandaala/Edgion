//! Sans-IO BER subset used by LDAP (RFC 4511 section 5.1).
//!
//! Only single-octet identifiers and definite lengths of at most four octets
//! are accepted. `Reader` is a flat cursor: it hands out sub-readers over
//! element contents and never builds a tree or calls itself.

use crate::Error;

/// Maximum content length of one inbound LDAPMessage frame.
pub const MAX_FRAME: usize = 64 * 1024;

pub(crate) const INTEGER: u8 = 0x02;
pub(crate) const OCTET_STRING: u8 = 0x04;
pub(crate) const ENUMERATED: u8 = 0x0a;
pub(crate) const SEQUENCE: u8 = 0x30;
#[cfg(feature = "client")]
pub(crate) const BIND_REQUEST: u8 = 0x60;
pub(crate) const BIND_RESPONSE: u8 = 0x61;
#[cfg(feature = "client")]
pub(crate) const SEARCH_REQUEST: u8 = 0x63;
pub(crate) const SEARCH_RESULT_ENTRY: u8 = 0x64;
pub(crate) const SEARCH_RESULT_DONE: u8 = 0x65;
pub(crate) const SEARCH_RESULT_REFERENCE: u8 = 0x73;
pub(crate) const SET: u8 = 0x31;
#[cfg(feature = "client")]
pub(crate) const UNBIND_REQUEST: u8 = 0x42;
#[cfg(feature = "client")]
pub(crate) const EXTENDED_REQUEST: u8 = 0x77;
pub(crate) const EXTENDED_RESPONSE: u8 = 0x78;
#[cfg(feature = "client")]
pub(crate) const CONTEXT_0_PRIMITIVE: u8 = 0x80;

/// Low five identifier bits set to one select the high-tag-number form.
const HIGH_TAG_NUMBER: u8 = 0x1f;

/// Number of length octets that follow a long-form initial length octet.
fn long_form_octets(first: u8) -> Result<usize, Error> {
    match first {
        0x00..=0x7f => Ok(0),
        0x81..=0x84 => Ok(usize::from(first & 0x7f)),
        _ => Err(Error::InvalidLength),
    }
}

/// Length value from the initial length octet and its 0-4 extra octets.
fn length_value(first: u8, octets: &[u8]) -> usize {
    if octets.is_empty() {
        usize::from(first)
    } else {
        octets
            .iter()
            .fold(0usize, |acc, &octet| (acc << 8) | usize::from(octet))
    }
}

/// Validate the identifier of an inbound frame and return how many extra
/// length octets (0-4) follow its initial length octet.
///
/// The identifier is checked first, so a non-LDAP byte stream (for example a
/// TLS record on a plain port) is reported as [`Error::UnexpectedTag`].
pub fn length_octets(tag: u8, first: u8) -> Result<usize, Error> {
    if tag != SEQUENCE {
        return Err(Error::UnexpectedTag {
            expected: SEQUENCE,
            actual: tag,
        });
    }
    long_form_octets(first)
}

/// Decode a frame's content length from its initial length octet and the
/// extra octets announced by [`length_octets`], enforcing [`MAX_FRAME`].
pub fn frame_content_len(first: u8, extra: &[u8]) -> Result<usize, Error> {
    // A count that disagrees with the initial octet is caller misuse.
    if extra.len() != long_form_octets(first)? {
        return Err(Error::InvalidLength);
    }
    let declared = length_value(first, extra);
    if declared > MAX_FRAME {
        return Err(Error::FrameTooLarge {
            declared,
            max: MAX_FRAME,
        });
    }
    Ok(declared)
}

/// Flat cursor over the content of one element.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn take_byte(&mut self) -> Result<u8, Error> {
        let byte = *self.buf.get(self.pos).ok_or(Error::Truncated)?;
        self.pos += 1;
        Ok(byte)
    }

    /// Read one identifier and length; the content must fit in the remainder.
    pub(crate) fn read_header(&mut self) -> Result<(u8, usize), Error> {
        let tag = self.take_byte()?;
        if tag & HIGH_TAG_NUMBER == HIGH_TAG_NUMBER {
            return Err(Error::UnsupportedTag);
        }
        let first = self.take_byte()?;
        let extra = long_form_octets(first)?;
        let end = self.pos.checked_add(extra).ok_or(Error::Truncated)?;
        let octets = self.buf.get(self.pos..end).ok_or(Error::Truncated)?;
        self.pos = end;
        let len = length_value(first, octets);
        if len > self.buf.len() - self.pos {
            return Err(Error::Truncated);
        }
        Ok((tag, len))
    }

    /// Read the next element's identifier and content.
    pub(crate) fn element(&mut self) -> Result<(u8, &'a [u8]), Error> {
        let (tag, len) = self.read_header()?;
        let end = self.pos + len;
        let content = self.buf.get(self.pos..end).ok_or(Error::Truncated)?;
        self.pos = end;
        Ok((tag, content))
    }

    /// Read the next element, which must carry `expected`, and return its content.
    pub(crate) fn element_with(&mut self, expected: u8) -> Result<&'a [u8], Error> {
        let (actual, content) = self.element()?;
        if actual != expected {
            return Err(Error::UnexpectedTag { expected, actual });
        }
        Ok(content)
    }

    /// Return a reader over the content of the next element, which must carry `tag`.
    pub(crate) fn expect(&mut self, tag: u8) -> Result<Reader<'a>, Error> {
        self.element_with(tag).map(Reader::new)
    }

    /// Skip the next element by its declared length without descending into it.
    pub(crate) fn skip_element(&mut self) -> Result<(), Error> {
        self.element().map(|_| ())
    }

    /// Read a non-negative INTEGER or ENUMERATED that fits in 32 bits.
    pub(crate) fn read_u32(&mut self, tag: u8) -> Result<u32, Error> {
        let content = self.element_with(tag)?;
        let (&first, _) = content.split_first().ok_or(Error::InvalidInteger)?;
        let fits = content.len() <= 4 || (content.len() == 5 && first == 0);
        if first & 0x80 != 0 || !fits {
            return Err(Error::InvalidInteger);
        }
        let value = content.iter().fold(0u64, |acc, &octet| (acc << 8) | u64::from(octet));
        u32::try_from(value).map_err(|_| Error::InvalidInteger)
    }

    /// Validate the next element as a primitive OCTET STRING holding UTF-8, then drop it.
    pub(crate) fn read_utf8_octets(&mut self) -> Result<(), Error> {
        let content = self.element_with(OCTET_STRING)?;
        std::str::from_utf8(content).map(|_| ()).map_err(|_| Error::InvalidUtf8)
    }
}

/// Bytes needed to encode `len` in the minimal definite form.
pub(crate) fn header_len(len: usize) -> usize {
    if len < 0x80 {
        2
    } else {
        2 + long_length_octets(len)
    }
}

fn long_length_octets(len: usize) -> usize {
    let bits = usize::BITS - len.leading_zeros();
    bits.div_ceil(8) as usize
}

#[cfg(feature = "client")]
/// Total encoded size of an element whose content is `len` bytes.
pub(crate) fn element_len(len: usize) -> usize {
    header_len(len) + len
}

#[cfg(feature = "client")]
/// Content length of the minimal two's-complement encoding of `value`.
pub(crate) fn u32_content_len(value: u32) -> usize {
    let bits = (u32::BITS - value.leading_zeros()) as usize;
    bits / 8 + 1
}

pub(crate) fn write_header(out: &mut Vec<u8>, tag: u8, len: usize) {
    out.push(tag);
    if len < 0x80 {
        out.push(len as u8);
        return;
    }
    let octets = long_length_octets(len);
    out.push(0x80 | octets as u8);
    for index in (0..octets).rev() {
        out.push((len >> (8 * index)) as u8);
    }
}

#[cfg(feature = "client")]
pub(crate) fn write_u32(out: &mut Vec<u8>, tag: u8, value: u32) {
    let len = u32_content_len(value);
    write_header(out, tag, len);
    for index in (0..len).rev() {
        out.push((u64::from(value) >> (8 * index)) as u8);
    }
}

#[cfg(feature = "client")]
pub(crate) fn write_octets(out: &mut Vec<u8>, tag: u8, bytes: &[u8]) {
    write_header(out, tag, bytes.len());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
#[path = "../tests/unit/codec_test.rs"]
mod tests;
