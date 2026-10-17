//! Concurrent deduplication of k-mer words.
//!
//! Every worker thread of a de Bruijn construction sees most of the k-mers of
//! a pangenome, so per-thread sets merged at the end would each hold almost the
//! whole set. Instead all threads insert into one [`ShardedSet`]: the words are
//! split over [`SHARDS`] independently locked hash sets, and each thread
//! batches its inserts per shard in a [`ShardBuffer`] so a lock is taken once
//! per [`BUFFER_LEN`] words.

use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Mutex, PoisonError};

use rayon::prelude::*;

use super::word::KmerWord;

const SHARD_BITS: u32 = 8;
/// Number of independently locked shards.
const SHARDS: usize = 1 << SHARD_BITS;
/// Words buffered per shard before a flush.
const BUFFER_LEN: usize = 512;

/// The splitmix64 finalizer: a fast, well-mixing 64-bit permutation.
#[inline]
const fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A cheap hasher for integer keys (k-mer words and small tuples of them).
///
/// k-mer words are not adversarial input, so hash-flooding resistance is
/// not needed; this is several times faster.
#[derive(Default, Clone, Copy)]
pub struct KmerHasher(u64);

impl Hasher for KmerHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    #[inline]
    fn write_u64(&mut self, x: u64) {
        self.0 = mix64(self.0.rotate_left(5) ^ x);
    }

    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    fn write_u128(&mut self, x: u128) {
        // Intentionally hashes the two halves.
        self.write_u64(x as u64);
        self.write_u64((x >> 64) as u64);
    }

    #[inline]
    fn write_usize(&mut self, x: usize) {
        self.write_u64(x as u64);
    }
}

pub type KmerBuildHasher = BuildHasherDefault<KmerHasher>;

/// Shard of a word. Uses a different mix than [`KmerHasher`]: hashbrown takes
/// its bucket index and tag bytes from the hash, so deriving the shard from the
/// same hash would leave the words of a shard with correlated hashes.
#[inline]
#[allow(clippy::cast_possible_truncation)]
fn shard_of<W: KmerWord>(word: W) -> usize {
    // The top SHARD_BITS bits always fit in usize.
    (word.fold64().wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - SHARD_BITS)) as usize
}

/// A set of k-mer words that many threads can insert into at once.
pub struct ShardedSet<W> {
    shards: Vec<Mutex<HashSet<W, KmerBuildHasher>>>,
}

impl<W: KmerWord> ShardedSet<W> {
    pub fn new() -> Self {
        Self {
            shards: (0..SHARDS)
                .map(|_| Mutex::new(HashSet::default()))
                .collect(),
        }
    }

    /// Returns a per-thread insert buffer; it flushes when dropped.
    pub fn buffer(&self) -> ShardBuffer<'_, W> {
        ShardBuffer {
            set: self,
            pending: (0..SHARDS).map(|_| Vec::new()).collect(),
        }
    }

    /// Moves `words` (all of shard `shard`) into the set.
    fn flush(&self, shard: usize, words: &mut Vec<W>) {
        // A poisoned lock only means another worker panicked mid-insert; the
        // set itself is still valid.
        let mut set = self.shards[shard]
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        set.extend(words.drain(..));
    }

    /// Number of distinct words inserted so far.
    fn len_all(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.lock().unwrap_or_else(PoisonError::into_inner).len())
            .sum()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.len_all()
    }

    /// Consumes the set into a sorted vector of its words. Shards are drained
    /// one at a time, so the set and the vector are never both fully resident.
    pub fn into_sorted_vec(self) -> Vec<W> {
        let total = self.len_all();
        let mut words = Vec::with_capacity(total);
        for shard in self.shards {
            // Each table is freed as soon as it has been moved into `words`.
            words.extend(shard.into_inner().unwrap_or_else(PoisonError::into_inner));
        }
        // Shards are disjoint, so the words are already distinct.
        words.par_sort_unstable();
        words
    }
}

/// A thread-local batch of pending inserts into a [`ShardedSet`].
pub struct ShardBuffer<'a, W: KmerWord> {
    set: &'a ShardedSet<W>,
    pending: Vec<Vec<W>>,
}

impl<W: KmerWord> ShardBuffer<'_, W> {
    #[inline]
    pub fn insert(&mut self, word: W) {
        let shard = shard_of(word);
        let buffer = &mut self.pending[shard];
        if buffer.capacity() == 0 {
            buffer.reserve_exact(BUFFER_LEN);
        }
        buffer.push(word);
        if buffer.len() >= BUFFER_LEN {
            self.set.flush(shard, buffer);
        }
    }
}

impl<W: KmerWord> Drop for ShardBuffer<'_, W> {
    fn drop(&mut self) {
        for (shard, buffer) in self.pending.iter_mut().enumerate() {
            if !buffer.is_empty() {
                self.set.flush(shard, buffer);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sharded_set_dedups_and_sorts() {
        let set = ShardedSet::<u64>::new();
        {
            let mut buf = set.buffer();
            for i in (0..5000u64).rev() {
                buf.insert(i % 1234);
            }
        }
        assert_eq!(set.len(), 1234);
        let words = set.into_sorted_vec();
        assert_eq!(words, (0..1234).collect::<Vec<u64>>());
    }

    #[test]
    fn test_sharded_set_concurrent_inserts() {
        let set = ShardedSet::<u128>::new();
        (0..64u128).into_par_iter().for_each_init(
            || set.buffer(),
            |buf, t| {
                // Every task inserts an overlapping range.
                for i in 0..10_000u128 {
                    buf.insert((i + t * 100) << 70);
                }
            },
        );
        let words = set.into_sorted_vec();
        assert_eq!(words.len(), 10_000 + 63 * 100);
        assert!(words.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn test_shards_are_spread() {
        let mut counts = vec![0usize; SHARDS];
        for i in 0..(SHARDS as u64 * 64) {
            counts[shard_of(i)] += 1;
        }
        assert!(counts.iter().all(|&c| c > 0));
    }
}
