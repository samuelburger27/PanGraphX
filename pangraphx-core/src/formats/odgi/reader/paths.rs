//! Paths: the steps stored on the node records are linked lists, walked per path.

use crate::core::core_types::{NodeId, Orientation, Path, Step};
use crate::error::{PanGraphXError, PanResult};
use crate::formats::odgi::encoding::to_usize;
use rayon::prelude::*;

/// The part of a stored step needed to walk a path, in 8 bytes (a path's
/// steps are the bulk of an ODGI file, so this dominates the reader's memory).
#[derive(Clone, Copy)]
pub(super) struct StepLink {
    /// Rank of the node holding the next step, [`StepLink::END`] for the last
    /// step of a path.
    pub(super) next_node: u32,
    /// Rank of the next step among the steps of that node `<< 1 | is_reverse`.
    pub(super) rank_rev: u32,
}

impl StepLink {
    /// `next_node` of a path's last step. Node ranks are below it (see `read_graph`).
    pub(super) const END: u32 = u32::MAX;
}

/// A path's metadata record.
pub(super) struct PathMeta {
    pub(super) name: Vec<u8>,
    pub(super) length: u64,
    pub(super) first_handle: u64,
    pub(super) first_rank: u64,
}

/// Steps of all nodes, indexed by node rank, used to walk the paths.
pub(super) struct StepTable {
    /// Steps of node rank `r` are `links[offsets[r]..offsets[r + 1]]`.
    pub(super) offsets: Vec<usize>,
    pub(super) links: Vec<StepLink>,
    pub(super) rank_to_node: Vec<NodeId>,
}

/// Paths a walker advances in lockstep. A path is a linked list scattered over
/// the node records, so every step is a dependent cache miss; interleaving
/// independent paths lets the CPU wait for several misses at once.
pub(super) const WALK_LANES: usize = 16;

/// A path being walked.
pub(super) struct Walk {
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
    pub(super) fn walk_all(&self, metas: Vec<PathMeta>) -> PanResult<Vec<Path>> {
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
