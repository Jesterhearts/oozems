use std::collections::HashMap;
use std::hash::Hash;

pub(super) struct KeyedRefreshState<K, R, V> {
    pub cached: HashMap<K, V>,
    pub pending: Option<R>,
    pub in_flight: Option<R>,
    pub retry_after_ms: HashMap<K, f64>,
    pub retry_count: HashMap<K, u8>,
}

impl<K, R, V> Default for KeyedRefreshState<K, R, V> {
    fn default() -> Self {
        Self {
            cached: HashMap::new(),
            pending: None,
            in_flight: None,
            retry_after_ms: HashMap::new(),
            retry_count: HashMap::new(),
        }
    }
}

impl<K, R, V> KeyedRefreshState<K, R, V>
where
    K: Eq + Hash,
{
    pub fn retry_is_ready(
        &self,
        key: &K,
        now_ms: f64,
    ) -> bool {
        self.retry_after_ms
            .get(key)
            .is_none_or(|deadline_ms| now_ms >= *deadline_ms)
    }

    pub fn clear_retry(
        &mut self,
        key: &K,
    ) {
        self.retry_after_ms.remove(key);
        self.retry_count.remove(key);
    }

    pub fn delay_retry(
        &mut self,
        key: K,
        retry_after_ms: f64,
    ) {
        self.retry_after_ms.insert(key, retry_after_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::KeyedRefreshState;

    #[test]
    fn retries_are_admitted_at_the_deadline_and_can_be_cleared() {
        let mut state = KeyedRefreshState::<u32, u32, String>::default();
        state.delay_retry(4, 500.0);

        assert!(!state.retry_is_ready(&4, 499.0));
        assert!(state.retry_is_ready(&4, 500.0));
        state.clear_retry(&4);
        assert!(state.retry_is_ready(&4, 0.0));
    }
}
