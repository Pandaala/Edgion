//! Peak-EWMA latency scoring for the `Ewma` load-balancer policy.
//!
//! The score itself lives in [`score`] and is stored per backend on
//! `LbBackend`; selection is performed by `lb::selection::p2c`.
//!
//! There is deliberately **no** global smoothing parameter. The blend weight is
//! the elapsed-derived decay factor, which makes smoothing a function of the
//! backend's own request rate rather than of a process-wide knob. The former
//! `alpha` / `set_alpha` / `get_alpha` surface is gone — it had no production
//! call site, and reintroducing it would restore the QPS-dependent decay this
//! design removes. See `skills/12-algorithms/lb-peak-ewma.md`.

pub mod score;
