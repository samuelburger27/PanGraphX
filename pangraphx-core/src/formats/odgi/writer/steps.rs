//! Path steps grouped by the node they visit, with each step's rank on its node.

use crate::core::core_types::{NodeId, Path};
use crate::error::{PanGraphXError, PanResult};
use crate::formats::odgi::MAX_STEP_RANK;
use rayon::prelude::*;

/// Index of a step in all paths concatenated, as stored in [`StepIndex`]:
/// `u32` when there are fewer than 2^32 steps, which halves the index.
pub(in crate::formats::odgi) trait StepNumber:
    Copy + Send + Sync + Default
{
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
pub(super) const MAX_STEP_RANGES: usize = 8;

/// Path steps grouped by the node they visit.
pub(super) struct StepIndex<I> {
    /// Steps on node `v` are `slots[offsets[v]..offsets[v + 1]]`, ordered by
    /// their rank on that node (which is also their order in all paths).
    pub(super) offsets: Vec<usize>,
    /// Global index (all paths concatenated) of the step in each slot.
    pub(super) slots: Vec<I>,
    /// Global index of the first step of every path, plus the total.
    pub(super) path_starts: Vec<usize>,
}

impl<I: StepNumber> StepIndex<I> {
    /// Counts and places the steps of every node range in parallel; each range's
    /// task scans all steps in order, so the result is deterministic.
    pub(super) fn new(node_count: usize, paths: &[Path]) -> PanResult<Self> {
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
    pub(super) fn rank(&self, node: NodeId, global: usize) -> u64 {
        let slots = &self.slots[self.offsets[node]..self.offsets[node + 1]];
        slots.partition_point(|&slot| slot.index() < global) as u64
    }

    /// Like [`StepIndex::rank`], for a caller that looks up increasing steps of
    /// each node: `hints` remembers where the last lookup on a node ended.
    pub(super) fn rank_after(
        &self,
        hints: &mut Vec<(NodeId, usize)>,
        node: NodeId,
        global: usize,
    ) -> u64 {
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
    pub(super) fn locate_after(&self, path_end: &mut usize, global: usize) -> (usize, usize) {
        // Empty paths share their start with the next path; the last path
        // starting at or before `global` is the one containing it.
        let starts = &self.path_starts;
        *path_end = gallop(starts.len(), *path_end, |i| starts[i] <= global);
        let path = *path_end - 1;
        (path, global - starts[path])
    }
}

/// Neighbor nodes per node record whose last rank lookup is remembered.
pub(in crate::formats::odgi) const MAX_RANK_HINTS: usize = 16;

/// First index in `from..len` where `is_before` is false, for a predicate that
/// holds on a prefix including `..from`. Searches exponentially from `from`,
/// so it is fast when the answer is close.
pub(super) fn gallop(len: usize, from: usize, is_before: impl Fn(usize) -> bool) -> usize {
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
