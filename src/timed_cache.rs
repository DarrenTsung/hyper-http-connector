use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

/// A simple cache which stores values that are valid for a limited amount of time.
pub struct TimedCache<K, V> {
    cache: HashMap<K, (V, Instant)>,
    cache_duration: Duration,
}

impl<K, V> TimedCache<K, V>
where
    K: Eq + Hash,
{
    /// Create a new TimedCache with a specific `cache_duration`, which specifies
    /// how long a value in the cache is valid for.
    pub fn new(cache_duration: Duration) -> TimedCache<K, V> {
        TimedCache {
            cache: HashMap::new(),
            cache_duration,
        }
    }

    /// Get the key if found in the cache. This will return
    /// `None` if the value is older than the cache's `cache_duration`.
    pub fn get(&self, key: &K) -> Option<&V> {
        if let Some((value, stored_at)) = self.cache.get(key) {
            if stored_at.elapsed() < self.cache_duration {
                return Some(value);
            }
        }

        None
    }

    /// Sets the value for the key, overwritting ignoring the previous value.
    /// Because this overwrites, the value's time in the cache will be refreshed.
    pub fn set(&mut self, key: K, value: V) {
        self.cache.insert(key, (value, Instant::now()));
    }
}
