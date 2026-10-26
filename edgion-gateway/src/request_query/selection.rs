//! Immutable current-query selection over one reusable raw backing.

use std::sync::{Arc, OnceLock};

use super::document::QueryDocument;
use super::QueryError;

#[derive(Clone)]
pub(super) enum SelectionBase {
    Entry,
    Replaced(Arc<QueryDocument>),
}

pub(super) struct Selection {
    pub(super) base: SelectionBase,
    pub(super) retained: Vec<usize>,
    output_len: usize,
    rendered: OnceLock<Option<Box<str>>>,
}

impl Selection {
    pub(super) fn new(base: SelectionBase, retained: Vec<usize>, document: &QueryDocument) -> Result<Self, QueryError> {
        let mut output_len = 0_usize;
        let mut pairs = document.pairs();
        for (position, &id) in retained.iter().enumerate() {
            let (_, pair) = pairs
                .find(|(candidate, _)| *candidate == id)
                .expect("retained ids are ordered initialized pairs");
            output_len = output_len
                .checked_add(usize::from(position != 0))
                .and_then(|len| len.checked_add(document.raw_pair(pair).len()))
                .ok_or(QueryError::SizeOverflow)?;
        }
        Ok(Self {
            base,
            retained,
            output_len,
            rendered: OnceLock::new(),
        })
    }

    pub(super) fn raw<'a>(&'a self, document: &QueryDocument) -> Option<&'a str> {
        self.rendered
            .get_or_init(|| {
                if self.retained.is_empty() {
                    return None;
                }
                let mut pairs = document.pairs();
                let mut output = String::with_capacity(self.output_len);
                for (position, &id) in self.retained.iter().enumerate() {
                    if position != 0 {
                        output.push('&');
                    }
                    let (_, pair) = pairs
                        .find(|(candidate, _)| *candidate == id)
                        .expect("retained ids are ordered initialized pairs");
                    output.push_str(document.raw_pair(pair));
                }
                Some(output.into_boxed_str())
            })
            .as_deref()
    }

    /// Equality must not materialize an otherwise unobserved selection.
    pub(super) fn equals_raw(&self, document: &QueryDocument, raw: Option<&str>) -> bool {
        if self.retained.is_empty() {
            return raw.is_none();
        }
        let Some(mut remaining) = raw else {
            return false;
        };
        if remaining.len() != self.output_len {
            return false;
        }
        let mut pairs = document.pairs();
        for (position, &id) in self.retained.iter().enumerate() {
            if position != 0 {
                let Some(rest) = remaining.strip_prefix('&') else {
                    return false;
                };
                remaining = rest;
            }
            let (_, pair) = pairs
                .find(|(candidate, _)| *candidate == id)
                .expect("retained ids are ordered initialized pairs");
            let Some(rest) = remaining.strip_prefix(document.raw_pair(pair)) else {
                return false;
            };
            remaining = rest;
        }
        remaining.is_empty()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/request_query/selection_test.rs"]
mod tests;
