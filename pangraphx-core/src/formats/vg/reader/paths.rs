//! Path steps and mapping ranks, accumulated across the `Graph` chunks a path is split over.

use crate::core::core_types::{Path, Step};
use crate::error::{PanGraphXError, PanResult};
use crate::proto_gen::vg_read;
use log::debug;

/// A path step as read from VG: `node_id * 2 + is_reverse`, half the size of
/// the two fields. Unpacking is an arithmetic shift, so negative ids survive.
pub(super) type PackedStep = i64;

/// Packs a VG node id and strand, or `None` if the id lies outside ±2^62.
/// handlegraph packs handles the same way, so real vg ids never get close.
pub(super) fn pack_step(node_id: i64, is_reverse: bool) -> Option<PackedStep> {
    node_id
        .checked_mul(2)
        .map(|id| id | PackedStep::from(is_reverse))
}

/// The error message for a node id [`pack_step`] cannot pack. Node ids are
/// checked against the same range, so the limit does not depend on whether
/// a path visits the node.
pub(super) fn node_id_out_of_range(node_id: i64) -> String {
    format!("VG node id {node_id} is outside the supported range ±2^62")
}

/// The VG node id and strand of a packed step.
pub(super) const fn unpack_step(step: PackedStep) -> (i64, bool) {
    (step >> 1, step & 1 == 1)
}

/// The mapping ranks of one path chunk.
pub(super) enum ChunkRanks {
    /// `first, first + 1, …`, one per step; what vg and `PanGraphX` write.
    Consecutive(i64),
    /// Any other ranks, one per step.
    Explicit(Vec<i64>),
}

/// The part of one path stored in a single `Graph` message.
pub(super) struct PathChunk {
    pub(super) name: Vec<u8>,
    /// Number of steps; they are stored in the batch the chunk was decoded in.
    pub(super) len: usize,
    pub(super) ranks: ChunkRanks,
}

impl PathChunk {
    /// Decodes `path`, appending its steps to `steps`.
    pub(super) fn decode(path: vg_read::Path, steps: &mut Vec<PackedStep>) -> PanResult<Self> {
        let name_error =
            |message: &str| PanGraphXError::Parse(format!("Path {}: {message}", path.name));
        let first = path.mapping.first().map_or(0, |m| m.rank);
        let mut next_rank = Some(first);
        let mut consecutive = true;
        steps.reserve(path.mapping.len());
        for mapping in &path.mapping {
            let position = mapping
                .position
                .as_ref()
                .ok_or_else(|| name_error("mapping is missing its position"))?;
            let step = pack_step(position.node_id, position.is_reverse)
                .ok_or_else(|| name_error(&node_id_out_of_range(position.node_id)))?;
            steps.push(step);
            consecutive &= next_rank == Some(mapping.rank);
            next_rank = next_rank.and_then(|rank| rank.checked_add(1));
        }
        let ranks = if consecutive {
            ChunkRanks::Consecutive(first)
        } else {
            ChunkRanks::Explicit(path.mapping.iter().map(|m| m.rank).collect())
        };
        Ok(Self {
            name: path.name.into_bytes(),
            len: path.mapping.len(),
            ranks,
        })
    }
}

/// What the mapping ranks of a path read so far say about its step order.
pub(super) enum PathRanks {
    /// Every rank equals its 1-based position, so the steps are in rank
    /// order and no rank needs storing.
    Implicit,
    /// A rank is unset (≤ 0) or repeated, so ranks define no order and
    /// stream order is kept.
    StreamOrder,
    /// Positive ranks that may reorder the path, one per step.
    Explicit(Vec<i64>),
}

/// Mappings of one path, accumulated across all `Graph` chunks it is split over.
pub(super) struct VgPath {
    name: Vec<u8>,
    steps: Vec<PackedStep>,
    ranks: PathRanks,
}

impl VgPath {
    pub(super) const fn new(name: Vec<u8>) -> Self {
        Self {
            name,
            steps: Vec::new(),
            ranks: PathRanks::Implicit,
        }
    }

    /// Appends the next chunk of this path in stream order.
    pub(super) fn append(&mut self, ranks: ChunkRanks, steps: &[PackedStep]) {
        let start = self.steps.len();
        match ranks {
            ChunkRanks::Consecutive(first) => {
                let continues = matches!(self.ranks, PathRanks::Implicit)
                    && usize::try_from(first).is_ok_and(|first| first == start + 1);
                if !continues && !matches!(self.ranks, PathRanks::StreamOrder) {
                    let ranks = std::iter::successors(Some(first), |rank| rank.checked_add(1));
                    for (offset, rank) in ranks.take(steps.len()).enumerate() {
                        self.push_rank(start + offset, rank);
                    }
                }
            }
            ChunkRanks::Explicit(ranks) => {
                for (offset, rank) in ranks.into_iter().enumerate() {
                    self.push_rank(start + offset, rank);
                }
            }
        }
        self.steps.extend_from_slice(steps);
    }

    /// Records the rank of the step at 0-based `position`.
    fn push_rank(&mut self, position: usize, rank: i64) {
        let next = match &mut self.ranks {
            PathRanks::StreamOrder => return,
            _ if rank <= 0 => PathRanks::StreamOrder,
            PathRanks::Explicit(ranks) => {
                ranks.push(rank);
                return;
            }
            PathRanks::Implicit => match usize::try_from(rank) {
                Ok(rank) if rank == position + 1 => return,
                // Ranks `1..=position` already belong to the earlier steps.
                Ok(rank) if rank <= position => {
                    debug!(
                        "VG path {} has repeated mapping ranks, keeping stream order",
                        String::from_utf8_lossy(&self.name)
                    );
                    PathRanks::StreamOrder
                }
                _ => {
                    let mut ranks: Vec<i64> = (1..).take(position).collect();
                    ranks.push(rank);
                    PathRanks::Explicit(ranks)
                }
            },
        };
        self.ranks = next;
    }

    /// Converts the path with `convert`, in path order: sorted by rank when
    /// every mapping carries a distinct positive one, otherwise in stream order.
    pub(super) fn into_path(
        self,
        convert: impl Fn(PackedStep) -> PanResult<Step>,
    ) -> PanResult<Path> {
        let Self {
            name,
            mut steps,
            ranks,
        } = self;
        if let PathRanks::Explicit(ranks) = ranks
            && !ranks.is_sorted()
        {
            let mut ranked: Vec<(i64, PackedStep)> =
                ranks.into_iter().zip(steps.iter().copied()).collect();
            ranked.sort_unstable_by_key(|&(rank, _)| rank);
            if ranked.windows(2).any(|w| w[0].0 == w[1].0) {
                debug!(
                    "VG path {} has repeated mapping ranks, keeping stream order",
                    String::from_utf8_lossy(&name)
                );
            } else {
                steps = ranked.into_iter().map(|(_, step)| step).collect();
            }
        }
        // Collecting from a borrowing iterator gives the steps an allocation of
        // their own; the packed steps are freed on return.
        let steps = steps
            .iter()
            .map(|&step| convert(step))
            .collect::<PanResult<Vec<Step>>>()?;
        Ok(Path {
            name,
            steps,
            overlaps: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path read from chunks with the given mapping ranks; its steps visit
    /// nodes `1, 2, …` in stream order.
    fn ranked_path(chunks: &[&[i64]]) -> VgPath {
        let mut path = VgPath::new(b"p".to_vec());
        let mut node_id = 0;
        for ranks in chunks {
            let mapping = ranks
                .iter()
                .map(|&rank| {
                    node_id += 1;
                    vg_read::Mapping {
                        position: Some(vg_read::Position {
                            node_id,
                            is_reverse: false,
                        }),
                        rank,
                    }
                })
                .collect();
            let chunk = vg_read::Path {
                name: "p".to_string(),
                mapping,
            };
            let mut steps = Vec::new();
            let chunk = PathChunk::decode(chunk, &mut steps).unwrap();
            path.append(chunk.ranks, &steps);
        }
        path
    }

    fn node_order(path: VgPath) -> Vec<usize> {
        let path = path
            .into_path(|step| {
                let (vg_id, is_reverse) = unpack_step(step);
                Step::new(usize::try_from(vg_id).unwrap(), is_reverse.into())
            })
            .unwrap();
        path.steps.iter().map(|step| step.node_id()).collect()
    }

    #[test]
    fn test_sort_by_rank() {
        // Distinct ranks define the order, also across chunks.
        assert_eq!(node_order(ranked_path(&[&[3, 1, 2]])), vec![2, 3, 1]);
        assert_eq!(
            node_order(ranked_path(&[&[3, 4], &[1, 2]])),
            vec![3, 4, 1, 2]
        );

        // Unset ranks keep stream order.
        assert_eq!(node_order(ranked_path(&[&[2, 0, 1]])), vec![1, 2, 3]);

        // Increasing ranks with gaps are already in order.
        assert_eq!(node_order(ranked_path(&[&[2, 4], &[6]])), vec![1, 2, 3]);
        // Ranks repeated within one chunk keep stream order.
        assert_eq!(node_order(ranked_path(&[&[3, 3, 1]])), vec![1, 2, 3]);
    }

    #[test]
    fn test_positional_ranks_are_not_stored() {
        let path = ranked_path(&[&[1, 2], &[3, 4], &[], &[5]]);
        assert!(matches!(path.ranks, PathRanks::Implicit));
        assert_eq!(node_order(path), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_restarting_ranks_keep_stream_order() {
        // Ranks restarting in every chunk must not interleave the chunks, and
        // are recognized as repeated without storing them.
        let path = ranked_path(&[&[1, 2], &[1, 2]]);
        assert!(matches!(path.ranks, PathRanks::StreamOrder));
        assert_eq!(node_order(path), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_packed_steps() {
        for (node_id, is_reverse) in [(1, false), (7, true), (-3, true), (-(1 << 62), false)] {
            let step = pack_step(node_id, is_reverse).unwrap();
            assert_eq!(unpack_step(step), (node_id, is_reverse));
        }
        assert_eq!(pack_step(1 << 62, false), None);
    }
}
