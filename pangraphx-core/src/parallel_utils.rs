//! Utilities for parallel processing using Rayon.
//!
//! This module provides helper functions and patterns for common parallelization
//! operations across the codebase, such as aggregating results from parallel iterations
//! and managing thread-local state.

use std::collections::HashSet;
use std::hash::Hash;

/// Merge multiple HashSets from parallel thread-local collections into a single set.
///
/// This is useful when parallel operations accumulate results into thread-local HashSets
/// and the final step is to combine them all.
///
/// # Example
///
/// ```ignore
/// use rayon::prelude::*;
/// let results: Vec<HashSet<i32>> = (0..10)
///     .into_par_iter()
///     .fold(
///         || HashSet::new(),
///         |mut set, i| {
///             set.insert(i);
///             set
///         }
///     )
///     .collect();
///
/// let merged = merge_hashsets(results);
/// ```
pub fn merge_hashsets<T: Eq + Hash + Clone>(sets: Vec<HashSet<T>>) -> HashSet<T> {
    let mut result = HashSet::new();
    for set in sets {
        result.extend(set);
    }
    result
}

/// Merge multiple HashSets in-place into an accumulator, consuming the input sets.
///
/// This is more efficient than `merge_hashsets` when you have mutable access
/// to the accumulator and want to avoid cloning.
pub fn merge_into_hashset<T: Eq + Hash>(accumulator: &mut HashSet<T>, sets: Vec<HashSet<T>>) {
    for set in sets {
        accumulator.extend(set);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_hashsets() {
        let set1 = vec![1, 2, 3].into_iter().collect();
        let set2 = vec![3, 4, 5].into_iter().collect();
        let sets = vec![set1, set2];

        let merged = merge_hashsets(sets);
        assert_eq!(merged.len(), 5);
        assert!(merged.contains(&1));
        assert!(merged.contains(&5));
    }

    #[test]
    fn test_merge_into_hashset() {
        let mut accumulator = vec![1, 2].into_iter().collect::<HashSet<_>>();
        let set1 = vec![3, 4].into_iter().collect();
        let set2 = vec![4, 5].into_iter().collect();

        merge_into_hashset(&mut accumulator, vec![set1, set2]);
        assert_eq!(accumulator.len(), 5);
    }
}
