//! Request-local raw query ownership, lazy reads, and deferred logical edits.
//!
//! URI validation and transport publication belong to the session adapter. It
//! must validate a candidate before calling an editing method, and only mark a
//! projection synchronized after successfully installing the latest target.

mod document;
mod selection;

use std::sync::Arc;

use document::QueryDocument;
use selection::{Selection, SelectionBase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueryError {
    AlreadyCaptured,
    NotCaptured,
    RevisionExhausted,
    SizeOverflow,
    NoBaseline,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::AlreadyCaptured => "request query already captured",
            Self::NotCaptured => "request query not captured",
            Self::RevisionExhausted => "request query revision exhausted",
            Self::SizeOverflow => "request query size overflow",
            Self::NoBaseline => "request query baseline not captured",
        })
    }
}

impl std::error::Error for QueryError {}

#[derive(Clone, Default)]
enum CurrentQuery {
    #[default]
    Uncaptured,
    Entry,
    Replaced(Arc<QueryDocument>),
    Selected(Arc<Selection>),
}

#[derive(Default)]
pub(crate) struct RequestQuery {
    entry: QueryDocument,
    current: CurrentQuery,
    revision: u64,
    synced_revision: u64,
    baseline: Option<Box<CurrentQuery>>,
}

impl RequestQuery {
    pub(crate) fn is_captured(&self) -> bool {
        !matches!(self.current, CurrentQuery::Uncaptured)
    }

    pub(crate) fn capture(&mut self, raw: Option<String>) -> Result<(), QueryError> {
        if self.is_captured() {
            return Err(QueryError::AlreadyCaptured);
        }
        self.entry = QueryDocument::new(raw);
        self.current = CurrentQuery::Entry;
        self.revision = 1;
        self.synced_revision = 1;
        Ok(())
    }

    pub(crate) fn entry_raw(&self) -> Option<&str> {
        self.entry.raw()
    }

    pub(crate) fn entry_param(&self, name: &str) -> Option<&str> {
        self.is_captured().then(|| self.entry.lookup(name, None)).flatten()
    }

    fn backing(&self) -> Option<(&QueryDocument, Option<&Selection>)> {
        match &self.current {
            CurrentQuery::Uncaptured => None,
            CurrentQuery::Entry => Some((&self.entry, None)),
            CurrentQuery::Replaced(document) => Some((document, None)),
            CurrentQuery::Selected(selection) => {
                let document = match &selection.base {
                    SelectionBase::Entry => &self.entry,
                    SelectionBase::Replaced(document) => document,
                };
                Some((document, Some(selection)))
            }
        }
    }

    pub(crate) fn current_raw(&self) -> Option<&str> {
        let (document, selection) = self.backing()?;
        match selection {
            Some(selection) => selection.raw(document),
            None => document.raw(),
        }
    }

    pub(crate) fn current_param(&self, name: &str) -> Option<&str> {
        let (document, selection) = self.backing()?;
        document.lookup(name, selection.map(|selection| selection.retained.as_slice()))
    }

    pub(crate) fn current_hash_arg_raw(&self, name: &str) -> Option<&str> {
        let (document, selection) = self.backing()?;
        document.hash_arg(name, selection.map(|selection| selection.retained.as_slice()))
    }

    pub(crate) fn projection_is_dirty(&self) -> bool {
        self.revision != self.synced_revision
    }

    /// Caller must have successfully installed this exact current target.
    pub(crate) fn mark_synced(&mut self) {
        self.synced_revision = self.revision;
    }

    pub(crate) fn check_next_revision(&self) -> Result<(), QueryError> {
        self.next_revision().map(|_| ())
    }

    fn next_revision(&self) -> Result<u64, QueryError> {
        if !self.is_captured() {
            return Err(QueryError::NotCaptured);
        }
        self.revision.checked_add(1).ok_or(QueryError::RevisionExhausted)
    }

    /// Invalidate after a validated path/authority update or header replacement.
    pub(crate) fn invalidate_projection(&mut self) -> Result<(), QueryError> {
        self.revision = self.next_revision()?;
        Ok(())
    }

    /// Input must already have passed the adapter's URI/target validation.
    pub(crate) fn replace_raw(&mut self, raw: Option<String>) -> Result<bool, QueryError> {
        let (document, selection) = self.backing().ok_or(QueryError::NotCaptured)?;
        let equal = match selection {
            Some(selection) => selection.equals_raw(document, raw.as_deref()),
            None => document.raw() == raw.as_deref(),
        };
        if equal {
            return Ok(false);
        }
        let revision = self.next_revision()?;
        let current = CurrentQuery::Replaced(Arc::new(QueryDocument::new(raw)));
        self.current = current;
        self.revision = revision;
        Ok(true)
    }

    /// Remove all decoded-equivalent names, retaining raw bytes and order.
    pub(crate) fn remove_current_names(&mut self, names: &[&str]) -> Result<bool, QueryError> {
        let (document, selection) = self.backing().ok_or(QueryError::NotCaptured)?;
        if names.is_empty() {
            return Ok(false);
        }
        let Some(_) = document.parsed() else {
            return Ok(false);
        };
        let mut active_ids = selection.map(|view| view.retained.iter().copied().peekable());
        let mut prefix_count = 0;
        let mut retained = None;
        for (id, pair) in document.pairs() {
            if let Some(ids) = &mut active_ids {
                if ids.peek().copied() != Some(id) {
                    continue;
                }
                ids.next();
            }
            if names.contains(&document.decoded_name(pair)) {
                if retained.is_none() {
                    self.check_next_revision()?;
                    let mut survivors = Vec::with_capacity(prefix_count);
                    match selection {
                        Some(view) => survivors.extend_from_slice(&view.retained[..prefix_count]),
                        None => survivors.extend(0..id),
                    }
                    retained = Some(survivors);
                }
            } else if let Some(survivors) = &mut retained {
                survivors.push(id);
            } else {
                prefix_count += 1;
            }
        }
        let Some(retained) = retained else {
            return Ok(false);
        };
        let revision = self.next_revision()?;
        let base = match &self.current {
            CurrentQuery::Entry => SelectionBase::Entry,
            CurrentQuery::Replaced(document) => SelectionBase::Replaced(document.clone()),
            CurrentQuery::Selected(selection) => selection.base.clone(),
            CurrentQuery::Uncaptured => return Err(QueryError::NotCaptured),
        };
        let next = Selection::new(base, retained, document)?;
        self.current = CurrentQuery::Selected(Arc::new(next));
        self.revision = revision;
        Ok(true)
    }

    /// Preserve the first AI baseline inside this request owner only.
    pub(crate) fn capture_baseline(&mut self) -> Result<(), QueryError> {
        if !self.is_captured() {
            return Err(QueryError::NotCaptured);
        }
        if self.baseline.is_none() {
            self.baseline = Some(Box::new(self.current.clone()));
        }
        Ok(())
    }

    pub(crate) fn restore_baseline(&mut self) -> Result<(), QueryError> {
        let baseline = self.baseline.as_ref().ok_or(QueryError::NoBaseline)?;
        let revision = self.next_revision()?;
        self.current = baseline.as_ref().clone();
        self.revision = revision;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/request_query/mod_test.rs"]
mod tests;
