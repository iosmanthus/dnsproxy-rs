use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Add;
use std::time;

pub struct ValueWithTtl<T, C = SystemClock>
where
    C: Clock,
{
    deadline: C::Instant,
    clock: C,
    value: T,
}

impl<T, C> ValueWithTtl<T, C>
where
    C: Clock,
{
    pub fn new(value: T, clock: C, ttl: C::Duration) -> Self {
        Self {
            value,
            deadline: clock.now() + ttl,
            clock,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.clock.now() >= self.deadline
    }

    pub fn get(&self) -> Option<&T> {
        if self.is_expired() {
            None
        } else {
            Some(&self.value)
        }
    }

    pub fn extract(self) -> Option<T> {
        if self.is_expired() {
            None
        } else {
            Some(self.value)
        }
    }
}

pub trait Clock {
    type Duration;
    type Instant: Add<Self::Duration, Output = Self::Instant> + PartialOrd;
    fn now(&self) -> Self::Instant;
}

#[derive(Clone)]
pub struct SystemClock;

impl Clock for SystemClock {
    type Duration = time::Duration;
    type Instant = time::Instant;

    fn now(&self) -> Self::Instant {
        time::Instant::now()
    }
}

pub struct Cache<K, V, C = SystemClock>
where
    C: Clock,
{
    inner: HashMap<K, ValueWithTtl<V, C>>,
    clock: C,
}

impl<K, V> Cache<K, V, SystemClock> {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
            clock: SystemClock,
        }
    }
}

impl<K, V, C> Cache<K, V, C>
where
    K: Eq + Hash,
    C: Clock + Clone,
{
    pub fn with_clock(clock: C) -> Self {
        Self {
            inner: HashMap::new(),
            clock,
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn insert(&mut self, key: K, value: V, ttl: C::Duration) -> Option<V> {
        self.inner
            .insert(key, ValueWithTtl::new(value, self.clock.clone(), ttl))?
            .extract()
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.inner.get(&key)?.get()
    }

    pub fn gc(&mut self) {
        self.inner.retain(|_, v| !v.is_expired());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct MockClock {
        now: Arc<Mutex<u64>>,
    }

    impl MockClock {
        pub fn new(now: u64) -> MockClock {
            MockClock {
                now: Arc::new(Mutex::new(now)),
            }
        }
        pub fn set(&self, time: u64) {
            let mut now = self.now.lock().unwrap();
            *now = time;
        }
    }

    impl Clock for MockClock {
        type Instant = u64;
        type Duration = u64;

        fn now(&self) -> Self::Instant {
            let now = self.now.lock().unwrap();
            *now
        }
    }

    #[test]
    fn test_insert_and_get() {
        let clock = MockClock::new(0);
        let mut cache = Cache::with_clock(clock.clone());
        let key = "key";
        let value = "value";

        cache.insert("key", "value", 1);
        assert_eq!(cache.get(&key), Some(&value));

        clock.set(2);
        assert_eq!(cache.get(&key), None)
    }

    #[test]
    fn test_gc() {
        let clock = MockClock::new(0);
        let mut cache = Cache::with_clock(clock.clone());
        let size = 10;
        for i in 1..=size {
            cache.insert(i, i, i);
        }
        clock.set(size / 2);
        cache.gc();
        assert_eq!(cache.len(), (size / 2) as usize);
    }
}
