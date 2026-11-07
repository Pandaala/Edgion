//! Safety ceilings shared by every `ShardedLruCache` sizing path.

pub const MAX_LRU_CACHE_CAPACITY: usize = 65_536;
pub const MAX_LRU_CACHE_SHARDS: usize = 64;
