//! Bounded publication-time proof that anchored regular languages are disjoint.
use regex_automata::{
    dfa::{dense, Automaton},
    nfa::thompson,
    Anchored, Input, MatchKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisjointError {
    InputLimit,
    BuildLimit,
    StateLimit,
    WorkLimit,
    Unsupported,
    Overlap,
}
#[derive(Clone, Copy)]
struct Limits {
    patterns: usize,
    input: usize,
    nfa: usize,
    dfa: usize,
    scratch: usize,
    states: usize,
    transitions: usize,
}
const LIMITS: Limits = Limits {
    patterns: 256,
    input: 1024 * 1024,
    nfa: 2 * 1024 * 1024,
    dfa: 4 * 1024 * 1024,
    scratch: 4 * 1024 * 1024,
    states: 16_384,
    transitions: 4_194_304,
};

/// Each group is an independent namespace (for example one HTTP method).
/// All groups share input and transition allowances. Exhaustion is an error,
/// never evidence of disjointness. Patterns must describe complete anchored paths.
pub fn check_disjoint(groups: &[Vec<&str>]) -> Result<(), DisjointError> {
    check(groups, LIMITS)
}
fn check(groups: &[Vec<&str>], limits: Limits) -> Result<(), DisjointError> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    for pattern in groups.iter().flatten() {
        count = count.checked_add(1).ok_or(DisjointError::InputLimit)?;
        bytes = bytes.checked_add(pattern.len()).ok_or(DisjointError::InputLimit)?;
        if count > limits.patterns || bytes > limits.input {
            return Err(DisjointError::InputLimit);
        }
    }
    let mut remaining = limits.transitions;
    for patterns in groups {
        if patterns.is_empty() {
            continue;
        }
        let dfa = dense::Builder::new()
            .configure(
                dense::Config::new()
                    .match_kind(MatchKind::All)
                    .start_kind(regex_automata::dfa::StartKind::Anchored)
                    .dfa_size_limit(Some(limits.dfa))
                    .determinize_size_limit(Some(limits.scratch)),
            )
            .thompson(thompson::Config::new().nfa_size_limit(Some(limits.nfa)))
            .build_many(patterns)
            .map_err(|_| DisjointError::BuildLimit)?;
        let start = dfa
            .start_state_forward(&Input::new(b"").anchored(Anchored::Yes))
            .map_err(|_| DisjointError::Unsupported)?;
        if limits.states == 0 {
            return Err(DisjointError::StateLimit);
        }
        // Exactly two fixed-capacity StateID vectors: <=256 KiB on 64-bit
        // platforms. Sorted visited storage avoids hash-table growth overhead.
        let mut visited = Vec::with_capacity(limits.states);
        let mut queue = Vec::with_capacity(limits.states);
        visited.push(start);
        queue.push(start);
        let mut cursor = 0;
        while cursor < queue.len() {
            let state = queue[cursor];
            cursor += 1;
            remaining = remaining.checked_sub(1).ok_or(DisjointError::WorkLimit)?;
            let end = dfa.next_eoi_state(state);
            if dfa.is_quit_state(end) {
                return Err(DisjointError::Unsupported);
            }
            if dfa.is_match_state(end) && dfa.match_len(end) > 1 {
                return Err(DisjointError::Overlap);
            }
            for byte in 0..=255 {
                remaining = remaining.checked_sub(1).ok_or(DisjointError::WorkLimit)?;
                let next = dfa.next_state(state, byte);
                if dfa.is_quit_state(next) {
                    return Err(DisjointError::Unsupported);
                }
                if dfa.is_dead_state(next) {
                    continue;
                }
                // Also handle acceptance before EOI for callers with a supported
                // early assertion, although path templates anchor at EOI.
                if dfa.is_match_state(next) && dfa.match_len(next) > 1 {
                    return Err(DisjointError::Overlap);
                }
                if let Err(index) = visited.binary_search(&next) {
                    if visited.len() == limits.states {
                        return Err(DisjointError::StateLimit);
                    }
                    visited.insert(index, next);
                    queue.push(next);
                }
            }
        }
    }
    Ok(())
}
#[cfg(test)]
#[path = "../../tests/unit/matcher/regex_disjoint_test.rs"]
mod tests;
