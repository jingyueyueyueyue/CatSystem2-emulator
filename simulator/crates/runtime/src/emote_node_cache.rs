//! Exact-input, byte-bounded memoization for derived node transforms and meshes.
//! Values never contain player clocks, controller state or resource references.

use std::collections::HashMap;

#[derive(Debug)]
pub(super) struct BoundedMemo<V> {
    entries: HashMap<Vec<u64>, (V, usize, u64)>,
    bytes: usize,
    limit: usize,
    clock: u64,
    pub(super) hits: u64,
    pub(super) misses: u64,
}

impl<V> BoundedMemo<V> {
    pub(super) fn new(limit: usize) -> Self {
        Self { entries: HashMap::new(), bytes: 0, limit, clock: 0, hits: 0, misses: 0 }
    }

    pub(super) fn get(&mut self, key: &[u64]) -> Option<&V> {
        self.clock = self.clock.saturating_add(1);
        if let Some((value, _, used)) = self.entries.get_mut(key) {
            self.hits += 1;
            *used = self.clock;
            Some(value)
        } else {
            self.misses += 1;
            None
        }
    }

    pub(super) fn insert(&mut self, key: Vec<u64>, value: V, value_bytes: usize) {
        let bytes = value_bytes.saturating_add(key.capacity().saturating_mul(8));
        if bytes > self.limit { return; }
        if let Some((_, old_bytes, _)) = self.entries.remove(key.as_slice()) {
            self.bytes -= old_bytes;
        }
        // The entry limit also bounds map overhead for many tiny transforms.
        while self.bytes.saturating_add(bytes) > self.limit || self.entries.len() >= 2048 {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, (_, _, used))| *used)
                .map(|(key, _)| key.clone()) else { break; };
            if let Some((_, old_bytes, _)) = self.entries.remove(&oldest) { self.bytes -= old_bytes; }
        }
        self.clock = self.clock.saturating_add(1);
        self.entries.insert(key, (value, bytes, self.clock));
        self.bytes += bytes;
    }

    pub(super) fn bytes(&self) -> usize { self.bytes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_input_reuses_value_and_changed_parent_or_viewport_misses() {
        let mut memo = BoundedMemo::new(256);
        let key = vec![1.0f64.to_bits(), 0.0f64.to_bits(), 1280];
        memo.insert(key.clone(), vec![[2.0f64, 3.0, 4.0, 5.0]], 32);
        assert_eq!(memo.get(&key).unwrap()[0], [2.0, 3.0, 4.0, 5.0]);
        let mut changed = key.clone(); changed[0] = 2.0f64.to_bits();
        assert!(memo.get(&changed).is_none());
        changed = key.clone(); changed[2] = 1920;
        assert!(memo.get(&changed).is_none());
        changed = key; changed[1] = (-0.0f64).to_bits();
        assert!(memo.get(&changed).is_none());
    }

    #[test]
    fn recently_used_entries_survive_and_budget_includes_keys() {
        let mut memo = BoundedMemo::new(80);
        memo.insert(vec![1], 11, 32);
        memo.insert(vec![2], 22, 32);
        assert_eq!(memo.get(&[1]), Some(&11));
        memo.insert(vec![3], 33, 32);
        assert!(memo.get(&[2]).is_none());
        assert_eq!(memo.get(&[1]), Some(&11));
        assert_eq!(memo.get(&[3]), Some(&33));
        assert_eq!(memo.bytes(), 80);
        memo.insert(vec![4; 20], 44, 1);
        assert!(memo.get(&[4; 20]).is_none());
        assert_eq!(memo.bytes(), 80);
    }

    #[test]
    fn repeated_insert_replaces_without_inflating_memory() {
        let mut memo = BoundedMemo::new(80);
        memo.insert(vec![1], 1, 32);
        memo.insert(vec![1], 2, 16);
        assert_eq!(memo.get(&[1]), Some(&2));
        assert_eq!(memo.bytes(), 24);
    }
}
