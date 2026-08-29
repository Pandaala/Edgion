//! ACME DNS-01 propagation probe.
//!
//! Discovers the zone's authoritative nameservers (the system recursive
//! resolver is used for discovery only) and queries the challenge TXT record
//! against those servers directly, bypassing recursive-resolver negative
//! caching — the converged cert-manager / lego design. Controller-only,
//! cold-path consumer; uses the same hickory-resolver backend as the standard
//! resolver so no second DNS library enters the workspace.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use hickory_resolver::config::ResolverOpts;
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::net::{DnsError, NetError};
use hickory_resolver::proto::rr::{RData, Record};
use hickory_resolver::TokioResolver;

use super::hickory;

/// Outcome of one propagation check for a single TXT record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxtCheckOutcome {
    /// Every authoritative nameserver that responded returned the expected
    /// value, and at least one responded.
    Found,
    /// At least one authoritative nameserver responded without the expected
    /// value — definitively not (fully) propagated yet.
    NotFound,
    /// No definitive observation was possible: authoritative-nameserver
    /// discovery failed, or no authoritative server was reachable within the
    /// budget. A single unreachable server never aborts the check; only the
    /// absence of any definitive answer produces this outcome.
    Unreachable,
}

/// Per-nameserver observation, folded by [`aggregate_observations`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NsObservation {
    HasValue,
    LacksValue,
    Unreachable,
}

/// Check whether the TXT record at `fqdn` currently carries `expected` on the
/// zone's authoritative nameservers. Never runs past `budget`; on budget
/// exhaustion the round reports [`TxtCheckOutcome::Unreachable`].
pub async fn check_txt_propagation(fqdn: &str, expected: &str, budget: Duration) -> TxtCheckOutcome {
    match tokio::time::timeout(budget, check_txt_propagation_inner(fqdn, expected)).await {
        Ok(outcome) => outcome,
        Err(_elapsed) => TxtCheckOutcome::Unreachable,
    }
}

async fn check_txt_propagation_inner(fqdn: &str, expected: &str) -> TxtCheckOutcome {
    let Ok(recursive) = TokioResolver::builder_tokio().and_then(|builder| builder.build()) else {
        return TxtCheckOutcome::Unreachable;
    };
    let ips = discover_authoritative_ips(&recursive, fqdn).await;
    if ips.is_empty() {
        return TxtCheckOutcome::Unreachable;
    }
    let mut observations = Vec::with_capacity(ips.len());
    for ip in ips {
        observations.push(observe_ns(ip, fqdn, expected).await);
    }
    aggregate_observations(&observations)
}

/// Find the zone apex for `fqdn` (first ascending name with an SOA record),
/// then resolve the zone's NS host names to IPs. Discovery runs on the
/// recursive resolver; an empty result means "could not discover".
async fn discover_authoritative_ips(recursive: &TokioResolver, fqdn: &str) -> Vec<IpAddr> {
    let mut zone = None;
    for candidate in zone_candidates(fqdn) {
        if let Ok(soa) = recursive.soa_lookup(absolute(&candidate)).await {
            if has_soa_answer(soa.answers()) {
                zone = Some(candidate);
                break;
            }
        }
    }
    let Some(zone) = zone else {
        return Vec::new();
    };
    let Ok(ns) = recursive.ns_lookup(absolute(&zone)).await else {
        return Vec::new();
    };
    let mut ips: Vec<IpAddr> = Vec::new();
    for record in ns.answers() {
        if let RData::NS(ns_name) = &record.data {
            if let Ok(addrs) = recursive.lookup_ip(ns_name.0.to_utf8()).await {
                ips.extend(addrs.iter());
            }
        }
    }
    ips.sort_unstable();
    ips.dedup();
    ips
}

/// Return whether the answer section contains an SOA record. Hickory 0.26
/// preserves intermediate records, so a CNAME-only answer is not sufficient
/// evidence that the candidate is a zone apex.
fn has_soa_answer(answers: &[Record]) -> bool {
    answers.iter().any(|record| matches!(&record.data, RData::SOA(_)))
}

/// Query one authoritative nameserver directly for the TXT record.
async fn observe_ns(ip: IpAddr, fqdn: &str, expected: &str) -> NsObservation {
    let config = hickory::resolver_config(&[SocketAddr::new(ip, 53)]);
    // Tighter than the 5s x 2-attempt default so a few hanging nameservers
    // (e.g. unreachable AAAA endpoints on a dual-stack NS set) cannot eat the
    // whole per-round budget and mask answers already obtained from healthy
    // servers.
    let mut opts = ResolverOpts::default();
    opts.timeout = Duration::from_secs(3);
    opts.attempts = 1;
    let Ok(resolver) = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
        .with_options(opts)
        .build()
    else {
        return NsObservation::Unreachable;
    };
    match resolver.txt_lookup(absolute(fqdn)).await {
        Ok(lookup) => {
            let has = lookup.answers().iter().any(|record| {
                matches!(
                    &record.data,
                    RData::TXT(txt) if txt_record_matches(&txt.txt_data, expected)
                )
            });
            if has {
                NsObservation::HasValue
            } else {
                NsObservation::LacksValue
            }
        }
        Err(e) => classify_lookup_error(&e),
    }
}

/// Classify a failed TXT lookup. Only NXDOMAIN and NODATA (NoError) are a
/// definitive "record absent"; any other response code means the
/// server could not answer (lame delegation, DNSSEC breakage, a middlebox
/// answering REFUSED) and must count as unreachable, never as a definitive
/// miss — otherwise a REFUSED-answering restricted network would fail closed
/// instead of degrading to the optimistic legacy path.
fn classify_lookup_error(e: &NetError) -> NsObservation {
    use hickory_resolver::proto::op::ResponseCode;
    match e {
        NetError::Dns(DnsError::NoRecordsFound(no_records))
            if matches!(no_records.response_code, ResponseCode::NXDomain | ResponseCode::NoError) =>
        {
            NsObservation::LacksValue
        }
        _ => NsObservation::Unreachable,
    }
}

/// A TXT record's character-strings, concatenated, equal `expected`.
fn txt_record_matches(chunks: &[Box<[u8]>], expected: &str) -> bool {
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    if total != expected.len() {
        return false;
    }
    let joined: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
    joined == expected.as_bytes()
}

/// Fold per-nameserver observations into a round outcome. A definitive miss
/// on any responding server wins over hits (the ACME server may query any of
/// them); unreachable servers are ignored unless nothing responded at all.
fn aggregate_observations(observations: &[NsObservation]) -> TxtCheckOutcome {
    if observations.contains(&NsObservation::LacksValue) {
        TxtCheckOutcome::NotFound
    } else if observations.contains(&NsObservation::HasValue) {
        TxtCheckOutcome::Found
    } else {
        TxtCheckOutcome::Unreachable
    }
}

/// Ascending zone-cut candidates: the name itself, then each parent suffix
/// keeping at least two labels (a TLD/root SOA is never the right zone for an
/// ACME challenge name).
fn zone_candidates(fqdn: &str) -> Vec<String> {
    let name = fqdn.trim_end_matches('.');
    let labels: Vec<&str> = name.split('.').filter(|l| !l.is_empty()).collect();
    let mut out = Vec::new();
    for start in 0..labels.len() {
        if labels.len() - start < 2 {
            break;
        }
        out.push(labels[start..].join("."));
    }
    out
}

/// Absolute (trailing-dot) form for hickory lookups so the system resolver's
/// search-domain list is never appended to the challenge name.
fn absolute(name: &str) -> String {
    format!("{}.", name.trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_resolver::net::NoRecords;
    use hickory_resolver::proto::op::{Query, ResponseCode};
    use hickory_resolver::proto::rr::rdata::{CNAME, SOA};
    use hickory_resolver::proto::rr::{Name, RecordType};

    fn lookup_error(response_code: ResponseCode) -> NetError {
        let query = Query::query(
            Name::from_ascii("_acme-challenge.example.com.").unwrap(),
            RecordType::TXT,
        );
        DnsError::NoRecordsFound(NoRecords::new(query, response_code)).into()
    }

    #[test]
    fn zone_candidates_walk_up_keeping_two_labels() {
        assert_eq!(
            zone_candidates("_acme-challenge.www.example.com"),
            vec![
                "_acme-challenge.www.example.com".to_string(),
                "www.example.com".to_string(),
                "example.com".to_string(),
            ]
        );
        assert_eq!(
            zone_candidates("_acme-challenge.example.com."),
            vec!["_acme-challenge.example.com".to_string(), "example.com".to_string()]
        );
        assert!(zone_candidates("com").is_empty());
    }

    #[test]
    fn absolute_appends_single_trailing_dot() {
        assert_eq!(absolute("example.com"), "example.com.");
        assert_eq!(absolute("example.com."), "example.com.");
    }

    #[test]
    fn txt_record_matches_joins_character_strings() {
        let single: Vec<Box<[u8]>> = vec![b"token-value".to_vec().into_boxed_slice()];
        assert!(txt_record_matches(&single, "token-value"));

        let split: Vec<Box<[u8]>> = vec![
            b"token-".to_vec().into_boxed_slice(),
            b"value".to_vec().into_boxed_slice(),
        ];
        assert!(txt_record_matches(&split, "token-value"));

        assert!(!txt_record_matches(&single, "other-value"));
        assert!(!txt_record_matches(&[], "token-value"));
    }

    #[test]
    fn soa_detection_ignores_intermediate_cname_answers() {
        let owner = Name::from_ascii("alias.example.com.").unwrap();
        let target = Name::from_ascii("example.com.").unwrap();
        let cname = Record::from_rdata(owner, 60, RData::CNAME(CNAME(target.clone())));
        assert!(!has_soa_answer(&[cname]));

        let soa = SOA::new(
            Name::from_ascii("ns.example.com.").unwrap(),
            Name::from_ascii("hostmaster.example.com.").unwrap(),
            1,
            3600,
            600,
            86400,
            60,
        );
        let soa = Record::from_rdata(target, 60, RData::SOA(soa));
        assert!(has_soa_answer(&[soa]));
    }

    #[test]
    fn aggregate_miss_beats_hit_and_unreachable_is_last_resort() {
        use NsObservation::*;
        assert_eq!(aggregate_observations(&[HasValue, HasValue]), TxtCheckOutcome::Found);
        assert_eq!(
            aggregate_observations(&[HasValue, LacksValue]),
            TxtCheckOutcome::NotFound
        );
        assert_eq!(
            aggregate_observations(&[Unreachable, LacksValue]),
            TxtCheckOutcome::NotFound
        );
        assert_eq!(aggregate_observations(&[Unreachable, HasValue]), TxtCheckOutcome::Found);
        assert_eq!(
            aggregate_observations(&[Unreachable, Unreachable]),
            TxtCheckOutcome::Unreachable
        );
        assert_eq!(aggregate_observations(&[]), TxtCheckOutcome::Unreachable);
    }

    #[test]
    fn classifies_only_nxdomain_and_nodata_as_definitive_misses() {
        assert_eq!(
            classify_lookup_error(&lookup_error(ResponseCode::NXDomain)),
            NsObservation::LacksValue
        );
        assert_eq!(
            classify_lookup_error(&lookup_error(ResponseCode::NoError)),
            NsObservation::LacksValue
        );
        assert_eq!(
            classify_lookup_error(&NetError::Dns(DnsError::ResponseCode(ResponseCode::ServFail))),
            NsObservation::Unreachable
        );
        assert_eq!(
            classify_lookup_error(&NetError::Dns(DnsError::ResponseCode(ResponseCode::Refused))),
            NsObservation::Unreachable
        );
    }
}
