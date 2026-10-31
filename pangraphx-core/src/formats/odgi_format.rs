//! Native reader and writer for odgi's binary graph format (`.og`).
//!
//! The layout mirrors odgi's `graph_t::serialize` (libhandlegraph `Serializable`
//! with odgi's `node_t` records and DYNAMIC's `hacked_vector`). All integers are
//! little-endian except the leading magic number:
//!
//! ```text
//! u32 BE  magic (1988148666)
//! u64 ×7  max_node_id, min_node_id, node_count, edge_count, path_count,
//!         path_handle_next, id_increment
//! node_count × node record (node rank r has id r + 1 + id_increment):
//!   u64 sequence length, sequence bytes, u64 id (0 marks a deleted node)
//!   packed edges    (other node id, other_rev | on_rev << 1 | to_curr << 2) pairs
//!   packed decoding deltas to neighbor ids (0 = this node, odd = higher, even = lower)
//!   packed steps    (path, rev | start << 1 | end << 2 | del << 3,
//!                    prev, prev rank, next, next rank) where prev/next index decoding
//! path_count × path: u64 length, first step (u64 handle, u64 rank),
//!                    last step (u64 handle, u64 rank), u64 name length, name bytes
//! packed = u64 word count, words, u64 mask, u64 length, u8 width, u8 values per word
//! ```
//!
//! Edges are stored on both of their nodes (`to_curr` on the right one, self loops
//! once). A path is a linked list of steps; a step is named by its node handle
//! (`rank << 1 | is_reverse`) and its rank among the steps stored on that node.
//!
//! `docs/odgi_format.md` describes every field in detail, with a worked example
//! decoded from `odgi build` output; `utils/dump_odgi.py` prints a file's fields.

use crate::core::core_types::{Edge, NodeId, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use log::warn;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::HashMap;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};

/// Codec for odgi's binary graph format.
///
/// Node ids are written as odgi ids: numeric node names are kept when they are
/// distinct positive integers that fit a dense id range (as `odgi build` keeps
/// GFA segment ids), otherwise nodes are numbered `1..=n`. Parsed nodes are named
/// by their odgi id. Overlaps are not part of the format and are dropped.
pub struct ODGICodec;

/// libhandlegraph magic number of odgi's `graph_t`.
const ODGI_MAGIC: u32 = 1_988_148_666;

/// Values per edge record and per step record in a node.
const EDGE_RECORD_LEN: usize = 2;
const STEP_RECORD_LEN: usize = 6;

/// Bits of a packed edge.
const EDGE_OTHER_REV: u64 = 1;
const EDGE_ON_REV: u64 = 1 << 1;
const EDGE_TO_CURR: u64 = 1 << 2;

/// Bits of a step type.
const STEP_REV: u64 = 1;
const STEP_START: u64 = 1 << 1;
const STEP_END: u64 = 1 << 2;

/// Largest buffer reserved up front for a length read from the input, so a
/// corrupt length fails with a parse error instead of a huge allocation.
const MAX_PREALLOC: usize = 1 << 20;

/// Largest rank of a step on its node that is read or written (the
/// reader packs ranks into 31 bits).
const MAX_STEP_RANK: u32 = u32::MAX >> 1;

/// Marks a deleted node rank.
const VACANT: NodeId = NodeId::MAX;

/// Target amount of output encoded per parallel task.
const CHUNK_BYTES: usize = 1 << 20;

/// Most chunks encoded per window when there are fewer threads (see
/// `OdgiWriter::write_records`).
const MAX_WINDOW_CHUNKS: usize = 64;

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

fn truncated(error: io::Error) -> PanGraphXError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        truncated_file()
    } else {
        PanGraphXError::Io(error)
    }
}

fn read_u64<R: Read>(reader: &mut R) -> PanResult<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf).map_err(truncated)?;
    Ok(u64::from_le_bytes(buf))
}

fn to_usize(value: u64, what: &str) -> PanResult<usize> {
    usize::try_from(value)
        .map_err(|_| PanGraphXError::Parse(format!("ODGI {what} {value} is too large")))
}

fn read_bytes<R: Read>(reader: &mut R, len: u64) -> PanResult<Vec<u8>> {
    let mut buf = Vec::with_capacity(to_usize(len, "length")?.min(MAX_PREALLOC));
    reader.by_ref().take(len).read_to_end(&mut buf)?;
    if buf.len() as u64 != len {
        return Err(truncated_file());
    }
    Ok(buf)
}

fn truncated_file() -> PanGraphXError {
    PanGraphXError::Parse("Truncated ODGI file".to_string())
}

fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().unwrap_or_default())
}

/// Reads the fields of node records from an in-memory buffer.
struct Fields<'a> {
    bytes: &'a [u8],
}

impl<'a> Fields<'a> {
    fn take(&mut self, len: usize) -> PanResult<&'a [u8]> {
        let (head, tail) = self
            .bytes
            .split_at_checked(len)
            .ok_or_else(truncated_file)?;
        self.bytes = tail;
        Ok(head)
    }

    fn u64(&mut self) -> PanResult<u64> {
        self.take(8).map(le_u64)
    }

    fn u8(&mut self) -> PanResult<u8> {
        self.take(1)
            .map(|byte| byte.first().copied().unwrap_or_default())
    }

    fn len(&mut self, what: &str) -> PanResult<usize> {
        to_usize(self.u64()?, what)
    }

    /// Reads a DYNAMIC `hacked_vector` (fixed-width integers packed into
    /// 64-bit words) and unpacks its values into `values`.
    fn packed(&mut self, values: &mut Vec<u64>) -> PanResult<()> {
        let word_count = self.len("word count")?;
        let words = self.take(word_count.checked_mul(8).ok_or_else(truncated_file)?)?;
        let _mask = self.u64()?;
        let len = self.len("vector length")?;
        let width = u32::from(self.u8()?);
        let _per_word = self.u8()?;
        values.clear();
        if len == 0 {
            return Ok(());
        }
        if width == 0 || width > 64 {
            return Err(PanGraphXError::Parse(format!(
                "Invalid ODGI packed vector width {width}"
            )));
        }
        let per_word = (64 / width) as usize;
        if word_count < len.div_ceil(per_word) {
            return Err(PanGraphXError::Parse(
                "ODGI packed vector is shorter than its length".to_string(),
            ));
        }
        let mask = u64::MAX >> (64 - width);
        // `len` is bounded by the words in the buffer, so this reservation is too.
        values.reserve(len);
        for chunk in words.chunks_exact(8) {
            let mut word = le_u64(chunk);
            for _ in 0..per_word.min(len - values.len()) {
                values.push(word & mask);
                // A shift by 64 (width 64) wraps to 0, but then `per_word` is 1.
                word = word.wrapping_shr(width);
            }
            if values.len() == len {
                break;
            }
        }
        Ok(())
    }
}

/// Smallest possible node record: sequence length, id and three empty packed
/// vectors (word count, mask, length, width, values per word).
const MIN_RECORD_BYTES: u64 = 8 + 8 + 3 * (8 + 8 + 8 + 1 + 1);

/// Position and number of stored steps of a node record in a buffer.
struct RecordSpan {
    start: usize,
    end: usize,
    /// Stored steps, 0 for a deleted node (whose steps are ignored).
    steps: usize,
}

/// Locates the node record at `bytes[start..]`, or returns `Err(needed)` with a
/// lower bound on the buffer size if the buffer holds only part of it.
fn scan_record(bytes: &[u8], start: usize) -> Result<RecordSpan, u64> {
    let field = |pos: u64| {
        usize::try_from(pos)
            .ok()
            .and_then(|pos| pos.checked_add(start))
            .and_then(|pos| bytes.get(pos..pos.checked_add(8)?))
            .map(le_u64)
    };
    let sequence_len = field(0).ok_or(8_u64)?;
    let id_pos = 8_u64.saturating_add(sequence_len);
    let id = field(id_pos).ok_or_else(|| id_pos.saturating_add(8))?;
    let mut pos = id_pos + 8;
    let mut step_values = 0;
    for _ in 0..3 {
        let word_count = field(pos).ok_or_else(|| pos.saturating_add(8))?;
        // Word count, words, mask, then the vector length and width.
        let len_pos = pos
            .saturating_add(8)
            .saturating_add(word_count.saturating_mul(8))
            .saturating_add(8);
        let len = field(len_pos).ok_or_else(|| len_pos.saturating_add(8))?;
        let width = usize::try_from(len_pos.saturating_add(8))
            .ok()
            .and_then(|width_pos| bytes.get(width_pos.checked_add(start)?))
            .map_or(0, |&width| u64::from(width));
        // Count only values the words can hold, so a corrupt length cannot size
        // the step table (the decoder rejects such a vector).
        let capacity = match width {
            1..=64 => word_count.saturating_mul(64 / width),
            _ => 0,
        };
        step_values = len.min(capacity);
        // Length, width and values per word.
        pos = len_pos.saturating_add(8 + 1 + 1);
    }
    let end = usize::try_from(pos)
        .ok()
        .and_then(|size| size.checked_add(start))
        .filter(|&end| end <= bytes.len())
        .ok_or(pos)?;
    // The decoder checks that the length is a whole number of step records.
    let steps = if id == 0 {
        0
    } else {
        usize::try_from(step_values).unwrap_or(usize::MAX) / STEP_RECORD_LEN
    };
    Ok(RecordSpan { start, end, steps })
}

/// The part of a stored step needed to walk a path, in 8 bytes (a path's
/// steps are the bulk of an ODGI file, so this dominates the reader's memory).
#[derive(Clone, Copy)]
struct StepLink {
    /// Rank of the node holding the next step, [`StepLink::END`] for the last
    /// step of a path.
    next_node: u32,
    /// Rank of the next step among the steps of that node `<< 1 | is_reverse`.
    rank_rev: u32,
}

impl StepLink {
    /// `next_node` of a path's last step. Node ranks are below it (see `read_graph`).
    const END: u32 = u32::MAX;
}

/// A path's metadata record.
struct PathMeta {
    name: Vec<u8>,
    length: u64,
    first_handle: u64,
    first_rank: u64,
}

/// Steps of all nodes, indexed by node rank, used to walk the paths.
struct StepTable {
    /// Steps of node rank `r` are `links[offsets[r]..offsets[r + 1]]`.
    offsets: Vec<usize>,
    links: Vec<StepLink>,
    rank_to_node: Vec<NodeId>,
}

/// Paths a walker advances in lockstep. A path is a linked list scattered over
/// the node records, so every step is a dependent cache miss; interleaving
/// independent paths lets the CPU wait for several misses at once.
const WALK_LANES: usize = 16;

/// A path being walked.
struct Walk {
    /// Index of the path in the batch.
    index: usize,
    name: Vec<u8>,
    steps: Vec<Step>,
    length: usize,
    node: usize,
    rank: usize,
}

impl StepTable {
    fn broken(name: &[u8], what: &str) -> PanGraphXError {
        PanGraphXError::Parse(format!(
            "ODGI path {} {what}",
            String::from_utf8_lossy(name)
        ))
    }

    fn start(&self, index: usize, meta: PathMeta) -> PanResult<Walk> {
        let length = to_usize(meta.length, "path length")?;
        if length > self.links.len() {
            return Err(Self::broken(
                &meta.name,
                "is longer than the number of stored steps",
            ));
        }
        Ok(Walk {
            index,
            steps: Vec::with_capacity(length),
            length,
            node: to_usize(meta.first_handle >> 1, "node rank")?,
            rank: to_usize(meta.first_rank, "step rank")?,
            name: meta.name,
        })
    }

    /// Appends the current step of `walk` and moves to the next one.
    fn advance(&self, walk: &mut Walk) -> PanResult<()> {
        let (Some(&start), Some(&end)) =
            (self.offsets.get(walk.node), self.offsets.get(walk.node + 1))
        else {
            return Err(Self::broken(&walk.name, "refers to a missing node"));
        };
        if walk.rank >= end - start {
            return Err(Self::broken(&walk.name, "refers to a missing step"));
        }
        let link = self.links[start + walk.rank];
        walk.steps.push(Step::new(
            self.rank_to_node[walk.node],
            Orientation::from(link.rank_rev & 1 == 1),
        )?);
        if walk.steps.len() < walk.length {
            if link.next_node == StepLink::END {
                return Err(Self::broken(&walk.name, "ends before its length"));
            }
            walk.node = link.next_node as usize;
            walk.rank = (link.rank_rev >> 1) as usize;
        }
        Ok(())
    }

    /// Walks a batch of paths, [`WALK_LANES`] at a time.
    fn walk_batch(&self, metas: Vec<PathMeta>) -> PanResult<Vec<Path>> {
        let mut paths: Vec<Option<Path>> = (0..metas.len()).map(|_| None).collect();
        let mut pending = metas.into_iter().enumerate();
        let mut lanes: Vec<Walk> = Vec::with_capacity(WALK_LANES);
        loop {
            while lanes.len() < WALK_LANES
                && let Some((index, meta)) = pending.next()
            {
                lanes.push(self.start(index, meta)?);
            }
            if lanes.is_empty() {
                break;
            }
            for walk in &mut lanes {
                if walk.steps.len() < walk.length {
                    self.advance(walk)?;
                }
            }
            let mut lane = 0;
            while lane < lanes.len() {
                if lanes[lane].steps.len() == lanes[lane].length {
                    let walk = lanes.swap_remove(lane);
                    paths[walk.index] = Some(Path {
                        name: walk.name,
                        steps: walk.steps,
                        overlaps: Vec::new(),
                    });
                } else {
                    lane += 1;
                }
            }
        }
        Ok(paths.into_iter().flatten().collect())
    }

    /// Walks all paths, in parallel batches.
    fn walk_all(&self, metas: Vec<PathMeta>) -> PanResult<Vec<Path>> {
        // Every step belongs to one path, so the paths' reservations together
        // are bounded by the stored steps.
        let total = metas.iter().try_fold(0_usize, |total, meta| {
            usize::try_from(meta.length)
                .ok()
                .and_then(|length| total.checked_add(length))
        });
        if total.is_none_or(|total| total > self.links.len()) {
            return Err(PanGraphXError::Parse(
                "ODGI paths are longer than the number of stored steps".to_string(),
            ));
        }
        // At most one batch of lanes per task, so that threads can steal work
        // when path lengths differ.
        let batch = metas
            .len()
            .div_ceil(rayon::current_num_threads())
            .clamp(1, WALK_LANES);
        let mut batches = Vec::new();
        let mut metas = metas.into_iter().peekable();
        while metas.peek().is_some() {
            batches.push(metas.by_ref().take(batch).collect::<Vec<_>>());
        }
        let walked = batches
            .into_par_iter()
            .map(|batch| self.walk_batch(batch))
            .collect::<PanResult<Vec<_>>>()?;
        Ok(walked.into_iter().flatten().collect())
    }
}

/// Bytes of node records read and decoded at a time.
const BLOCK_BYTES: usize = 16 << 20;

/// Header fields needed to decode node records.
#[derive(Clone, Copy)]
struct RecordContext {
    record_count: usize,
    id_increment: u64,
}

impl RecordContext {
    /// Maps an odgi node id to its node rank, which fits in `u32` below
    /// [`StepLink::END`] (checked in `read_graph`).
    #[allow(clippy::cast_possible_truncation)]
    fn rank(self, id: u64) -> PanResult<u32> {
        id.checked_sub(self.id_increment)
            .and_then(|id| id.checked_sub(1))
            .filter(|&rank| rank < self.record_count as u64)
            .map(|rank| rank as u32)
            .ok_or_else(|| PanGraphXError::Parse(format!("ODGI node id {id} is out of range")))
    }
}

/// Decoded node records of one parallel task, in record order. Their steps
/// are written straight into the task's part of the final link table.
#[derive(Default)]
struct DecodedRecords {
    /// Sequence of every record, `None` for a deleted node.
    sequences: Vec<Option<Vec<u8>>>,
    /// Edges with node ranks in `from_node` and `to_node`.
    edges: Vec<Edge>,
}

/// Per-task buffers for unpacked vectors.
#[derive(Default)]
struct RecordScratch {
    edges: Vec<u64>,
    decoding: Vec<u64>,
    steps: Vec<u64>,
}

impl DecodedRecords {
    /// Decodes the node record of rank `rank` held in `bytes`, writing its
    /// steps to the start of `links` (sized by [`scan_record`]).
    fn decode(
        &mut self,
        bytes: &[u8],
        rank: usize,
        context: RecordContext,
        scratch: &mut RecordScratch,
        links: &mut [StepLink],
    ) -> PanResult<()> {
        let mut fields = Fields { bytes };
        let sequence_len = fields.len("sequence length")?;
        let sequence = fields.take(sequence_len)?;
        let stored_id = fields.u64()?;
        fields.packed(&mut scratch.edges)?;
        fields.packed(&mut scratch.decoding)?;
        fields.packed(&mut scratch.steps)?;
        if stored_id == 0 {
            // Deleted nodes are stored as empty records.
            self.sequences.push(None);
            return Ok(());
        }
        self.sequences.push(Some(sequence.to_vec()));
        // Checked in `read_graph` not to overflow.
        let odgi_id = rank as u64 + 1 + context.id_increment;
        let invalid = |what: &str| PanGraphXError::Parse(format!("ODGI node {odgi_id} has {what}"));

        let edge_records = scratch.edges.chunks_exact(EDGE_RECORD_LEN);
        if !edge_records.remainder().is_empty() {
            return Err(invalid("an incomplete edge record"));
        }
        for record in edge_records {
            let (other, packed) = (record[0], record[1]);
            // The edge is read from its left node; `to_curr` marks the right node's copy.
            if packed & EDGE_TO_CURR != 0 {
                continue;
            }
            self.edges.push(Edge {
                from_node: rank,
                from_orient: Orientation::from(packed & EDGE_ON_REV != 0),
                to_node: context.rank(other)? as usize,
                to_orient: Orientation::from(packed & EDGE_OTHER_REV != 0),
                overlap: 0,
            });
        }

        let step_records = scratch.steps.chunks_exact(STEP_RECORD_LEN);
        if !step_records.remainder().is_empty() || step_records.len() != links.len() {
            return Err(invalid("an incomplete step record"));
        }
        for (record, link) in step_records.zip(links) {
            let step_type = record[1];
            let is_reverse = u32::from(step_type & STEP_REV != 0);
            if step_type & STEP_END != 0 {
                *link = StepLink {
                    next_node: StepLink::END,
                    rank_rev: is_reverse,
                };
                continue;
            }
            let delta = usize::try_from(record[4])
                .ok()
                .and_then(|index| scratch.decoding.get(index))
                .ok_or_else(|| invalid("a step with an invalid neighbor"))?;
            let next_id = match delta {
                0 => Some(stored_id),
                d if d & 1 == 1 => stored_id.checked_add(d >> 1),
                d => stored_id.checked_sub(d >> 1),
            }
            .ok_or_else(|| invalid("an invalid neighbor"))?;
            let next_rank = u32::try_from(record[5])
                .ok()
                .filter(|&rank| rank <= MAX_STEP_RANK)
                .ok_or_else(|| invalid("an invalid step rank"))?;
            *link = StepLink {
                next_node: context.rank(next_id)?,
                rank_rev: next_rank << 1 | is_reverse,
            };
        }
        Ok(())
    }
}

/// Node records decoded so far, merged in rank order.
struct NodeSection {
    nodes: Nodes,
    edges: Vec<Edge>,
    offsets: Vec<usize>,
    links: Vec<StepLink>,
    rank_to_node: Vec<NodeId>,
}

/// Splits `links` into consecutive parts holding the steps of each chunk of spans.
fn split_links<'a>(
    mut links: &'a mut [StepLink],
    chunks: std::slice::Chunks<'_, RecordSpan>,
) -> PanResult<Vec<&'a mut [StepLink]>> {
    let mut parts = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let steps = chunk.iter().map(|span| span.steps).sum();
        let (part, rest) = links
            .split_at_mut_checked(steps)
            .ok_or_else(truncated_file)?;
        parts.push(part);
        links = rest;
    }
    Ok(parts)
}

/// Tops `buf` up to a block, or to the size of the record it holds part of,
/// and returns the spans of the complete records in it (at most `records_left`).
fn next_block<R: Read>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    input_left: &mut u64,
    records_left: usize,
) -> PanResult<Vec<RecordSpan>> {
    let mut want = BLOCK_BYTES.saturating_sub(buf.len()) as u64;
    let mut spans: Vec<RecordSpan> = Vec::new();
    loop {
        if want > 0 {
            let request = want.min(*input_left);
            let got = reader.by_ref().take(request).read_to_end(buf)? as u64;
            // A short read is the end of the input, even if it came early.
            *input_left = if got < request { 0 } else { *input_left - got };
            want = 0;
        }
        let mut pos = 0;
        while spans.len() < records_left {
            match scan_record(buf, pos) {
                Ok(span) => {
                    pos = span.end;
                    spans.push(span);
                }
                Err(needed) => {
                    if spans.is_empty() {
                        let missing = needed.saturating_sub(buf.len() as u64);
                        if missing > *input_left {
                            return Err(truncated_file());
                        }
                        want = missing;
                    }
                    break;
                }
            }
        }
        if !spans.is_empty() {
            return Ok(spans);
        }
    }
}

impl NodeSection {
    /// Appends the decoded records of a block, whose spans are `spans`.
    fn merge(&mut self, decoded: DecodedRecords, spans: &[RecordSpan]) {
        for (sequence, span) in decoded.sequences.into_iter().zip(spans) {
            match sequence {
                Some(sequence) => {
                    self.rank_to_node.push(self.nodes.len());
                    self.nodes.push(sequence);
                }
                None => self.rank_to_node.push(VACANT),
            }
            let end = self.offsets.last().copied().unwrap_or_default() + span.steps;
            self.offsets.push(end);
        }
        self.edges.extend(decoded.edges);
    }

    /// Decodes the records `spans` of `buf` in parallel tasks, the first of
    /// rank `first_rank`, and appends them.
    fn decode_block(
        &mut self,
        buf: &[u8],
        spans: &[RecordSpan],
        first_rank: usize,
        context: RecordContext,
    ) -> PanResult<()> {
        // Every step needs at least one bit of input, so this is bounded too.
        let block_steps = spans
            .iter()
            .try_fold(0_usize, |sum, span| sum.checked_add(span.steps))
            .filter(|&steps| steps as u64 <= buf.len() as u64 * 8)
            .ok_or_else(truncated_file)?;
        let base = self.links.len();
        self.links.resize(
            base + block_steps,
            StepLink {
                next_node: StepLink::END,
                rank_rev: 0,
            },
        );
        let per_task = spans.len().div_ceil(rayon::current_num_threads() * 4);
        let parts = split_links(&mut self.links[base..], spans.chunks(per_task))?;
        let decoded = spans
            .par_chunks(per_task)
            .zip(parts)
            .enumerate()
            .map(|(task, (chunk, mut links))| {
                let mut decoded = DecodedRecords::default();
                let mut scratch = RecordScratch::default();
                for (i, span) in chunk.iter().enumerate() {
                    let (record_links, rest) = links
                        .split_at_mut_checked(span.steps)
                        .ok_or_else(truncated_file)?;
                    links = rest;
                    decoded.decode(
                        &buf[span.start..span.end],
                        first_rank + task * per_task + i,
                        context,
                        &mut scratch,
                        record_links,
                    )?;
                }
                Ok(decoded)
            })
            .collect::<PanResult<Vec<_>>>()?;
        for (decoded, chunk) in decoded.into_iter().zip(spans.chunks(per_task)) {
            self.merge(decoded, chunk);
        }
        Ok(())
    }

    /// Replaces the node ranks in the edges by node ids.
    fn resolve_edges(&mut self) -> PanResult<()> {
        for edge in &mut self.edges {
            let (Some(&from), Some(&to)) = (
                self.rank_to_node.get(edge.from_node),
                self.rank_to_node.get(edge.to_node),
            ) else {
                return Err(PanGraphXError::Parse(
                    "ODGI edge refers to a missing node".to_string(),
                ));
            };
            if to == VACANT {
                return Err(PanGraphXError::Parse(
                    "ODGI edge refers to a deleted node".to_string(),
                ));
            }
            (edge.from_node, edge.to_node) = (from, to);
        }
        Ok(())
    }
}

/// Builds the node name map. Inserting is a chain of cache misses, so it is
/// done block by block alongside the decoding of the next block.
struct NodeNamer {
    names: HashMap<NodeId, Vec<u8>>,
    /// odgi id of the last rank added.
    last_id: u64,
}

impl NodeNamer {
    /// Names the nodes of the next ranks, given their `rank_to_node` entries.
    fn add(&mut self, ranks: &[NodeId]) {
        let mut id_buf = itoa::Buffer::new();
        for &node in ranks {
            // Checked in `read_graph` not to overflow.
            self.last_id += 1;
            if node != VACANT {
                self.names
                    .insert(node, id_buf.format(self.last_id).as_bytes().to_vec());
            }
        }
    }
}

/// Reads the node records in blocks: record boundaries are found sequentially
/// from their length fields, then the records of a block are decoded in
/// parallel while `namer` names the nodes of the previous block. Returns the
/// records, the input left after them and the ranks still to be named.
fn read_node_section<R: Read>(
    reader: &mut R,
    context: RecordContext,
    mut input_left: u64,
    namer: &mut NodeNamer,
) -> PanResult<(NodeSection, Vec<u8>, Vec<NodeId>)> {
    let count = context.record_count;
    // `record_count` is bounded by the input size, so these reservations are too.
    let mut section = NodeSection {
        nodes: Nodes::new(),
        edges: Vec::new(),
        offsets: Vec::with_capacity(count + 1),
        links: Vec::new(),
        rank_to_node: Vec::with_capacity(count),
    };
    section.offsets.push(0);
    let mut buf: Vec<u8> = Vec::new();
    let mut unnamed = Vec::new();
    let mut rank = 0;
    while rank < count {
        let spans = next_block(reader, &mut buf, &mut input_left, count - rank)?;
        let (decoded, ()) = rayon::join(
            || section.decode_block(&buf, &spans, rank, context),
            || namer.add(&unnamed),
        );
        decoded?;
        unnamed = section.rank_to_node[rank..].to_vec();
        rank += spans.len();
        let consumed = spans.last().map_or(0, |span| span.end);
        buf.drain(..consumed);
    }
    section.resolve_edges()?;
    Ok((section, buf, unnamed))
}

/// Reads the graph after the magic number; `input_len` is the number of bytes
/// left in the input, which bounds the node count and the reads.
fn read_graph<R: Read>(reader: &mut R, input_len: u64) -> PanResult<CoreGraphDTO> {
    let _max_node_id = read_u64(reader)?;
    let _min_node_id = read_u64(reader)?;
    let record_count = to_usize(read_u64(reader)?, "node count")?;
    let _edge_count = read_u64(reader)?;
    let path_count = to_usize(read_u64(reader)?, "path count")?;
    let _path_handle_next = read_u64(reader)?;
    let id_increment = read_u64(reader)?;
    let input_left = input_len.saturating_sub(7 * 8);
    // Node ranks must fit in `u32` below `StepLink::END`.
    if record_count >= StepLink::END as usize || record_count as u64 > input_left / MIN_RECORD_BYTES
    {
        return Err(PanGraphXError::Parse(format!(
            "ODGI node count {record_count} is too large"
        )));
    }
    if id_increment.checked_add(record_count as u64 + 1).is_none() {
        return Err(PanGraphXError::Parse(format!(
            "ODGI id increment {id_increment} is too large"
        )));
    }
    let context = RecordContext {
        record_count,
        id_increment,
    };
    let mut namer = NodeNamer {
        // `record_count` is bounded by the input size.
        names: HashMap::with_capacity(record_count),
        last_id: id_increment,
    };
    let (section, rest, unnamed) = read_node_section(reader, context, input_left, &mut namer)?;

    let mut reader = rest.as_slice().chain(reader);
    let mut metas = Vec::with_capacity(path_count.min(MAX_PREALLOC));
    for _ in 0..path_count {
        let length = read_u64(&mut reader)?;
        let first_handle = read_u64(&mut reader)?;
        let first_rank = read_u64(&mut reader)?;
        let _last_handle = read_u64(&mut reader)?;
        let _last_rank = read_u64(&mut reader)?;
        let name_len = read_u64(&mut reader)?;
        let name = read_bytes(&mut reader, name_len)?;
        metas.push(PathMeta {
            name,
            length,
            first_handle,
            first_rank,
        });
    }

    let NodeSection {
        nodes,
        edges,
        offsets,
        links,
        rank_to_node,
    } = section;
    let table = StepTable {
        offsets,
        links,
        rank_to_node,
    };
    let ((), paths) = rayon::join(|| namer.add(&unnamed), || table.walk_all(metas));
    Ok(CoreGraphDTO {
        nodes,
        edges,
        paths: paths?,
        node_name_map: Some(namer.names),
    })
}

impl<R: Read + Seek> GraphParser<R> for ODGICodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        let mut reader = BufReader::with_capacity(1 << 20, reader);
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic).map_err(truncated)?;
        if u32::from_be_bytes(magic) != ODGI_MAGIC {
            // odgi accepts files without the magic number (older versions), so do we.
            warn!("ODGI magic number not found; reading the file anyway");
            reader.seek_relative(-4)?;
        }
        let position = reader.stream_position()?;
        let input_len = reader.seek(SeekFrom::End(0))?.saturating_sub(position);
        reader.seek(SeekFrom::Start(position))?;
        read_graph(&mut reader, input_len)
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

fn push_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Appends `values` as a `hacked_vector` of the smallest width that fits them.
#[allow(clippy::cast_possible_truncation)]
fn push_packed(buf: &mut Vec<u8>, values: &[u64]) {
    let Some(&max) = values.iter().max() else {
        // A default-constructed (empty) vector.
        push_u64(buf, 0);
        push_u64(buf, 0);
        push_u64(buf, 0);
        buf.extend_from_slice(&[0, 0]);
        return;
    };
    let width = (64 - max.leading_zeros()).max(1) as usize;
    let per_word = 64 / width;
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1 << width) - 1
    };
    push_u64(buf, values.len().div_ceil(per_word) as u64);
    for chunk in values.chunks(per_word) {
        let word = chunk
            .iter()
            .enumerate()
            .fold(0, |word, (k, &value)| word | value << (k * width));
        push_u64(buf, word);
    }
    push_u64(buf, mask);
    push_u64(buf, values.len() as u64);
    buf.push(width as u8);
    buf.push(per_word as u8);
}

/// Assignment of odgi node ranks (`odgi id - 1`) to nodes.
struct Numbering {
    /// odgi id of every node, `None` for `1..=n` in node order.
    ids: Option<Vec<u64>>,
    /// Node at every rank (`VACANT` for gaps), `None` for `1..=n` in node order.
    by_rank: Option<Vec<NodeId>>,
    /// Number of node records, including gaps.
    records: usize,
}

impl Numbering {
    /// Keeps numeric node names as odgi ids when they are distinct, positive,
    /// written canonically and dense enough that the gaps stay small.
    fn new(graph: &CoreGraphDTO) -> Self {
        let n = graph.nodes.len();
        let sequential = Self {
            ids: None,
            by_rank: None,
            records: n,
        };
        let Some(map) = &graph.node_name_map else {
            return sequential;
        };
        let ids: Option<Vec<u64>> = (0..n)
            .into_par_iter()
            .map(|node| {
                let name = map.get(&node)?;
                let id = std::str::from_utf8(name).ok()?.parse::<u64>().ok()?;
                (id > 0 && itoa::Buffer::new().format(id).as_bytes() == name.as_slice())
                    .then_some(id)
            })
            .collect();
        let named = ids.and_then(|ids| {
            let max = usize::try_from(*ids.iter().max()?).ok()?;
            if max > n.saturating_mul(2).saturating_add(1024) {
                return None;
            }
            let mut by_rank = vec![VACANT; max];
            for (node, &id) in ids.iter().enumerate() {
                let slot = &mut by_rank[usize::try_from(id - 1).ok()?];
                if *slot != VACANT {
                    return None;
                }
                *slot = node;
            }
            Some(Self {
                ids: Some(ids),
                by_rank: Some(by_rank),
                records: max,
            })
        });
        named.unwrap_or_else(|| {
            if map.values().any(|name| !name.is_empty()) {
                warn!(
                    "ODGI needs positive integer node ids; node names are replaced by ids 1..={n}"
                );
            }
            sequential
        })
    }

    fn id(&self, node: NodeId) -> u64 {
        self.ids.as_ref().map_or(node as u64 + 1, |ids| ids[node])
    }

    fn node_at(&self, rank: usize) -> Option<NodeId> {
        self.by_rank.as_ref().map_or(Some(rank), |by_rank| {
            Some(by_rank[rank]).filter(|&node| node != VACANT)
        })
    }
}

/// Bits of the packed edge type in an [`EdgeIndex`] record.
const EDGE_TYPE_BITS: u32 = 3;

/// Nodes per parallel task when de-duplicating edge records.
const EDGE_DEDUP_NODES: usize = 1 << 12;

/// Up to this many records, a node's duplicates are found by linear scan.
const SMALL_NODE_DEGREE: usize = 16;

/// Edge records of every node, as odgi stores them on both endpoints.
struct EdgeIndex {
    /// Records of node `v` are `records[offsets[v]..offsets[v + 1]]`.
    offsets: Vec<usize>,
    /// `other node << EDGE_TYPE_BITS | packed edge type`.
    records: Vec<u64>,
    /// Number of distinct edges.
    edge_count: usize,
}

/// The edge a record of `node` stands for, as a pair of handles, identical for
/// an edge and its reverse complement.
const fn canonical_edge(node: NodeId, record: u64) -> (u64, u64) {
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
fn dedup_node_records(
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
    fn new(node_count: usize, edges: &[Edge]) -> Self {
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

/// Index of a step in all paths concatenated, as stored in [`StepIndex`]:
/// `u32` when there are fewer than 2^32 steps, which halves the index.
trait StepNumber: Copy + Send + Sync + Default {
    /// Converts `index`, which the caller checked to fit.
    fn from_index(index: usize) -> Self;
    fn index(self) -> usize;
}

impl StepNumber for u32 {
    #[allow(clippy::cast_possible_truncation)]
    fn from_index(index: usize) -> Self {
        index as Self
    }
    fn index(self) -> usize {
        self as usize
    }
}

impl StepNumber for u64 {
    fn from_index(index: usize) -> Self {
        index as Self
    }
    // Step indices come from `usize` values.
    #[allow(clippy::cast_possible_truncation)]
    fn index(self) -> usize {
        self as usize
    }
}

/// Most node ranges the step index is built in. Every range's task scans all
/// steps, so more ranges cost memory bandwidth rather than saving time.
const MAX_STEP_RANGES: usize = 8;

/// Path steps grouped by the node they visit.
struct StepIndex<I> {
    /// Steps on node `v` are `slots[offsets[v]..offsets[v + 1]]`, ordered by
    /// their rank on that node (which is also their order in all paths).
    offsets: Vec<usize>,
    /// Global index (all paths concatenated) of the step in each slot.
    slots: Vec<I>,
    /// Global index of the first step of every path, plus the total.
    path_starts: Vec<usize>,
}

impl<I: StepNumber> StepIndex<I> {
    /// Counts and places the steps of every node range in parallel; each range's
    /// task scans all steps in order, so the result is deterministic.
    fn new(node_count: usize, paths: &[Path]) -> PanResult<Self> {
        let mut path_starts = Vec::with_capacity(paths.len() + 1);
        path_starts.push(0);
        for path in paths {
            path_starts.push(path_starts[path_starts.len() - 1] + path.steps.len());
        }
        let all_steps = || paths.iter().flat_map(|path| &path.steps).enumerate();
        let parts = rayon::current_num_threads().clamp(1, MAX_STEP_RANGES);

        // Step counts, in node ranges of equal size.
        let mut offsets = vec![0; node_count + 1];
        let bounds: Vec<usize> = (0..=parts).map(|part| node_count * part / parts).collect();
        let mut ranges = Vec::with_capacity(parts);
        let mut rest = &mut offsets[1..];
        for range in bounds.windows(2) {
            let (part, tail) = rest.split_at_mut(range[1] - range[0]);
            ranges.push((range[0], part));
            rest = tail;
        }
        ranges.into_par_iter().for_each(|(first, counts)| {
            for (_, step) in all_steps() {
                if let Some(count) = step
                    .node_id()
                    .checked_sub(first)
                    .and_then(|i| counts.get_mut(i))
                {
                    *count += 1;
                }
            }
        });
        if let Some(node) = offsets
            .iter()
            .position(|&count| count > MAX_STEP_RANK as usize + 1)
        {
            return Err(PanGraphXError::Serialize(format!(
                "Node {} is visited too often for ODGI",
                node - 1
            )));
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }

        // Slots, in node ranges holding equal numbers of steps.
        let mut slots = vec![I::default(); offsets[node_count]];
        let total = offsets[node_count];
        let mut bounds: Vec<usize> = (0..=parts)
            .map(|part| offsets.partition_point(|&offset| offset < total / parts * part))
            .map(|node| node.min(node_count))
            .collect();
        bounds[parts] = node_count;
        let mut ranges = Vec::with_capacity(parts);
        let mut rest = slots.as_mut_slice();
        for range in bounds.windows(2) {
            let (part, tail) = rest.split_at_mut(offsets[range[1]] - offsets[range[0]]);
            ranges.push((range[0], range[1], part));
            rest = tail;
        }
        ranges.into_par_iter().for_each(|(first, last, slots)| {
            let base = offsets[first];
            let mut cursor: Vec<usize> = offsets[first..last].iter().map(|&o| o - base).collect();
            for (global, step) in all_steps() {
                if let Some(next) = step
                    .node_id()
                    .checked_sub(first)
                    .and_then(|i| cursor.get_mut(i))
                {
                    slots[*next] = I::from_index(global);
                    *next += 1;
                }
            }
        });
        Ok(Self {
            offsets,
            slots,
            path_starts,
        })
    }

    /// Rank of global step `global` among the steps on its node `node`.
    fn rank(&self, node: NodeId, global: usize) -> u64 {
        let slots = &self.slots[self.offsets[node]..self.offsets[node + 1]];
        slots.partition_point(|&slot| slot.index() < global) as u64
    }

    /// Like [`StepIndex::rank`], for a caller that looks up increasing steps of
    /// each node: `hints` remembers where the last lookup on a node ended.
    fn rank_after(&self, hints: &mut Vec<(NodeId, usize)>, node: NodeId, global: usize) -> u64 {
        let slots = &self.slots[self.offsets[node]..self.offsets[node + 1]];
        let hint = hints.iter().position(|&(hinted, _)| hinted == node);
        let from = hint
            .map(|i| hints[i].1)
            .filter(|&from| {
                from == 0
                    || slots
                        .get(from - 1)
                        .is_some_and(|slot| slot.index() < global)
            })
            .unwrap_or(0);
        let rank = gallop(slots.len(), from, |i| slots[i].index() < global);
        match hint {
            Some(i) => hints[i].1 = rank + 1,
            None if hints.len() < MAX_RANK_HINTS => hints.push((node, rank + 1)),
            None => {}
        }
        rank as u64
    }

    /// Path index and position within that path of a global step, for
    /// increasing steps: `path_end` is the index after the previous step's path.
    fn locate_after(&self, path_end: &mut usize, global: usize) -> (usize, usize) {
        // Empty paths share their start with the next path; the last path
        // starting at or before `global` is the one containing it.
        let starts = &self.path_starts;
        *path_end = gallop(starts.len(), *path_end, |i| starts[i] <= global);
        let path = *path_end - 1;
        (path, global - starts[path])
    }
}

/// Neighbor nodes per node record whose last rank lookup is remembered.
const MAX_RANK_HINTS: usize = 16;

/// First index in `from..len` where `is_before` is false, for a predicate that
/// holds on a prefix including `..from`. Searches exponentially from `from`,
/// so it is fast when the answer is close.
fn gallop(len: usize, from: usize, is_before: impl Fn(usize) -> bool) -> usize {
    let mut low = from;
    let mut bound = 1_usize;
    loop {
        let probe = low.saturating_add(bound - 1);
        if probe >= len || !is_before(probe) {
            let mut high = probe.min(len);
            while low < high {
                let mid = low + (high - low) / 2;
                if is_before(mid) {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            return low;
        }
        low = probe + 1;
        bound = bound.saturating_mul(2);
    }
}

/// Up to this many entries, the decoding table is searched linearly.
const SMALL_DECODING: usize = 16;

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
fn write_odgi<I: StepNumber>(graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
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

impl GraphSerializer for ODGICodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn edge(from: NodeId, from_orient: Orientation, to: NodeId, to_orient: Orientation) -> Edge {
        Edge {
            from_node: from,
            from_orient,
            to_node: to,
            to_orient,
            overlap: 0,
        }
    }

    fn path(name: &str, steps: &[(NodeId, Orientation)]) -> Path {
        Path {
            name: name.as_bytes().to_vec(),
            steps: steps
                .iter()
                .map(|&(node, orientation)| Step::new(node, orientation).unwrap())
                .collect(),
            overlaps: Vec::new(),
        }
    }

    fn roundtrip(graph: &CoreGraphDTO) -> CoreGraphDTO {
        let mut buf = Vec::new();
        ODGICodec.serialize(graph, &mut buf).unwrap();
        ODGICodec.parse(&mut Cursor::new(buf)).unwrap()
    }

    /// Names nodes by their 1-based id, as they come back from odgi.
    fn one_based(mut graph: CoreGraphDTO) -> CoreGraphDTO {
        graph.node_name_map = Some(
            (0..graph.nodes.len())
                .map(|id| (id, (id + 1).to_string().into_bytes()))
                .collect(),
        );
        graph
    }

    use Orientation::{Forward, Reverse};

    #[test]
    fn roundtrip_keeps_orientations() {
        let original = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse)],
            paths: vec![path("p", &[(0, Forward), (1, Reverse)])],
            node_name_map: None,
        };
        let parsed = roundtrip(&original);
        assert_eq!(parsed.get_name_from_id(0), "1");
        assert_eq!(parsed.get_name_from_id(1), "2");
        assert_eq!(parsed, one_based(original));
    }

    #[test]
    fn roundtrip_self_loops_revisits_and_empty_paths() {
        let original = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"CC".to_vec(), b"GGG".to_vec()]),
            edges: vec![
                edge(0, Forward, 0, Forward),
                edge(1, Forward, 1, Reverse),
                edge(2, Reverse, 0, Forward),
                edge(0, Forward, 1, Forward),
            ],
            paths: vec![
                path(
                    "a",
                    &[(0, Forward), (1, Forward), (0, Forward), (0, Reverse)],
                ),
                path("empty", &[]),
                path(
                    "b",
                    &[(2, Reverse), (0, Forward), (1, Forward), (1, Reverse)],
                ),
            ],
            node_name_map: None,
        });
        // Edges come back grouped by node, so compare them as a set.
        let parsed = roundtrip(&original);
        assert!(parsed.isomorphic(&original));
        assert_eq!(parsed.paths, original.paths);
    }

    #[test]
    fn reverse_complement_edges_are_merged() {
        let graph = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec()]),
            edges: vec![
                edge(0, Forward, 1, Forward),
                edge(1, Reverse, 0, Reverse),
                edge(0, Forward, 1, Forward),
            ],
            paths: Vec::new(),
            node_name_map: None,
        };
        assert_eq!(roundtrip(&graph).edges, vec![edge(0, Forward, 1, Forward)]);
    }

    #[test]
    fn numeric_names_are_kept_with_gaps() {
        let names = ["7", "3", "1"];
        let original = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec(), b"G".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse), edge(2, Forward, 0, Forward)],
            paths: vec![path("p", &[(2, Forward), (0, Forward), (1, Reverse)])],
            node_name_map: Some(
                names
                    .iter()
                    .enumerate()
                    .map(|(id, name)| (id, name.as_bytes().to_vec()))
                    .collect(),
            ),
        };
        let parsed = roundtrip(&original);
        // Nodes come back in id order; ids 2 and 4..=6 are deleted records.
        assert_eq!(parsed.nodes.len(), 3);
        assert!(parsed.isomorphic(&original));
    }

    #[test]
    fn non_numeric_names_are_replaced() {
        let original = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec()]),
            edges: vec![edge(0, Forward, 1, Forward)],
            paths: Vec::new(),
            node_name_map: Some(HashMap::from([(0, b"x".to_vec()), (1, b"01".to_vec())])),
        };
        let parsed = roundtrip(&original);
        assert_eq!(parsed.get_name_from_id(0), "1");
        assert_eq!(parsed.get_name_from_id(1), "2");
    }

    #[test]
    fn records_spanning_blocks() {
        // Records larger than a block and records cut by a block boundary.
        let big =
            |base: u8, len: usize| (0..len).map(|i| b"ACGT"[(i + base as usize) % 4]).collect();
        let original = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![
                b"A".to_vec(),
                big(0, BLOCK_BYTES + 100),
                big(1, BLOCK_BYTES / 3),
                big(2, BLOCK_BYTES / 3),
                big(3, BLOCK_BYTES / 2),
                b"C".to_vec(),
            ]),
            edges: (0..5).map(|i| edge(i, Forward, i + 1, Forward)).collect(),
            paths: vec![
                path("p", &(0..6).map(|i| (i, Forward)).collect::<Vec<_>>()),
                path("q", &[(5, Reverse), (4, Reverse), (3, Reverse)]),
            ],
            node_name_map: None,
        });
        assert_eq!(roundtrip(&original), original);
    }

    #[test]
    fn high_degree_nodes() {
        // Node 0 has more edges, decoding entries and path neighbors than the
        // small linear-scan limits, with duplicate and reverse-complement edges.
        let n = 3 * SMALL_NODE_DEGREE.max(SMALL_DECODING).max(MAX_RANK_HINTS);
        let mut edges = Vec::new();
        let mut paths = Vec::new();
        for i in 1..n {
            edges.push(edge(0, Forward, i, Forward));
            edges.push(edge(i, Reverse, 0, Reverse));
            edges.push(edge(i, Forward, 0, Forward));
            paths.push(path(
                &format!("p{i}"),
                &[(i, Forward), (0, Forward), (n - i, Forward)],
            ));
        }
        let original = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq((0..n).map(|i| vec![b"ACGT"[i % 4]]).collect()),
            edges,
            paths,
            node_name_map: None,
        });
        let parsed = roundtrip(&original);
        assert_eq!(parsed.edges.len(), 2 * (n - 1));
        assert_eq!(parsed.paths, original.paths);
    }

    #[test]
    fn step_number_widths_agree() {
        let graph = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"CC".to_vec(), b"GGG".to_vec()]),
            edges: vec![edge(0, Forward, 1, Forward), edge(1, Forward, 2, Reverse)],
            paths: vec![
                path(
                    "a",
                    &[(0, Forward), (1, Forward), (2, Reverse), (1, Reverse)],
                ),
                path("b", &[(1, Forward), (2, Reverse)]),
            ],
            node_name_map: None,
        });
        let (mut narrow, mut wide) = (Vec::new(), Vec::new());
        write_odgi::<u32>(&graph, &mut narrow).unwrap();
        write_odgi::<u64>(&graph, &mut wide).unwrap();
        assert_eq!(narrow, wide);
    }

    /// A source whose end lies `extra` bytes beyond its data.
    struct OverstatedLen {
        inner: Cursor<Vec<u8>>,
        extra: u64,
    }

    impl Read for OverstatedLen {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.inner.read(buf)
        }
    }

    impl Seek for OverstatedLen {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            match pos {
                SeekFrom::End(offset) => {
                    let end = self.inner.get_ref().len() as u64 + self.extra;
                    let position = end.checked_add_signed(offset).unwrap();
                    self.inner.set_position(position);
                    Ok(position)
                }
                pos => self.inner.seek(pos),
            }
        }
    }

    fn two_path_graph() -> CoreGraphDTO {
        one_based(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse)],
            paths: vec![
                path("a", &[(0, Forward), (1, Reverse)]),
                path("b", &[(1, Forward), (0, Reverse)]),
            ],
            node_name_map: None,
        })
    }

    #[test]
    fn truncated_input_with_overstated_length_is_rejected() {
        let mut buf = Vec::new();
        ODGICodec.serialize(&two_path_graph(), &mut buf).unwrap();
        buf.truncate(buf.len() / 2);
        let mut reader = OverstatedLen {
            inner: Cursor::new(buf),
            extra: 1 << 20,
        };
        assert!(ODGICodec.parse(&mut reader).is_err());
    }

    #[test]
    fn step_count_is_bounded_by_stored_words() {
        // A live record whose step vector claims 6 * 2^27 values in 0 words.
        let mut record = Vec::new();
        push_u64(&mut record, 1);
        record.push(b'A');
        push_u64(&mut record, 1);
        for _ in 0..2 {
            push_packed(&mut record, &[]);
        }
        for value in [0, 0, 6 << 27] {
            push_u64(&mut record, value);
        }
        record.extend_from_slice(&[1, 64]);
        let span = scan_record(&record, 0).unwrap();
        assert_eq!((span.end, span.steps), (record.len(), 0));
        assert!(
            Fields {
                bytes: &record[9 + 8 + 2 * 26..]
            }
            .packed(&mut Vec::new())
            .is_err()
        );
    }

    #[test]
    fn paths_longer_than_the_stored_steps_are_rejected() {
        let mut buf = Vec::new();
        ODGICodec.serialize(&two_path_graph(), &mut buf).unwrap();
        // Each path record: length, first and last step (4 values), name length, name.
        let record = 8 * 6 + 1;
        for start in [buf.len() - 2 * record, buf.len() - record] {
            buf[start..start + 8].copy_from_slice(&4_u64.to_le_bytes());
        }
        let error = ODGICodec.parse(&mut Cursor::new(buf)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("longer than the number of stored steps")
        );
    }

    #[test]
    fn empty_graph() {
        let parsed = roundtrip(&CoreGraphDTO::default());
        assert_eq!(
            (parsed.nodes.len(), parsed.edges.len(), parsed.paths.len()),
            (0, 0, 0)
        );
    }

    #[test]
    fn packed_widths() {
        for values in [
            vec![0],
            vec![1, 0, 1],
            vec![u64::MAX, 3],
            (0..1000).collect(),
        ] {
            let mut buf = Vec::new();
            push_packed(&mut buf, &values);
            let mut read = Vec::new();
            Fields { bytes: &buf }.packed(&mut read).unwrap();
            assert_eq!(read, values);
        }
    }

    #[test]
    fn rejects_truncated_and_corrupt_input() {
        let graph = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse)],
            paths: vec![path("p", &[(0, Forward), (1, Reverse)])],
            node_name_map: None,
        });
        let mut buf = Vec::new();
        ODGICodec.serialize(&graph, &mut buf).unwrap();
        for len in [0, 3, 20, buf.len() / 2, buf.len() - 1] {
            assert!(
                ODGICodec.parse(&mut Cursor::new(&buf[..len])).is_err(),
                "{len}"
            );
        }
        assert!(ODGICodec.parse(&mut Cursor::new(vec![0xFF; 256])).is_err());
    }
}
