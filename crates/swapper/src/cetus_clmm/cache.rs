use super::model::DiscoveredPool;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Default)]
struct PoolDiscovery {
    pools: Vec<DiscoveredPool>,
    explored_ticks: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct PoolCache {
    pools: Arc<Mutex<HashMap<(String, String), PoolDiscovery>>>,
    routes: Arc<Mutex<HashMap<(String, String), Vec<DiscoveredPool>>>>,
}

impl PoolCache {
    pub fn get(&self, from: &str, to: &str) -> Option<(Vec<DiscoveredPool>, Vec<u32>)> {
        let cache = self.pools.lock().ok()?;
        cache.get(&Self::pool_key(from, to)).map(|d| (d.pools.clone(), d.explored_ticks.clone()))
    }

    pub fn put(&self, from: &str, to: &str, pools: &[DiscoveredPool], ticks: &[u32]) {
        if pools.is_empty() && ticks.is_empty() {
            return;
        }
        if let Ok(mut cache) = self.pools.lock() {
            let entry = cache.entry(Self::pool_key(from, to)).or_default();
            let mut seen: HashSet<String> = entry.pools.iter().map(|p| p.pool_id.clone()).collect();
            for pool in pools {
                if seen.insert(pool.pool_id.clone()) {
                    entry.pools.push(pool.clone());
                }
            }
            for tick in ticks {
                if !entry.explored_ticks.contains(tick) {
                    entry.explored_ticks.push(*tick);
                }
            }
        }
    }

    pub fn get_route(&self, from: &str, to: &str) -> Option<Vec<DiscoveredPool>> {
        let cache = self.routes.lock().ok()?;
        cache.get(&Self::route_key(from, to)).cloned()
    }

    pub fn put_route(&self, from: &str, to: &str, route: &[DiscoveredPool]) {
        if route.is_empty() {
            return;
        }
        if let Ok(mut cache) = self.routes.lock() {
            cache.insert(Self::route_key(from, to), route.to_vec());
        }
    }

    fn pool_key(from: &str, to: &str) -> (String, String) {
        let (a, b) = if from <= to { (from, to) } else { (to, from) };
        (a.to_string(), b.to_string())
    }

    fn route_key(from: &str, to: &str) -> (String, String) {
        (from.to_string(), to.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(id: &str) -> DiscoveredPool {
        DiscoveredPool {
            pool_id: id.into(),
            pool_init_version: 1,
            coin_a: "0xa".into(),
            coin_b: "0xb".into(),
        }
    }

    #[test]
    fn test_pool_key_is_direction_insensitive() {
        assert_eq!(PoolCache::pool_key("0xa", "0xb"), PoolCache::pool_key("0xb", "0xa"));
    }

    #[test]
    fn test_route_key_is_direction_sensitive() {
        assert_ne!(PoolCache::route_key("0xa", "0xb"), PoolCache::route_key("0xb", "0xa"));
    }

    #[test]
    fn test_pool_cache_merges_pools_and_ticks_across_passes() {
        let cache = PoolCache::default();
        cache.put("0xa", "0xb", &[pool("0x1")], &[60, 200]);
        cache.put("0xa", "0xb", &[pool("0x2"), pool("0x1")], &[10, 2]);

        let (pools, ticks) = cache.get("0xa", "0xb").unwrap();
        assert_eq!(pools.len(), 2);
        assert_eq!(pools[0].pool_id, "0x1");
        assert_eq!(pools[1].pool_id, "0x2");
        assert_eq!(ticks, vec![60, 200, 10, 2]);
    }

    #[test]
    fn test_pool_cache_tracks_explored_ticks_when_no_pools_found() {
        let cache = PoolCache::default();
        cache.put("0xa", "0xb", &[], &[60, 200]);
        let (pools, ticks) = cache.get("0xa", "0xb").unwrap();
        assert!(pools.is_empty());
        assert_eq!(ticks, vec![60, 200]);
    }

    #[test]
    fn test_route_roundtrip() {
        let cache = PoolCache::default();
        let route = vec![pool("0x1"), pool("0x2")];
        cache.put_route("USDC", "WAL", &route);
        let fetched = cache.get_route("USDC", "WAL").expect("hit");
        assert_eq!(fetched.len(), 2);
        assert!(cache.get_route("WAL", "USDC").is_none(), "direction-sensitive");
    }

    #[test]
    fn test_put_route_skips_empty() {
        let cache = PoolCache::default();
        cache.put_route("USDC", "WAL", &[]);
        assert!(cache.get_route("USDC", "WAL").is_none());
    }
}
