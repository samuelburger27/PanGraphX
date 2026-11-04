//! The BWT of a GBZ decoded into flat arrays, for walking all of its paths.
//!
//! `GBWT::forward` finds the record in a sparse vector and decodes its edges
//! into a new vector on every step, which dominates the time to read a GBZ.
//! Here every record is decoded once. A record with a single successor needs
//! no runs: the sequence at offset `i` continues at `successor offset + i`.

use crate::error::{PanGraphXError, PanResult};
use gbz_construct::bwt::BWT;
use gbz_construct::support::{ByteCodeIter, RLEIter};
use gbz_construct::{ENDMARKER, GBWT};
use rayon::prelude::*;

/// Number of records decoded in one parallel task.
const RECORDS_PER_TASK: usize = 1 << 15;

/// An outgoing edge of a record: the successor and the offset of the first
/// visit from this record to it.
#[derive(Clone, Copy)]
pub(super) struct FlatEdge {
    pub(super) successor: u32,
    offset: u32,
}

/// A BWT run: the rank of the successor among the edges of the record, and the
/// length of the run.
#[derive(Clone, Copy)]
struct FlatRun {
    rank: u32,
    len: u32,
}

/// The records of a BWT. Record `i` has the edges
/// `edge_ends[i - 1]..edge_ends[i]` and the runs `run_ends[i - 1]..run_ends[i]`,
/// with 0 as the start of the first record.
pub(super) struct FlatBwt {
    alphabet_offset: usize,
    edge_ends: Vec<u32>,
    run_ends: Vec<u32>,
    edges: Vec<FlatEdge>,
    runs: Vec<FlatRun>,
}

/// The decoded records of one task; the ends are relative to the task.
#[derive(Default)]
struct DecodedRecords {
    edge_ends: Vec<u32>,
    run_ends: Vec<u32>,
    edges: Vec<FlatEdge>,
    runs: Vec<FlatRun>,
}

fn invalid_record(record: usize) -> PanGraphXError {
    PanGraphXError::Parse(format!("GBZ file has an invalid GBWT record {record}"))
}

fn too_large() -> PanGraphXError {
    PanGraphXError::Parse("GBZ file has a BWT too large to decode".to_string())
}

fn small(value: usize, record: usize) -> PanResult<u32> {
    u32::try_from(value).map_err(|_| invalid_record(record))
}

impl DecodedRecords {
    fn decode(bwt: &BWT, records: std::ops::Range<usize>) -> PanResult<Self> {
        let mut decoded = Self {
            edge_ends: Vec::with_capacity(records.len()),
            run_ends: Vec::with_capacity(records.len()),
            ..Self::default()
        };
        for record in records {
            if let Some((edges, runs)) = bwt.compressed_record(record) {
                decoded.push_record(record, edges, runs)?;
            }
            decoded
                .edge_ends
                .push(u32::try_from(decoded.edges.len()).map_err(|_| too_large())?);
            decoded
                .run_ends
                .push(u32::try_from(decoded.runs.len()).map_err(|_| too_large())?);
        }
        Ok(decoded)
    }

    fn push_record(&mut self, record: usize, edges: &[u8], runs: &[u8]) -> PanResult<()> {
        let mut ints = ByteCodeIter::new(edges);
        let sigma = ints.next().ok_or_else(|| invalid_record(record))?;
        let mut successor = 0usize;
        for _ in 0..sigma {
            let gap = ints.next().ok_or_else(|| invalid_record(record))?;
            let offset = ints.next().ok_or_else(|| invalid_record(record))?;
            successor = successor
                .checked_add(gap)
                .ok_or_else(|| invalid_record(record))?;
            self.edges.push(FlatEdge {
                successor: small(successor, record)?,
                offset: small(offset, record)?,
            });
        }
        if sigma > 1 {
            for run in RLEIter::with_sigma(runs, sigma) {
                if run.value >= sigma {
                    return Err(invalid_record(record));
                }
                self.runs.push(FlatRun {
                    rank: small(run.value, record)?,
                    len: small(run.len, record)?,
                });
            }
        }
        Ok(())
    }
}

impl FlatBwt {
    /// Decodes all records of `gbwt`.
    pub(super) fn new(gbwt: &GBWT) -> PanResult<Self> {
        let bwt: &BWT = gbwt.as_ref();
        let record_count = bwt.len();
        let tasks: Vec<DecodedRecords> = (0..record_count.div_ceil(RECORDS_PER_TASK))
            .into_par_iter()
            .map(|task| {
                let start = task * RECORDS_PER_TASK;
                DecodedRecords::decode(bwt, start..record_count.min(start + RECORDS_PER_TASK))
            })
            .collect::<PanResult<_>>()?;

        let mut flat = Self {
            alphabet_offset: gbwt.alphabet_offset(),
            edge_ends: Vec::with_capacity(record_count),
            run_ends: Vec::with_capacity(record_count),
            edges: Vec::with_capacity(tasks.iter().map(|task| task.edges.len()).sum()),
            runs: Vec::with_capacity(tasks.iter().map(|task| task.runs.len()).sum()),
        };
        for task in tasks {
            let edge_base = u32::try_from(flat.edges.len()).map_err(|_| too_large())?;
            let run_base = u32::try_from(flat.runs.len()).map_err(|_| too_large())?;
            for (edge_end, run_end) in task.edge_ends.iter().zip(&task.run_ends) {
                flat.edge_ends
                    .push(edge_base.checked_add(*edge_end).ok_or_else(too_large)?);
                flat.run_ends
                    .push(run_base.checked_add(*run_end).ok_or_else(too_large)?);
            }
            flat.edges.extend_from_slice(&task.edges);
            flat.runs.extend_from_slice(&task.runs);
        }
        Ok(flat)
    }

    /// Returns the record id of a GBWT node, and the ranges of its edges and runs.
    #[inline]
    fn record(
        &self,
        node: usize,
    ) -> PanResult<(usize, std::ops::Range<usize>, std::ops::Range<usize>)> {
        let record = node.wrapping_sub(self.alphabet_offset);
        let (Some(&edge_end), Some(&run_end)) =
            (self.edge_ends.get(record), self.run_ends.get(record))
        else {
            return Err(PanGraphXError::Parse(format!(
                "GBZ file refers to GBWT node {node}, which is not in the alphabet"
            )));
        };
        let previous = |ends: &[u32]| record.checked_sub(1).map_or(0, |i| ends[i] as usize);
        Ok((
            record,
            previous(&self.edge_ends)..edge_end as usize,
            previous(&self.run_ends)..run_end as usize,
        ))
    }

    /// Returns the outgoing edges of a GBWT node.
    pub(super) fn edges(&self, node: usize) -> PanResult<&[FlatEdge]> {
        let (_, edges, _) = self.record(node)?;
        Ok(&self.edges[edges])
    }

    /// Follows the sequence at `offset` of a GBWT node. Returns the next node
    /// and offset, or [`None`] where the sequence ends.
    #[inline]
    pub(super) fn lf(&self, node: usize, offset: usize) -> PanResult<Option<(usize, usize)>> {
        let (record, edges, runs) = self.record(node)?;
        let edges = &self.edges[edges];
        let (edge, position) = if let [edge] = edges {
            (*edge, offset)
        } else {
            let runs = &self.runs[runs];
            // Find the run containing `offset`, then count the earlier visits
            // to the same successor.
            let mut run_start = 0usize;
            let containing = runs.iter().position(|run| {
                let contains = offset - run_start < run.len as usize;
                if !contains {
                    run_start += run.len as usize;
                }
                contains
            });
            let containing = containing.ok_or_else(|| invalid_record(record))?;
            let rank = runs[containing].rank;
            let earlier: usize = runs[..containing]
                .iter()
                .filter(|run| run.rank == rank)
                .map(|run| run.len as usize)
                .sum();
            let edge = *edges
                .get(rank as usize)
                .ok_or_else(|| invalid_record(record))?;
            (edge, earlier + offset - run_start)
        };
        if edge.successor as usize == ENDMARKER {
            return Ok(None);
        }
        Ok(Some((
            edge.successor as usize,
            edge.offset as usize + position,
        )))
    }
}
