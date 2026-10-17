//! Lookup from external integer node ids (VG ids, numeric GFA segment names)
//! to dense internal [`NodeId`]s.

use crate::core::core_types::NodeId;
use rustc_hash::{FxBuildHasher, FxHashMap};

/// Maps external integer ids to dense internal ids; the internal id of an
/// external id is its position in the input slice.
///
/// External ids are usually compact (e.g. `1..=n`), so a flat lookup table
/// replaces hashing for the tens of millions of path-step lookups; sparse ids
/// fall back to a hash map.
pub enum IdMap {
    Dense { min_id: i64, table: Vec<NodeId> },
    Sparse(FxHashMap<i64, NodeId>),
}

/// Marks an unused slot in [`IdMap::Dense`].
const VACANT: NodeId = NodeId::MAX;

impl IdMap {
    /// Builds the map from external ids in internal-id order.
    ///
    /// # Errors
    /// Returns the first external id that occurs more than once.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn from_ids(ids: &[i64]) -> Result<Self, i64> {
        let (Some(&min_id), Some(&max_id)) = (ids.iter().min(), ids.iter().max()) else {
            return Ok(Self::Sparse(FxHashMap::default()));
        };
        let span = max_id.abs_diff(min_id).saturating_add(1);
        let dense_limit = ids.len().saturating_mul(2).saturating_add(1024) as u64;
        if span <= dense_limit {
            let mut table = vec![VACANT; span as usize];
            for (internal, &id) in ids.iter().enumerate() {
                let slot = &mut table[id.abs_diff(min_id) as usize];
                if *slot != VACANT {
                    return Err(id);
                }
                *slot = internal;
            }
            Ok(Self::Dense { min_id, table })
        } else {
            let mut map = FxHashMap::with_capacity_and_hasher(ids.len(), FxBuildHasher);
            for (internal, &id) in ids.iter().enumerate() {
                if map.insert(id, internal).is_some() {
                    return Err(id);
                }
            }
            Ok(Self::Sparse(map))
        }
    }

    /// Returns the internal id of `id`, if it is mapped.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn get(&self, id: i64) -> Option<NodeId> {
        match self {
            Self::Dense { min_id, table } => {
                if id < *min_id {
                    return None;
                }
                table
                    .get(id.abs_diff(*min_id) as usize)
                    .copied()
                    .filter(|&i| i != VACANT)
            }
            Self::Sparse(map) => map.get(&id).copied(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_and_sparse_lookups() {
        let dense = IdMap::from_ids(&[5, 3, 4]).unwrap();
        assert!(matches!(dense, IdMap::Dense { .. }));
        let sparse = IdMap::from_ids(&[i64::MIN, 0, i64::MAX]).unwrap();
        assert!(matches!(sparse, IdMap::Sparse(_)));

        assert_eq!(dense.get(5), Some(0));
        assert_eq!(dense.get(3), Some(1));
        assert_eq!(dense.get(2), None);
        assert_eq!(dense.get(6), None);
        assert_eq!(sparse.get(i64::MAX), Some(2));
        assert_eq!(sparse.get(1), None);
    }

    #[test]
    fn duplicates_are_reported() {
        assert!(matches!(IdMap::from_ids(&[1, 2, 1]), Err(1)));
        assert!(matches!(IdMap::from_ids(&[0, i64::MAX, 0]), Err(0)));
    }

    #[test]
    fn empty_map() {
        assert_eq!(IdMap::from_ids(&[]).unwrap().get(0), None);
    }
}
