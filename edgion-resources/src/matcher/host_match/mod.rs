pub mod radix_match;

mod hash_match;

pub use hash_match::{canonical_host_key, lookup_rfc6125, HashHost};
pub use radix_match::{RadixHost, RadixHostMatchEngine};
