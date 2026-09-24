//! Runtime-independent Web Application Firewall engine primitives.
//!
//! This crate is intentionally a workspace leaf. It owns Edgion's WAF rule
//! semantics and matching implementation without depending on another WAF
//! engine or on gateway transport types.

#![forbid(unsafe_code)]

pub mod aho_corasick;
pub mod matcher;
pub mod seclang;

mod injection;
#[allow(dead_code)]
mod sqli;
#[allow(dead_code)]
mod xss;

#[cfg(test)]
mod libinjection_diff;
