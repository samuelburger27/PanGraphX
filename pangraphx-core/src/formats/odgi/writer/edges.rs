//! Edge records of every node, de-duplicated, as odgi stores them on both endpoints.

use crate::core::core_types::{Edge, NodeId, Orientation};
use crate::formats::odgi::{EDGE_ON_REV, EDGE_OTHER_REV, EDGE_TO_CURR};
use rayon::prelude::*;
use rustc_hash::FxHashSet;

/// Bits of the packed edge type in an [`EdgeIndex`] record.
pub(super) const EDGE_TYPE_BITS: u32 = 3;

/// Nodes per parallel task when de-duplicating edge records.
pub(super) const EDGE_DEDUP_NODES: usize = 1 << 12;

/// Up to this many records, a node's duplicates are found by linear scan.
pub(in crate::formats::odgi) const SMALL_NODE_DEGREE: usize = 16;

/// Edge records of every node, as odgi stores them on both endpoints.
pub(super) struct EdgeIndex {
    /// Records of node `v` are `records[offsets[v]..offsets[v + 1]]`.
    pub(super) offsets: Vec<usize>,
    /// `other node << EDGE_TYPE_BITS | packed edge type`.
    pub(super) records: Vec<u64>,
    /// Number of distinct edges.
    pub(super) edge_count: usize,
}

/// The edge a record of `node` stands for, as a pair of handles, identical for
/// an edge and its reverse complement.
pub(super) const fn canonical_edge(node: NodeId, record: u64) -> (u64, u64) {
    let packed = record & ((1 << EDGE_TYPE_BITS) - 1);
    let own = (node as u64) << 1 | (packed & EDGE_ON_REV) >> 1;
    let other = (record >> EDGE_TYPE_BITS) << 1 | (packed & EDGE_OTHER_REV);
    let (from, to) = if packed & EDGE_TO_CURR == 0 {
        (own, other)
    } else {
        (other, own)
    };
    let reverse = (to ^ 1, from ^ 1);
    if reverse.0 < from || (reverse.0 == from && reverse.1 < to) {
        reverse
    } else {
        (from, to)
    }
}

/// Moves the first record of every distinct edge of `node` to the front of
/// `records`, keeping their order, and returns how many there are.
pub(super) fn dedup_node_records(
    node: NodeId,
    records: &mut [u64],
    seen: &mut FxHashSet<(u64, u64)>,
) -> usize {
    let mut kept = 0;
    if records.len() <= SMALL_NODE_DEGREE {
        for i in 0..records.len() {
            let edge = canonical_edge(node, records[i]);
            if !records[..kept]
                .iter()
                .any(|&record| canonical_edge(node, record) == edge)
            {
                records[kept] = records[i];
                kept += 1;
            }
        }
    } else {
        seen.clear();
        for i in 0..records.len() {
            if seen.insert(canonical_edge(node, records[i])) {
                records[kept] = records[i];
                kept += 1;
            }
        }
    }
    kept
}

impl EdgeIndex {
    /// Lays out every edge on its endpoints in input order, then drops the
    /// later copies of duplicate edges and reverse complements (as odgi's
    /// `has_edge` does): every copy of an edge is stored on both of its
    /// endpoints, so de-duplicating each node's records on their own keeps the
    /// first copy on both sides.
    pub(super) fn new(node_count: usize, edges: &[Edge]) -> Self {
        let mut offsets = vec![0; node_count + 1];
        for edge in edges {
            offsets[edge.from_node + 1] += 1;
            if edge.to_node != edge.from_node {
                offsets[edge.to_node + 1] += 1;
            }
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut cursor = offsets.clone();
        let mut records = vec![0; offsets[node_count]];
        for edge in edges {
            let from_rev = u64::from(edge.from_orient == Orientation::Reverse);
            let to_rev = u64::from(edge.to_orient == Orientation::Reverse);
            records[cursor[edge.from_node]] =
                (edge.to_node as u64) << EDGE_TYPE_BITS | to_rev | from_rev << 1;
            cursor[edge.from_node] += 1;
            if edge.to_node != edge.from_node {
                records[cursor[edge.to_node]] = (edge.from_node as u64) << EDGE_TYPE_BITS
                    | from_rev
                    | to_rev << 1
                    | EDGE_TO_CURR;
                cursor[edge.to_node] += 1;
            }
        }
        drop(cursor);

        // De-duplicate the records of every node in parallel, in place.
        let mut kept = vec![0; node_count];
        let mut parts = Vec::new();
        let mut rest = records.as_mut_slice();
        for first in (0..node_count).step_by(EDGE_DEDUP_NODES) {
            let last = (first + EDGE_DEDUP_NODES).min(node_count);
            let (part, tail) = rest.split_at_mut(offsets[last] - offsets[first]);
            parts.push((first, part));
            rest = tail;
        }
        parts
            .into_par_iter()
            .zip(kept.par_chunks_mut(EDGE_DEDUP_NODES))
            .for_each(|((first, part), kept)| {
                let mut seen = FxHashSet::default();
                let base = offsets[first];
                for (i, kept) in kept.iter_mut().enumerate() {
                    let node = first + i;
                    let range = offsets[node] - base..offsets[node + 1] - base;
                    *kept = dedup_node_records(node, &mut part[range], &mut seen);
                }
            });

        // Close the gaps left by the dropped records.
        let mut end = 0;
        let mut edge_count = 0;
        for (node, &kept) in kept.iter().enumerate() {
            let start = offsets[node];
            records.copy_within(start..start + kept, end);
            edge_count += records[end..end + kept]
                .iter()
                .filter(|&&record| record & EDGE_TO_CURR == 0)
                .count();
            offsets[node] = end;
            end += kept;
        }
        offsets[node_count] = end;
        records.truncate(end);
        Self {
            offsets,
            records,
            edge_count,
        }
    }
}
