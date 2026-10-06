//! Audited unary capacity ledger and checked controlled-buffer growth.
use super::super::super::config::UnaryConfig;
use edgion_resources::{duration::parse_duration, resources::common::parse_byte_size};
use std::time::Duration;
const MIB: usize = 1024 * 1024;
// The ledger depends on the pinned Rust/serde_json/http implementations, not just these guards.
const _: () = {
    assert!(std::mem::size_of::<usize>() == 8);
    assert!(std::mem::size_of::<serde_json::Value>() <= 32);
    assert!(std::mem::align_of::<serde_json::Value>() <= 8);
    assert!(std::mem::size_of::<String>() <= 24);
    assert!(std::mem::size_of::<bytes::Bytes>() <= 32);
    assert!(std::mem::size_of::<pingora_core::protocols::http::HttpTask>() <= 64);
    assert!(std::mem::size_of::<http::HeaderName>() <= 32);
    assert!(std::mem::size_of::<http::HeaderValue>() <= 40);
    assert!(std::mem::size_of::<serde_json::Number>() <= 16);
    assert!(std::mem::size_of::<serde_json::Map<String, serde_json::Value>>() <= 32);
};
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnaryLimits {
    pub(crate) input: usize,
    pub(crate) output: usize,
    pub(crate) timeout: Duration,
    pub(crate) depth: usize,
    pub(crate) nodes: usize,
    pub(crate) key: usize,
    pub(crate) events: usize,
    pub(crate) metadata: usize,
    pub(crate) lexical_events: usize,
    pub(crate) lexical_work: usize,
    pub(crate) reservation: usize,
    pub(crate) slots: usize,
}
fn size(raw: &str, name: &str, max: u64, minimum: u64) -> Result<usize, String> {
    let value = parse_byte_size(raw).ok_or_else(|| format!("Invalid unary {name}; use a positive byte size"))?;
    if value < minimum || value > max {
        return Err(format!("Unary {name} exceeds its audited range"));
    }
    usize::try_from(value).map_err(|_| format!("Unary {name} exceeds platform capacity"))
}
fn sum(values: &[usize]) -> Result<usize, String> {
    values
        .iter()
        .try_fold(0usize, |a, b| a.checked_add(*b))
        .ok_or_else(overflow)
}
fn mul(a: usize, b: usize) -> Result<usize, String> {
    a.checked_mul(b).ok_or_else(overflow)
}
fn overflow() -> String {
    "Unary capacity ledger overflow; reduce configured limits".into()
}
fn c(x: usize) -> Result<usize, String> {
    sum(&[mul(2, x)?, 8])
}
fn p(x: usize) -> Result<usize, String> {
    sum(&[mul(3, x)?, 16])
}
impl UnaryLimits {
    pub(crate) fn resolve(config: &UnaryConfig) -> Result<Self, String> {
        let input = size(&config.max_input_size, "max_input_size", 16 * MIB as u64, 1)?;
        let output = size(&config.max_output_size, "max_output_size", 16 * MIB as u64, 1)?;
        let pool = size(&config.memory_budget, "memory_budget", 16 * 1024 * MIB as u64, 0)?;
        let key = size(&config.max_key_size, "max_key_size", 256, 1)?;
        let metadata = size(&config.max_metadata_size, "max_metadata_size", 65536, 1)?;
        let timeout = parse_duration(&config.hold_timeout)
            .map_err(|_| "Invalid unary hold_timeout; use a duration such as 120s".to_string())?;
        if !(Duration::from_millis(1)..=Duration::from_secs(3600)).contains(&timeout)
            || !(1..=64).contains(&config.max_json_depth)
            || !(1..=16384).contains(&config.max_json_nodes)
            || !(4..=65536).contains(&config.max_source_events)
            || metadata < 8192
        {
            return Err(
                "Unary safety limits exceed audited ranges; check depth, nodes, events, metadata and timeout".into(),
            );
        }
        let reservation = reservation(input, output, config.max_json_nodes, config.max_source_events, metadata)?;
        let slots = pool / reservation;
        if slots == 0 {
            return Err(format!("Unary memory_budget must fund at least one {reservation}-byte reservation; increase memory_budget or lower conversion limits"));
        }
        if slots > tokio::sync::Semaphore::MAX_PERMITS {
            return Err("Unary memory_budget exceeds semaphore capacity".into());
        }
        Ok(Self {
            input,
            output,
            timeout,
            depth: config.max_json_depth,
            nodes: config.max_json_nodes,
            key,
            events: config.max_source_events,
            metadata,
            lexical_events: mul(3, config.max_json_nodes)?,
            lexical_work: mul(4, input)?,
            reservation,
            slots,
        })
    }
}
fn reservation(i: usize, o: usize, n: usize, f: usize, w: usize) -> Result<usize, String> {
    let d = mul(768, n.checked_add(1).ok_or_else(overflow)?)?;
    let q = mul(
        3 * 64,
        f.checked_add(4)
            .and_then(usize::checked_next_power_of_two)
            .ok_or_else(overflow)?,
    )?;
    let h = sum(&[mul(4, sum(&[w, 128 * 1024])?)?, 64 * 1024])?;
    let common = sum(&[q, h, 128 * 1024])?;
    let phases = [
        sum(&[p(i)?, common, 16 * 1024])?,
        sum(&[c(i)?, d, i, p(i)?, common, 16 * 1024])?,
        sum(&[c(i)?, d, i, p(i)?, p(o)?, common, 16 * 1024])?,
        sum(&[c(i)?, d, i, c(i)?, c(o)?, p(o)?, common, 16 * 1024])?,
        sum(&[c(o)?, common])?,
    ];
    let peak = phases.into_iter().max().ok_or_else(overflow)?;
    mul(peak.checked_add(MIB - 1).ok_or_else(overflow)? / MIB, MIB)
}
pub(crate) fn append(bytes: &mut Vec<u8>, input: &[u8], limit: usize) -> Result<(), ()> {
    let len = bytes
        .len()
        .checked_add(input.len())
        .filter(|len| *len <= limit)
        .ok_or(())?;
    if len > bytes.capacity() {
        bytes.try_reserve(len - bytes.len()).map_err(|_| ())?;
    }
    bytes.extend_from_slice(input);
    Ok(())
}
#[cfg(test)]
#[path = "../../../../tests/unit/ai/conversion/chat_to_anthropic/limits_test.rs"]
mod tests;
