use super::model::DiscoveredPool;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Default)]
pub(super) struct PoolCache {
    inner: Arc<Mutex<HashMap<(String, String), Vec<DiscoveredPool>>>>,
}

impl PoolCache {
    pub fn get(&self, from: &str, to: &str) -> Option<Vec<DiscoveredPool>> {
        let cache = self.inner.lock().ok()?;
        cache.get(&Self::key(from, to)).cloned()
    }

    pub fn put(&self, from: &str, to: &str, pools: &[DiscoveredPool]) {
        if pools.is_empty() {
            return;
        }
        if let Ok(mut cache) = self.inner.lock() {
            cache.insert(Self::key(from, to), pools.to_vec());
        }
    }

    fn key(from: &str, to: &str) -> (String, String) {
        let (a, b) = if from <= to { (from, to) } else { (to, from) };
        (a.to_string(), b.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_is_direction_insensitive() {
        assert_eq!(PoolCache::key("0xa", "0xb"), PoolCache::key("0xb", "0xa"));
    }

    #[test]
    fn test_put_skips_empty_to_avoid_false_negative_cache() {
        let cache = PoolCache::default();
        cache.put("0xa", "0xb", &[]);
        assert!(cache.get("0xa", "0xb").is_none());
    }
}
