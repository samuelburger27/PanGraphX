//! Streaming k-mer extraction from a variation graph.
//!
//! Walkers roll a [`RollingWindow`] over graph sequence and report every k-mer
//! and (k+1)-mer to a [`WindowSink`]; nothing is materialized per path.
//!
//! * Paths are spelled as the concatenation of their oriented node sequences
//!   (minus overlaps), so k-mers span node boundaries. Long paths are split
//!   into chunks of about [`CHUNK_BASES`] that are processed independently.
//! * The full topology is walked on the bidirected graph: from every oriented
//!   node, all walks are followed just far enough to complete the windows that
//!   start in that node.

use std::collections::HashSet;
use std::ops::Range;

use log::debug;
use rayon::prelude::*;

use super::kmer_set::{KmerBuildHasher, ShardBuffer, ShardedSet};
use super::word::{KmerWord, RollingWindow, Spelled};
use crate::core::core_types::{Orientation, Path};
use crate::core::graph::CoreGraph;
use crate::core::graph_utils::oriented_bases;
use crate::error::PanResult;

/// Target number of bases per path chunk processed as one parallel task.
pub const CHUNK_BASES: usize = 1 << 20;

/// Receives the windows found by a walker.
pub trait WindowSink<W> {
    /// A k-mer occurrence, as spelled on the walk.
    fn kmer(&mut self, kmer: Spelled<W>);
    /// A canonical (k+1)-mer occurrence, i.e. a de Bruijn edge.
    fn edge(&mut self, edge: W);
    /// The walk hit a non-nucleotide base; the next k-mer is not adjacent to
    /// the previous one.
    fn gap(&mut self) {}
}

/// Collects the distinct k-mers and edges of the graph across threads.
pub struct DbgSets<W> {
    pub kmers: ShardedSet<W>,
    pub edges: ShardedSet<W>,
}

impl<W: KmerWord> DbgSets<W> {
    pub fn new() -> Self {
        Self {
            kmers: ShardedSet::new(),
            edges: ShardedSet::new(),
        }
    }

    pub fn sink(&self) -> SetSink<'_, W> {
        SetSink {
            kmers: self.kmers.buffer(),
            edges: self.edges.buffer(),
        }
    }
}

/// A per-thread [`WindowSink`] inserting into [`DbgSets`].
pub struct SetSink<'a, W: KmerWord> {
    kmers: ShardBuffer<'a, W>,
    edges: ShardBuffer<'a, W>,
}

impl<W: KmerWord> WindowSink<W> for SetSink<'_, W> {
    #[inline]
    fn kmer(&mut self, kmer: Spelled<W>) {
        self.kmers.insert(kmer.canonical());
    }

    #[inline]
    fn edge(&mut self, edge: W) {
        self.edges.insert(edge);
    }
}

/// Converts an overlap length to `usize`, saturating.
fn overlap_len(overlap: u32) -> usize {
    usize::try_from(overlap).unwrap_or(usize::MAX)
}

/// A contiguous range of steps of one path, processed as one task.
pub struct PathChunk<'a> {
    /// Index of the path in [`CoreGraph::sorted_paths`].
    pub path_index: usize,
    pub path: &'a Path,
    pub steps: Range<usize>,
}

impl CoreGraph {
    /// Paths in name order, so results do not depend on hash map order.
    pub(crate) fn sorted_paths(&self) -> Vec<&Path> {
        let mut paths: Vec<&Path> = self.paths().iter().collect();
        paths.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        paths
    }

    /// Splits each path into step ranges of about [`CHUNK_BASES`] bases, in
    /// path order.
    pub(crate) fn path_chunks<'a>(&self, paths: &[&'a Path]) -> Vec<PathChunk<'a>> {
        self.path_chunks_with(paths, CHUNK_BASES)
    }

    /// Splits each path into step ranges of about `chunk_bases` bases (at
    /// least one step each), in path order.
    pub(crate) fn path_chunks_with<'a>(
        &self,
        paths: &[&'a Path],
        chunk_bases: usize,
    ) -> Vec<PathChunk<'a>> {
        let mut chunks = Vec::new();
        for (path_index, &path) in paths.iter().enumerate() {
            let mut start = 0;
            let mut bases = 0;
            for (i, step) in path.steps.iter().enumerate() {
                bases += self.nodes()[step.node_id()].sequence.len();
                if bases >= chunk_bases {
                    chunks.push(PathChunk {
                        path_index,
                        path,
                        steps: start..i + 1,
                    });
                    start = i + 1;
                    bases = 0;
                }
            }
            if start < path.steps.len() {
                chunks.push(PathChunk {
                    path_index,
                    path,
                    steps: start..path.steps.len(),
                });
            }
        }
        chunks
    }

    /// Reports every window of `path` that *starts* within the steps of
    /// `steps`. Reads on past the end of the range just far enough (k bases)
    /// to complete those windows, so consecutive chunks together report each
    /// window of the path exactly once, in order.
    pub(crate) fn walk_path_chunk<W: KmerWord>(
        &self,
        path: &Path,
        steps: Range<usize>,
        k: usize,
        sink: &mut impl WindowSink<W>,
    ) {
        let mut window = RollingWindow::<W>::new(k);
        // Bases read after the end of the range.
        let mut past_end = 0;
        for i in steps.start..path.steps.len() {
            let step = &path.steps[i];
            let outside = i >= steps.end;
            let skip = i
                .checked_sub(1)
                .and_then(|prev| path.overlaps.get(prev))
                .map_or(0, |&o| overlap_len(o));
            let sequence = &self.nodes()[step.node_id()].sequence;
            for base in oriented_bases(sequence, step.orientation(), skip) {
                if outside {
                    past_end += 1;
                }
                if !window.push(base) {
                    sink.gap();
                    if outside {
                        // No window starting in the range can cross this base.
                        return;
                    }
                    continue;
                }
                // The k-mer ending here starts `k - 1` bases back and the
                // (k+1)-mer `k` bases back.
                if past_end < k
                    && let Some(kmer) = window.kmer()
                {
                    sink.kmer(kmer);
                }
                if let Some(edge) = window.edge() {
                    sink.edge(edge);
                }
                if past_end >= k {
                    return;
                }
            }
        }
    }

    /// Walks the chunks of `paths` in windows of one chunk per thread and
    /// hands the results to `stitch` in path order.
    ///
    /// The chunks of a window are walked in parallel, each into a fresh sink
    /// from `new_sink`; `stitch` then receives every sink of the window in
    /// order, together with whether its chunk is the last of its path. Only
    /// one window of sinks is alive at a time, so the memory of the sinks is
    /// bounded however long the paths are.
    ///
    /// # Errors
    /// Returns the first error of `stitch`.
    pub(crate) fn walk_chunks_ordered<W, S>(
        &self,
        paths: &[&Path],
        chunk_bases: usize,
        k: usize,
        new_sink: impl Fn() -> S + Sync,
        mut stitch: impl FnMut(&PathChunk<'_>, S, bool) -> PanResult<()>,
    ) -> PanResult<()>
    where
        W: KmerWord,
        S: WindowSink<W> + Send,
    {
        let chunks = self.path_chunks_with(paths, chunk_bases);
        let window = rayon::current_num_threads().max(1);
        for (w, group) in chunks.chunks(window).enumerate() {
            let sinks: Vec<S> = group
                .par_iter()
                .map(|chunk| {
                    let mut sink = new_sink();
                    self.walk_path_chunk(chunk.path, chunk.steps.clone(), k, &mut sink);
                    sink
                })
                .collect();
            for (i, (chunk, sink)) in group.iter().zip(sinks).enumerate() {
                let last_of_path = chunks
                    .get(w * window + i + 1)
                    .is_none_or(|next| next.path_index != chunk.path_index);
                stitch(chunk, sink, last_of_path)?;
            }
        }
        Ok(())
    }

    /// Streams the windows of all paths into `sets`, in parallel over chunks.
    pub(crate) fn extract_path_windows<W: KmerWord>(&self, k: usize, sets: &DbgSets<W>) {
        let paths = self.sorted_paths();
        let chunks = self.path_chunks(&paths);
        debug!(
            "extracting k-mers from {} paths in {} chunks",
            paths.len(),
            chunks.len()
        );
        chunks.par_iter().for_each_init(
            || sets.sink(),
            |sink, chunk| self.walk_path_chunk(chunk.path, chunk.steps.clone(), k, sink),
        );
    }

    /// Streams the windows of every walk through the graph into `sets`.
    pub(crate) fn extract_topology_windows<W: KmerWord>(&self, k: usize, sets: &DbgSets<W>) {
        let adjacency = BidirectedAdjacency::new(self);
        let frames: usize = (0..2 * self.nodes().len())
            .into_par_iter()
            .map_init(
                || (sets.sink(), TopologyScratch::default()),
                |(sink, scratch), side| self.walk_from_side(&adjacency, side, k, sink, scratch),
            )
            .sum();
        debug!(
            "full topology: followed {frames} walk extensions from {} oriented nodes",
            2 * self.nodes().len()
        );
    }

    /// Reports every window whose first base lies in the oriented node
    /// `start` (see [`side`]), following all walks out of it for at most k
    /// bases. Returns the number of walk extensions followed.
    fn walk_from_side<W: KmerWord>(
        &self,
        adjacency: &BidirectedAdjacency,
        start: usize,
        k: usize,
        sink: &mut impl WindowSink<W>,
        scratch: &mut TopologyScratch<W>,
    ) -> usize {
        let (node, orientation) = side_node(start);
        let mut window = RollingWindow::<W>::new(k);
        for base in oriented_bases(&self.nodes()[node].sequence, orientation, 0) {
            if !window.push(base) {
                continue;
            }
            if let Some(kmer) = window.kmer() {
                sink.kmer(kmer);
            }
            if let Some(edge) = window.edge() {
                sink.edge(edge);
            }
        }
        if window.len() == 0 {
            // The node ends in a non-nucleotide: nothing starting in it can
            // be extended.
            return 0;
        }

        scratch.reset();
        for &(next, overlap) in adjacency.successors(start) {
            scratch.stack.push(Frame {
                side: next,
                overlap,
                window,
                past_end: 0,
            });
        }
        let mut followed = 0;
        while let Some(frame) = scratch.stack.pop() {
            followed += 1;
            let Frame {
                side,
                overlap,
                mut window,
                mut past_end,
            } = frame;
            let (node, orientation) = side_node(side);
            let mut extend = true;
            for base in oriented_bases(&self.nodes()[node].sequence, orientation, overlap) {
                past_end += 1;
                if !window.push(base) {
                    extend = false;
                    break;
                }
                if past_end < k
                    && let Some(kmer) = window.kmer()
                {
                    sink.kmer(kmer);
                }
                if let Some(edge) = window.edge() {
                    sink.edge(edge);
                }
                if past_end >= k {
                    extend = false;
                    break;
                }
            }
            if !extend {
                continue;
            }
            for &(next, overlap) in adjacency.successors(side) {
                // Walks reaching the same oriented node through the same
                // overlap with the same window after the same distance
                // continue identically.
                let (code, len) = window.state();
                if scratch.seen.insert((next, overlap, past_end, code, len)) {
                    scratch.stack.push(Frame {
                        side: next,
                        overlap,
                        window,
                        past_end,
                    });
                }
            }
        }
        followed
    }
}

/// Index of an oriented node: `2 * node + 1` for the reverse strand.
#[inline]
const fn side(node: usize, orientation: Orientation) -> usize {
    2 * node
        + match orientation {
            Orientation::Forward => 0,
            Orientation::Reverse => 1,
        }
}

#[inline]
fn side_node(side: usize) -> (usize, Orientation) {
    (side / 2, Orientation::from(side % 2 == 1))
}

/// Successors of every oriented node in compressed sparse row form.
///
/// An edge `(a, o1) -> (b, o2)` can be traversed on either strand, so it is
/// stored both as `(a, o1) -> (b, o2)` and as its twin `(b, !o2) -> (a, !o1)`.
struct BidirectedAdjacency {
    offsets: Vec<usize>,
    /// `(oriented successor, overlap)`.
    targets: Vec<(usize, usize)>,
}

impl BidirectedAdjacency {
    fn new(graph: &CoreGraph) -> Self {
        let sides = 2 * graph.nodes().len();
        let mut arcs: Vec<(usize, usize, usize)> = Vec::with_capacity(2 * graph.edges().len());
        for edge in graph.edges() {
            let from = side(edge.from_node, edge.from_orient);
            let to = side(edge.to_node, edge.to_orient);
            let overlap = overlap_len(edge.overlap);
            arcs.push((from, to, overlap));
            arcs.push((to ^ 1, from ^ 1, overlap));
        }
        // Drops twins that a file lists explicitly, and self-twins.
        arcs.par_sort_unstable();
        arcs.dedup();

        let mut offsets = vec![0; sides + 1];
        for &(from, _, _) in &arcs {
            offsets[from + 1] += 1;
        }
        for i in 0..sides {
            offsets[i + 1] += offsets[i];
        }
        let targets = arcs.into_iter().map(|(_, to, ov)| (to, ov)).collect();
        Self { offsets, targets }
    }

    fn successors(&self, side: usize) -> &[(usize, usize)] {
        &self.targets[self.offsets[side]..self.offsets[side + 1]]
    }
}

/// A pending walk extension into an oriented node.
#[derive(Clone, Copy)]
struct Frame<W> {
    side: usize,
    /// Bases of the node already covered by the previous one.
    overlap: usize,
    window: RollingWindow<W>,
    /// Bases read after the start node.
    past_end: usize,
}

/// Per-thread buffers of the topology walk, reused across start nodes.
struct TopologyScratch<W> {
    stack: Vec<Frame<W>>,
    seen: HashSet<(usize, usize, usize, W, usize), KmerBuildHasher>,
}

impl<W> Default for TopologyScratch<W> {
    fn default() -> Self {
        Self {
            stack: Vec::new(),
            seen: HashSet::default(),
        }
    }
}

impl<W> TopologyScratch<W> {
    /// Empties the buffers. `HashSet::clear` costs time proportional to the
    /// capacity, so a set grown by one dense region is dropped instead.
    fn reset(&mut self) {
        self.stack.clear();
        if self.seen.capacity() > 4096 {
            self.seen = HashSet::default();
        } else {
            self.seen.clear();
        }
    }
}
