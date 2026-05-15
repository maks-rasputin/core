use gem_client::X_CACHE_TTL;
use std::collections::HashMap;

pub(crate) const STATIC_READ_CACHE_TTL_SECONDS: u64 = 30 * primitives::duration::DAY.as_secs();

pub(crate) fn static_read_cache_headers() -> HashMap<String, String> {
    HashMap::from([(X_CACHE_TTL.to_string(), STATIC_READ_CACHE_TTL_SECONDS.to_string())])
}
