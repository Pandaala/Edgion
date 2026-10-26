//! Immutable raw query backing and demand-created parameter views.

use std::ops::Range;
use std::sync::OnceLock;

use edgion_resources::uri_utils::url_decode;

/// Raw bytes remain authoritative; decoded strings are only read projections.
#[derive(Default)]
pub(super) struct QueryDocument {
    raw: Option<String>,
    parsed: OnceLock<Option<Box<ParsedQuery>>>,
}

pub(super) struct ParsedQuery {
    first: Box<[Slot]>,
    first_cursor: usize,
    names: OnceLock<Box<[usize]>>,
}

pub(super) struct QueryPair {
    whole: Range<usize>,
    equals: Option<usize>,
    name: OnceLock<Box<str>>,
    value: OnceLock<Box<str>>,
}

// A terminal cell stores None; later reads never rescan a known end.
enum Slot {
    Pair(OnceLock<Option<QueryPair>>),
    Tail(OnceLock<Option<Tail>>),
}

struct Tail {
    cursor: usize,
    slots: Box<[Slot]>,
}

fn new_block(capacity: usize) -> Box<[Slot]> {
    let length = capacity.checked_add(1).expect("query block length fits input bound");
    std::alloc::Layout::array::<Slot>(length).expect("query block allocation layout fits address space");
    let mut slots = Vec::with_capacity(length);
    slots.extend((0..capacity).map(|_| Slot::Pair(OnceLock::new())));
    slots.push(Slot::Tail(OnceLock::new()));
    slots.into_boxed_slice()
}

struct Pairs<'a> {
    raw: &'a str,
    block: &'a [Slot],
    position: usize,
    cursor: usize,
    ended: bool,
}

impl ParsedQuery {
    fn iter<'a>(&'a self, raw: &'a str) -> Pairs<'a> {
        Pairs {
            raw,
            block: &self.first,
            position: 0,
            cursor: self.first_cursor,
            ended: false,
        }
    }

    // Only ids returned by a completed sequential traversal are retained.
    pub(super) fn pair(&self, mut id: usize) -> &QueryPair {
        let mut block = self.first.as_ref();
        loop {
            let count = block.len() - 1;
            if id < count {
                let Slot::Pair(cell) = &block[id] else { unreachable!() };
                return cell.get().and_then(Option::as_ref).expect("initialized query pair id");
            }
            id -= count;
            let Slot::Tail(cell) = &block[count] else {
                unreachable!()
            };
            block = &cell
                .get()
                .and_then(Option::as_ref)
                .expect("initialized query block")
                .slots;
        }
    }
}

impl<'a> Iterator for Pairs<'a> {
    type Item = &'a QueryPair;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return None;
        }
        loop {
            match &self.block[self.position] {
                Slot::Pair(cell) => {
                    let pair = cell.get_or_init(|| {
                        let rest = self.raw[self.cursor..].trim_start_matches('&');
                        if rest.is_empty() {
                            return None;
                        }
                        let start = self.raw.len() - rest.len();
                        let part = rest.split('&').next().expect("nonempty query suffix");
                        let end = start.checked_add(part.len()).expect("pair bounded by raw input");
                        Some(QueryPair {
                            whole: start..end,
                            equals: part
                                .find('=')
                                .map(|offset| start.checked_add(offset).expect("equals bounded by pair")),
                            name: OnceLock::new(),
                            value: OnceLock::new(),
                        })
                    });
                    let Some(pair) = pair else {
                        self.ended = true;
                        return None;
                    };
                    self.cursor = pair.whole.end;
                    if self.cursor < self.raw.len() {
                        self.cursor = self.cursor.checked_add(1).expect("separator bounded by input");
                    }
                    self.position = self.position.checked_add(1).expect("position bounded by block");
                    return Some(pair);
                }
                Slot::Tail(cell) => {
                    let tail = cell.get_or_init(|| {
                        let rest = self.raw[self.cursor..].trim_start_matches('&');
                        if rest.is_empty() {
                            return None;
                        }
                        let capacity = (self.block.len() - 1)
                            .checked_mul(2)
                            .unwrap_or(rest.len())
                            .min(rest.len());
                        Some(Tail {
                            cursor: self.raw.len() - rest.len(),
                            slots: new_block(capacity),
                        })
                    });
                    let Some(tail) = tail else {
                        self.ended = true;
                        return None;
                    };
                    self.cursor = tail.cursor;
                    self.block = &tail.slots;
                    self.position = 0;
                }
            }
        }
    }
}

impl QueryDocument {
    pub(super) fn new(raw: Option<String>) -> Self {
        Self {
            raw,
            parsed: OnceLock::new(),
        }
    }

    pub(super) fn raw(&self) -> Option<&str> {
        self.raw.as_deref()
    }

    pub(super) fn parsed(&self) -> Option<&ParsedQuery> {
        self.parsed
            .get_or_init(|| {
                let raw = self.raw.as_deref()?;
                let rest = raw.trim_start_matches('&');
                if rest.is_empty() {
                    return None;
                }
                Some(Box::new(ParsedQuery {
                    first: new_block(4),
                    first_cursor: raw.len() - rest.len(),
                    names: OnceLock::new(),
                }))
            })
            .as_deref()
    }

    pub(super) fn pairs(&self) -> impl Iterator<Item = (usize, &QueryPair)> {
        // Every yielded id consumes at least one raw byte, so enumerate cannot
        // exceed the already allocated string's isize-bounded length.
        self.parsed()
            .into_iter()
            .flat_map(|parsed| parsed.iter(self.raw().unwrap_or_default()))
            .enumerate()
    }

    pub(super) fn raw_pair(&self, pair: &QueryPair) -> &str {
        &self.raw.as_deref().unwrap_or_default()[pair.whole.clone()]
    }

    pub(super) fn raw_name(&self, pair: &QueryPair) -> &str {
        &self.raw.as_deref().unwrap_or_default()[pair.whole.start..pair.equals.unwrap_or(pair.whole.end)]
    }

    pub(super) fn raw_value(&self, pair: &QueryPair) -> &str {
        let start = pair.equals.map_or(pair.whole.end, |equals| equals + 1);
        &self.raw.as_deref().unwrap_or_default()[start..pair.whole.end]
    }

    pub(super) fn decoded_name<'a>(&'a self, pair: &'a QueryPair) -> &'a str {
        Self::decode(self.raw_name(pair), &pair.name)
    }

    pub(super) fn decoded_value<'a>(&'a self, pair: &'a QueryPair) -> &'a str {
        Self::decode(self.raw_value(pair), &pair.value)
    }

    fn decode<'a>(raw: &'a str, cache: &'a OnceLock<Box<str>>) -> &'a str {
        if !raw.contains(['%', '+']) {
            raw
        } else {
            cache.get_or_init(|| url_decode(raw).into_boxed_str())
        }
    }

    pub(super) fn lookup<'a>(&'a self, name: &str, retained: Option<&[usize]>) -> Option<&'a str> {
        if retained.is_some_and(<[usize]>::is_empty) {
            return None;
        }
        let parsed = self.parsed()?;
        let names = parsed.names.get_or_init(|| {
            let mut names: Vec<_> = self.pairs().map(|(id, _)| id).collect();
            names.sort_unstable_by(|a, b| {
                self.decoded_name(parsed.pair(*a))
                    .cmp(self.decoded_name(parsed.pair(*b)))
                    .then_with(|| a.cmp(b))
            });
            names.into_boxed_slice()
        });
        let end = names.partition_point(|id| self.decoded_name(parsed.pair(*id)) <= name);
        for id in names[..end].iter().rev().copied() {
            let pair = parsed.pair(id);
            if self.decoded_name(pair) != name {
                break;
            }
            if retained.is_none_or(|ids| ids.binary_search(&id).is_ok()) {
                return Some(self.decoded_value(pair));
            }
        }
        None
    }

    /// LB's first raw value is deliberately distinct from ordinary lookup.
    pub(super) fn hash_arg<'a>(&'a self, name: &str, retained: Option<&[usize]>) -> Option<&'a str> {
        let parsed = self.parsed()?;
        let find = |pair: &'a QueryPair| {
            pair.equals?;
            let raw = self.raw_name(pair);
            (raw == name || self.decoded_name(pair) == name).then(|| self.raw_value(pair))
        };
        match retained {
            Some(ids) => ids.iter().find_map(|id| find(parsed.pair(*id))),
            None => parsed.iter(self.raw().unwrap_or_default()).find_map(find),
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/request_query/document_test.rs"]
mod tests;
