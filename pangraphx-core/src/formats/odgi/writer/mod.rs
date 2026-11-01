//! ODGI writer: builds the node numbering and the edge and step indexes, then
//! encodes the node records in parallel windows and writes the paths.

mod edges;
mod numbering;
mod steps;

#[cfg(test)]
pub(super) use edges::SMALL_NODE_DEGREE;
#[cfg(test)]
pub(super) use steps::MAX_RANK_HINTS;

use crate::core::core_types::NodeId;
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::odgi::encoding::{push_packed, push_u64};
use crate::formats::odgi::{ODGI_MAGIC, STEP_END, STEP_START};
use edges::{EDGE_TYPE_BITS, EdgeIndex};
use log::warn;
use numbering::Numbering;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::io::Write;
use steps::{StepIndex, StepNumber};

/// Target amount of output encoded per parallel task.
const CHUNK_BYTES: usize = 1 << 20;

/// Most chunks encoded per window when there are fewer threads (see
/// `OdgiWriter::write_records`).
const MAX_WINDOW_CHUNKS: usize = 64;

/// Up to this many entries, the decoding table is searched linearly.
pub(super) const SMALL_DECODING: usize = 16;

/// Per-task buffers for encoding node records.
#[derive(Default)]
struct Scratch {
    values: Vec<u64>,
    /// Rank lookup hints for the previous and next steps (see [`StepIndex::rank_after`]).
    prev_hints: Vec<(NodeId, usize)>,
    next_hints: Vec<(NodeId, usize)>,
    decoding: Vec<u64>,
    /// Index of every `decoding` entry, filled once the table is large.
    decoding_index: FxHashMap<u64, u64>,
}

impl Scratch {
    fn clear_decoding(&mut self) {
        if self.decoding.len() > SMALL_DECODING {
            self.decoding_index.clear();
        }
        self.decoding.clear();
    }

    /// Index of `other_id` in the node's decoding table, adding it if needed.
    fn encode(&mut self, own_id: u64, other_id: u64) -> u64 {
        let delta = match other_id.cmp(&own_id) {
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => (other_id - own_id) << 1 | 1,
            std::cmp::Ordering::Less => (own_id - other_id) << 1,
        };
        let next = self.decoding.len() as u64;
        if self.decoding.len() <= SMALL_DECODING {
            if let Some(index) = self.decoding.iter().position(|&entry| entry == delta) {
                return index as u64;
            }
            self.decoding.push(delta);
            if self.decoding.len() > SMALL_DECODING {
                self.decoding_index
                    .extend((0..).zip(self.decoding.iter()).map(|(i, &d)| (d, i)));
            }
            return next;
        }
        *self.decoding_index.entry(delta).or_insert_with(|| {
            self.decoding.push(delta);
            next
        })
    }
}

struct OdgiWriter<'a, I> {
    graph: &'a CoreGraphDTO,
    numbering: Numbering,
    edges: EdgeIndex,
    steps: StepIndex<I>,
}

impl<I: StepNumber> OdgiWriter<'_, I> {
    fn header(&self) -> Vec<u8> {
        let ranks = || {
            (0..self.numbering.records)
                .filter_map(|r| self.numbering.node_at(r).map(|_| r as u64 + 1))
        };
        let mut buf = Vec::with_capacity(60);
        buf.extend_from_slice(&ODGI_MAGIC.to_be_bytes());
        push_u64(&mut buf, ranks().next_back().unwrap_or(0));
        push_u64(&mut buf, ranks().next().unwrap_or(0));
        push_u64(&mut buf, self.numbering.records as u64);
        push_u64(&mut buf, self.edges.edge_count as u64);
        push_u64(&mut buf, self.graph.paths.len() as u64);
        push_u64(&mut buf, self.graph.paths.len() as u64);
        push_u64(&mut buf, 0); // id increment
        buf
    }

    /// Appends the record of node rank `rank`.
    fn push_record(&self, rank: usize, buf: &mut Vec<u8>, scratch: &mut Scratch) {
        let Some(node) = self.numbering.node_at(rank) else {
            // A deleted node: empty sequence, id 0 and three empty vectors.
            push_u64(buf, 0);
            push_u64(buf, 0);
            for _ in 0..3 {
                push_packed(buf, &[]);
            }
            return;
        };
        let own_id = rank as u64 + 1;
        let sequence = &self.graph.nodes.as_slice()[node].sequence;
        push_u64(buf, sequence.len() as u64);
        buf.extend_from_slice(sequence);
        push_u64(buf, own_id);

        scratch.values.clear();
        let edges = &self.edges;
        for &record in &edges.records[edges.offsets[node]..edges.offsets[node + 1]] {
            // Records hold node ids, which fit in `usize`.
            #[allow(clippy::cast_possible_truncation)]
            let other = (record >> EDGE_TYPE_BITS) as usize;
            scratch.values.push(self.numbering.id(other));
            scratch.values.push(record & ((1 << EDGE_TYPE_BITS) - 1));
        }
        push_packed(buf, &scratch.values);

        scratch.values.clear();
        scratch.clear_decoding();
        let steps = &self.steps;
        if steps.offsets[node] < steps.offsets[node + 1] {
            // odgi's `create_step` encodes the node itself before linking any step.
            scratch.encode(own_id, own_id);
        }
        // Steps come in increasing global order, so lookups resume where the last ended.
        scratch.prev_hints.clear();
        scratch.next_hints.clear();
        let mut path_end = 0;
        for &slot in &steps.slots[steps.offsets[node]..steps.offsets[node + 1]] {
            let global = slot.index();
            let (path_index, position) = steps.locate_after(&mut path_end, global);
            let path = &self.graph.paths[path_index].steps;
            let is_start = position == 0;
            let is_end = position + 1 == path.len();
            let (prev_id, prev_rank) = if is_start {
                (own_id, 0)
            } else {
                let prev = path[position - 1].node_id();
                (
                    self.numbering.id(prev),
                    steps.rank_after(&mut scratch.prev_hints, prev, global - 1),
                )
            };
            let (next_id, next_rank) = if is_end {
                (own_id, 0)
            } else {
                let next = path[position + 1].node_id();
                (
                    self.numbering.id(next),
                    steps.rank_after(&mut scratch.next_hints, next, global + 1),
                )
            };
            let step_type = u64::from(path[position].is_reverse())
                | (u64::from(is_start) * STEP_START)
                | (u64::from(is_end) * STEP_END);
            let prev = scratch.encode(own_id, prev_id);
            let next = scratch.encode(own_id, next_id);
            scratch.values.extend_from_slice(&[
                path_index as u64 + 1,
                step_type,
                prev,
                prev_rank,
                next,
                next_rank,
            ]);
        }
        push_packed(buf, &scratch.decoding);
        push_packed(buf, &scratch.values);
    }

    /// Node ranks per parallel task so that a chunk holds about [`CHUNK_BYTES`].
    fn ranks_per_chunk(&self) -> usize {
        let records = self.numbering.records.max(1);
        let sequence_bytes: usize = self
            .graph
            .nodes
            .par_iter()
            .map(|node| node.sequence.len())
            .sum();
        let average =
            (sequence_bytes + 16 * self.edges.records.len() + 48 * self.steps.slots.len())
                / records
                + 100;
        (CHUNK_BYTES / average).clamp(1, 1 << 14)
    }

    /// Encodes the node records of ranks `window` in parallel chunks.
    fn encode_window(&self, window: std::ops::Range<usize>, per_chunk: usize) -> Vec<Vec<u8>> {
        window
            .clone()
            .into_par_iter()
            .step_by(per_chunk)
            .map(|start| {
                let mut buf = Vec::new();
                let mut scratch = Scratch::default();
                for rank in start..(start + per_chunk).min(window.end) {
                    self.push_record(rank, &mut buf, &mut scratch);
                }
                buf
            })
            .collect()
    }

    /// Writes the node records window by window, encoding the next window on
    /// the thread pool while this thread writes the current one.
    fn write_records(&self, writer: &mut dyn Write) -> PanResult<()> {
        let per_chunk = self.ranks_per_chunk();
        let records = self.numbering.records;
        // Several chunks per thread keep the threads busy when records vary in
        // size; the cap bounds the two windows held in memory on many cores.
        let threads = rayon::current_num_threads();
        let window = per_chunk * (threads * 4).min(MAX_WINDOW_CHUNKS).max(threads);
        let mut current = self.encode_window(0..window.min(records), per_chunk);
        for start in (window..records).step_by(window) {
            let mut next = Vec::new();
            rayon::in_place_scope(|scope| {
                scope.spawn(|_| {
                    next = self.encode_window(start..(start + window).min(records), per_chunk);
                });
                current.iter().try_for_each(|buf| writer.write_all(buf))
            })?;
            current = next;
        }
        for buf in current {
            writer.write_all(&buf)?;
        }
        Ok(())
    }

    fn write_paths(&self, writer: &mut dyn Write) -> PanResult<()> {
        let mut buf = Vec::new();
        for (index, path) in self.graph.paths.iter().enumerate() {
            let step_handle = |position: usize| {
                let step = path.steps[position];
                let rank = self.numbering.id(step.node_id()) - 1;
                let global = self.steps.path_starts[index] + position;
                (
                    rank << 1 | u64::from(step.is_reverse()),
                    self.steps.rank(step.node_id(), global),
                )
            };
            let (first, last) = match path.steps.len() {
                0 => ((0, 0), (0, 0)),
                len => (step_handle(0), step_handle(len - 1)),
            };
            push_u64(&mut buf, path.steps.len() as u64);
            for value in [first.0, first.1, last.0, last.1, path.name.len() as u64] {
                push_u64(&mut buf, value);
            }
            buf.extend_from_slice(&path.name);
            if buf.len() >= CHUNK_BYTES {
                writer.write_all(&buf)?;
                buf.clear();
            }
        }
        writer.write_all(&buf)?;
        Ok(())
    }
}

/// Writes `graph`, indexing its steps with `I`.
pub(super) fn write_odgi<I: StepNumber>(
    graph: &CoreGraphDTO,
    writer: &mut dyn Write,
) -> PanResult<()> {
    let node_count = graph.nodes.len();
    // The indexes are independent and partly sequential, so build them side by side.
    let ((numbering, edges), steps) = rayon::join(
        || {
            rayon::join(
                || Numbering::new(graph),
                || EdgeIndex::new(node_count, &graph.edges),
            )
        },
        || StepIndex::new(node_count, &graph.paths),
    );
    let odgi = OdgiWriter::<I> {
        graph,
        numbering,
        edges,
        steps: steps?,
    };
    writer.write_all(&odgi.header())?;
    odgi.write_records(writer)?;
    odgi.write_paths(writer)
}

/// Writes `graph` in ODGI format; overlaps are dropped.
pub(super) fn write(graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
    if let Some(message) = graph.dangling_reference() {
        return Err(PanGraphXError::Serialize(message));
    }
    if graph.edges.par_iter().any(|edge| edge.overlap != 0)
        || graph
            .paths
            .par_iter()
            .any(|path| path.overlaps.iter().any(|&overlap| overlap != 0))
    {
        warn!("ODGI files don't support overlaps; overlaps are dropped.");
    }
    let total_steps: usize = graph.paths.iter().map(|path| path.steps.len()).sum();
    if u32::try_from(total_steps).is_ok() {
        write_odgi::<u32>(graph, writer)
    } else {
        write_odgi::<u64>(graph, writer)
    }
}
