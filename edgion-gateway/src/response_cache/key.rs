//! One bounded normalization/framing policy for effective cache inputs.

use crate::request_trace_id::RequestTraceId;
use http::{HeaderMap, HeaderName};
use sha2::{Digest, Sha256};

pub(super) fn authority_is_eligible(
    compiled: &edgion_resources::resources::upstream_authority::CompiledUpstreamAuthority,
) -> bool {
    compiled.suffix.len() <= 253
        && compiled.label_template.segments().iter().all(|segment| {
            matches!(
                segment,
                edgion_resources::resources::common::request_template::TemplateSegment::Literal(_)
            )
        })
}

pub(super) fn authority_stamp(result: Option<&str>) -> Option<[u8; 32]> {
    let mut key = Builder::new();
    key.number(u64::from(result.is_some()))?;
    if let Some(result) = result {
        if result.len() > 316 {
            return None;
        }
        key.frame(result.as_bytes())?;
    }
    Some(key.finish())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiscoveryStamp {
    pub fingerprint: [u8; 32],
    pub membership: u64,
    pub service: [u8; 32],
    pub section: [u8; 32],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrafficStamp {
    pub generation: u64,
    pub result: Option<[u8; 32]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TlsResult {
    Absent,
    Invalid,
    Effective([u8; 32]),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TlsStamp {
    pub generation: u64,
    pub discovery: Option<DiscoveryStamp>,
    pub result: TlsResult,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OriginStamp {
    pub discovery: DiscoveryStamp,
    pub traffic: TrafficStamp,
    pub tls: TlsStamp,
}

pub(crate) fn discovery_stamp(
    target: &crate::backends::runtime::CompiledServiceTarget,
    available: &crate::backends::discovery::AvailableServiceProjection,
    section: &crate::backends::discovery::ResolvedServiceSection,
) -> Option<DiscoveryStamp> {
    use crate::backends::discovery::ServiceSectionKey;
    let section = &section.identity;
    let mut service = Builder::new();
    service.frame(target.namespace().as_bytes())?;
    service.frame(target.name().as_bytes())?;
    match target.section() {
        ServiceSectionKey::Number(number) => {
            service.frame(b"number")?;
            service.number(u64::from(number.get()))?;
        }
        ServiceSectionKey::Name(name) => {
            service.frame(b"name")?;
            service.frame(name.as_bytes())?;
        }
    }
    let mut resolved = Builder::new();
    resolved.number(u64::from(section.number.get()))?;
    resolved.frame(section.name.as_deref().unwrap_or("").as_bytes())?;
    resolved.frame(section.protocol.as_bytes())?;
    resolved.frame(section.app_protocol.as_deref().unwrap_or("").as_bytes())?;
    Some(DiscoveryStamp {
        fingerprint: *available.content_fingerprint(),
        membership: available.membership_identity,
        service: service.finish(),
        section: resolved.finish(),
    })
}

pub(crate) fn traffic_stamp(
    generation: u64,
    policy: Option<&crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy>,
) -> Option<TrafficStamp> {
    let result = if let Some(policy) = policy {
        let mut value = Builder::new();
        value.frame(policy.winning_policy_key.as_bytes())?;
        Some(value.finish())
    } else {
        None
    };
    Some(TrafficStamp { generation, result })
}

pub(crate) fn tls_result(lookup: &crate::backends::policy::BackendTlsPolicyLookup) -> TlsResult {
    use crate::backends::policy::BackendTlsPolicyLookup;
    match lookup {
        BackendTlsPolicyLookup::Absent => TlsResult::Absent,
        BackendTlsPolicyLookup::Invalid => TlsResult::Invalid,
        BackendTlsPolicyLookup::Effective(policy) => TlsResult::Effective(policy.profile.fingerprint),
    }
}

pub(super) const MATERIAL_LIMIT: usize = 64 * 1024;
pub(super) const WORK_LIMIT: usize = 2 * MATERIAL_LIMIT + 4096;

pub(super) struct HeaderCertificate {
    names: [[u8; 32]; 80],
    used: usize,
}
impl Default for HeaderCertificate {
    fn default() -> Self {
        Self {
            names: [[0; 32]; 80],
            used: 0,
        }
    }
}
impl HeaderCertificate {
    pub(super) fn add(&mut self, name: &str) -> Option<()> {
        if self.used == self.names.len() {
            return None;
        }
        let digest = header_digest(name);
        if !self.names[..self.used].contains(&digest) {
            self.names[self.used] = digest;
            self.used += 1;
        }
        Some(())
    }
    pub(super) fn permits(&self, vary: &VaryCertificate) -> bool {
        vary.names[..vary.used]
            .iter()
            .all(|name| self.names[..self.used].contains(name))
    }
}

#[derive(Clone, Copy)]
pub(super) struct VaryCertificate {
    names: [[u8; 32]; 16],
    used: usize,
}
impl VaryCertificate {
    pub(super) fn parse(headers: &HeaderMap) -> Option<Self> {
        let mut certificate = Self {
            names: [[0; 32]; 16],
            used: 0,
        };
        let mut bytes = 0usize;
        for value in headers.get_all("vary") {
            bytes = bytes.checked_add(value.len())?;
            if bytes > super::policy::MAX_METADATA {
                return None;
            }
            for name in value.to_str().ok()?.split(',') {
                let name = name.trim_matches(|c| c == ' ' || c == '\t');
                if name == "*" || !edgion_resources::gwapi_types::is_valid_header_token(name) {
                    return None;
                }
                let digest = header_digest(name);
                if certificate.names[..certificate.used].contains(&digest) {
                    continue;
                }
                if certificate.used == certificate.names.len() {
                    return None;
                }
                certificate.names[certificate.used] = digest;
                certificate.used += 1;
            }
        }
        certificate.names[..certificate.used].sort_unstable();
        Some(certificate)
    }
    pub(super) fn is_empty(&self) -> bool {
        self.used == 0
    }
    pub(super) fn matches(&self, headers: &HeaderMap) -> bool {
        Self::parse(headers)
            .is_some_and(|other| self.used == other.used && self.names[..self.used] == other.names[..other.used])
    }
}
fn header_digest(name: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"edgion response cache header name\0");
    let mut lowercase = [0u8; 64];
    for chunk in name.as_bytes().chunks(lowercase.len()) {
        let output = &mut lowercase[..chunk.len()];
        output.copy_from_slice(chunk);
        output.make_ascii_lowercase();
        digest.update(output);
    }
    digest.finalize().into()
}

pub(super) struct Exclusions<'a> {
    connection: [&'a str; 16],
    connection_count: usize,
    trace: &'a RequestTraceId,
    testing: bool,
}
impl<'a> Exclusions<'a> {
    pub(super) fn parse(headers: &'a HeaderMap, trace: &'a RequestTraceId, testing: bool) -> Option<Self> {
        let mut exclusions = Self {
            connection: [""; 16],
            connection_count: 0,
            trace,
            testing,
        };
        let mut values = headers.get_all("connection").iter();
        if let Some(value) = values.next() {
            if values.next().is_some() || value.len() > 512 {
                return None;
            }
            let value = value.to_str().ok()?;
            // Validate the bounded grammar before indexing borrowed canonical tokens.
            // Empty options and internal whitespace bypass.
            let mut options = 1;
            let mut token = false;
            let mut trailing_space = false;
            for byte in value.bytes() {
                match byte {
                    b' ' | b'\t' => {
                        if token {
                            trailing_space = true;
                        }
                    }
                    b',' => {
                        if !token || options == 16 {
                            return None;
                        }
                        options += 1;
                        token = false;
                        trailing_space = false;
                    }
                    _ => {
                        if trailing_space
                            || !edgion_resources::gwapi_types::is_valid_header_token(std::str::from_utf8(&[byte]).ok()?)
                        {
                            return None;
                        }
                        token = true;
                    }
                }
            }
            if !token {
                return None;
            }
            for token in crate::plugins::http::common::http_client::connection_option_tokens(value) {
                if exclusions.connection[..exclusions.connection_count]
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(token))
                {
                    continue;
                }
                *exclusions.connection.get_mut(exclusions.connection_count)? = token;
                exclusions.connection_count += 1;
            }
        }
        Some(exclusions)
    }
    pub(super) fn contains(&self, name: &str) -> bool {
        crate::plugins::http::common::HOP_BY_HOP_HEADERS.contains(&name)
            || matches!(
                name,
                "host" | "content-length" | "proxy-connection" | "traceparent" | "tracestate"
            )
            || (self.testing && name == "access_log")
            || self
                .trace
                .managed_header_names()
                .any(|managed| managed.as_str() == name)
            || self.connection[..self.connection_count]
                .iter()
                .any(|token| name.eq_ignore_ascii_case(token))
    }
}
pub(super) struct Builder {
    hash: Sha256,
    used: usize,
}

impl Builder {
    pub(super) fn new() -> Self {
        let mut hash = Sha256::new();
        hash.update(b"edgion bounded public response cache/v1\0");
        Self { hash, used: 0 }
    }

    pub(super) fn frame(&mut self, value: &[u8]) -> Option<()> {
        self.used = self.used.checked_add(8)?.checked_add(value.len())?;
        if self.used > MATERIAL_LIMIT {
            return None;
        }
        self.hash.update(u64::try_from(value.len()).ok()?.to_be_bytes());
        self.hash.update(value);
        Some(())
    }

    pub(super) fn number(&mut self, value: u64) -> Option<()> {
        self.frame(&value.to_be_bytes())
    }

    pub(super) fn headers(
        &mut self,
        headers: &HeaderMap,
        trace: &RequestTraceId,
        testing: bool,
    ) -> Option<HeaderCertificate> {
        if headers.len() > super::policy::MAX_FIELDS {
            return None;
        }
        let exclusions = Exclusions::parse(headers, trace, testing)?;
        let mut names: [Option<&HeaderName>; super::policy::MAX_FIELDS] = [None; super::policy::MAX_FIELDS];
        let mut count = 0;
        for name in headers.keys() {
            if exclusions.contains(name.as_str()) {
                continue;
            }
            names[count] = Some(name);
            count += 1;
        }
        names[..count].sort_unstable_by(|left, right| left.unwrap().as_str().cmp(right.unwrap().as_str()));
        let mut certificate = HeaderCertificate::default();
        self.number(count as u64)?;
        for name in names[..count].iter().flatten() {
            certificate.add(name.as_str())?;
            self.frame(name.as_str().as_bytes())?;
            let values = headers.get_all(*name);
            self.number(values.iter().count() as u64)?;
            for value in values {
                self.frame(value.as_bytes())?;
            }
        }
        Some(certificate)
    }

    pub(super) fn finish(self) -> [u8; 32] {
        self.hash.finalize().into()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/response_cache/key_test.rs"]
mod tests;
