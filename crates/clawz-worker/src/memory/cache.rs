use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct MemoryCache {
    capacity: usize,
    map: HashMap<String, String>,
    order: VecDeque<String>,
    hit_count: AtomicU64,
    miss_count: AtomicU64,
}

impl MemoryCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
        }
    }

    pub fn get(&mut self, key: &str) -> Option<&String> {
        if self.map.contains_key(key) {
            self.hit_count.fetch_add(1, Ordering::Relaxed);
            self.order.retain(|k| k != key);
            self.order.push_back(key.to_string());
            self.map.get(key)
        } else {
            self.miss_count.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    pub fn put(&mut self, key: String, value: String) {
        if self.map.contains_key(&key) {
            self.order.retain(|k| k != &key);
        } else if self.map.len() >= self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.map.remove(&evicted);
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, value);
    }

    pub fn hits(&self) -> u64 {
        self.hit_count.load(Ordering::Relaxed)
    }
    pub fn misses(&self) -> u64 {
        self.miss_count.load(Ordering::Relaxed)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hit_avoids_second_lookup() {
        let mut cache = MemoryCache::new(10);
        cache.put("key1".into(), "value1".into());
        assert_eq!(cache.get("key1"), Some(&"value1".to_string()));
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 0);
        assert_eq!(cache.get("missing"), None);
        assert_eq!(cache.misses(), 1);
    }

    #[test]
    fn eviction_at_capacity() {
        let mut cache = MemoryCache::new(2);
        cache.put("a".into(), "1".into());
        cache.put("b".into(), "2".into());
        cache.put("c".into(), "3".into());
        assert_eq!(cache.get("a"), None);
        assert_eq!(cache.get("b"), Some(&"2".to_string()));
        assert_eq!(cache.get("c"), Some(&"3".to_string()));
        assert_eq!(cache.len(), 2);
    }
}
