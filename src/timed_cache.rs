use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

/// A simple cache which stores values that are valid for a limited amount of time.
pub struct TimedCache<K, V> {
    cache: HashMap<K, (V, Instant)>,
}

impl<K, V> TimedCache<K, V>
where
    K: Eq + Hash,
{
    /// Create a new TimedCache with a specific `cache_duration`, which specifies
    /// how long a value in the cache is valid for.
    pub fn new() -> TimedCache<K, V> {
        TimedCache {
            cache: HashMap::new(),
        }
    }

    /// Get the key if found in the cache. This will return
    /// `None` if the value is older than the cache's `cache_duration`.
    pub fn get(&self, key: &K) -> Option<&V> {
        if let Some((value, stored_at)) = self.cache.get(key) {
            if stored_at > &Instant::now() {
                return Some(value);
            }
        }

        None
    }

    /// Sets the value for the key, overwriting the previous value.
    /// Because this overwrites, the value's time in the cache will be refreshed.
    pub fn set(&mut self, key: K, value: V, ttl: Duration) {
        self.cache.insert(key, (value, Instant::now() + ttl));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::sync::Arc;

    #[test]
    fn works_as_expected() {
        let mut cache: TimedCache<Arc<&str>, usize> = TimedCache::new();

        let key = Arc::new("hello");
        assert!(cache.get(&key).is_none());
        cache.set(Arc::clone(&key), 10, Duration::from_secs(1));
        assert_eq!(Some(&10), cache.get(&key));
        thread::sleep(Duration::from_secs(2));
        assert!(cache.get(&key).is_none());
    }
}
