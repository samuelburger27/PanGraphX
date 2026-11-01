//! Node records: locating them in the input, then decoding blocks of them in parallel.

use crate::core::core_types::{Edge, NodeId, Nodes, Orientation};
use crate::error::{PanGraphXError, PanResult};
use crate::formats::odgi::encoding::{Fields, le_u64, truncated_file};
use crate::formats::odgi::reader::paths::StepLink;
use crate::formats::odgi::{
    EDGE_ON_REV, EDGE_OTHER_REV, EDGE_RECORD_LEN, EDGE_TO_CURR, MAX_STEP_RANK, STEP_END,
    STEP_RECORD_LEN, STEP_REV, VACANT,
};
use rayon::prelude::*;
use std::io::Read;

/// Smallest possible node record: sequence length, id and three empty packed
/// vectors (word count, mask, length, width, values per word).
pub(super) const MIN_RECORD_BYTES: u64 = 8 + 8 + 3 * (8 + 8 + 8 + 1 + 1);

/// Position and number of stored steps of a node record in a buffer.
pub(in crate::formats::odgi) struct RecordSpan {
    pub(in crate::formats::odgi) start: usize,
    pub(in crate::formats::odgi) end: usize,
    /// Stored steps, 0 for a deleted node (whose steps are ignored).
    pub(in crate::formats::odgi) steps: usize,
}

/// Locates the node record at `bytes[start..]`, or returns `Err(needed)` with a
/// lower bound on the buffer size if the buffer holds only part of it.
pub(in crate::formats::odgi) fn scan_record(bytes: &[u8], start: usize) -> Result<RecordSpan, u64> {
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

/// Bytes of node records read and decoded at a time.
pub(in crate::formats::odgi) const BLOCK_BYTES: usize = 16 << 20;

/// Header fields needed to decode node records.
#[derive(Clone, Copy)]
pub(super) struct RecordContext {
    pub(super) record_count: usize,
    pub(super) id_increment: u64,
}

impl RecordContext {
    /// Maps an odgi node id to its node rank, which fits in `u32` below
    /// [`StepLink::END`] (checked in `read_graph`).
    #[allow(clippy::cast_possible_truncation)]
    pub(super) fn rank(self, id: u64) -> PanResult<u32> {
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
pub(super) struct DecodedRecords {
    /// Sequence of every record, `None` for a deleted node.
    sequences: Vec<Option<Vec<u8>>>,
    /// Edges with node ranks in `from_node` and `to_node`.
    edges: Vec<Edge>,
}

/// Per-task buffers for unpacked vectors.
#[derive(Default)]
pub(super) struct RecordScratch {
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

        let (edge_records, edge_rest) = scratch.edges.as_chunks::<EDGE_RECORD_LEN>();
        if !edge_rest.is_empty() {
            return Err(invalid("an incomplete edge record"));
        }
        for &[other, packed] in edge_records {
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

        let (step_records, step_rest) = scratch.steps.as_chunks::<STEP_RECORD_LEN>();
        if !step_rest.is_empty() || step_records.len() != links.len() {
            return Err(invalid("an incomplete step record"));
        }
        for (record, link) in step_records.iter().zip(links) {
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
pub(super) struct NodeSection {
    pub(super) nodes: Nodes,
    pub(super) edges: Vec<Edge>,
    pub(super) offsets: Vec<usize>,
    pub(super) links: Vec<StepLink>,
    pub(super) rank_to_node: Vec<NodeId>,
}

/// Splits `links` into consecutive parts holding the steps of each chunk of spans.
pub(super) fn split_links<'a>(
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
pub(super) fn next_block<R: Read>(
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
    pub(super) fn decode_block(
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
    pub(super) fn resolve_edges(&mut self) -> PanResult<()> {
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
