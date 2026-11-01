//! Unitig compaction: merges maximal non-branching chains of k-mers into
//! single nodes.
//!
//! The de Bruijn edges are first turned into a per-k-mer successor bitmask
//! (one byte per k-mer), after which the (k+1)-mer table is dropped. Chains are
//! then grown from every unvisited k-mer in both directions while the join is
//! unambiguous: the current k-mer has exactly one successor, that successor has
//! exactly one predecessor, and neither is a path endpoint (colored graphs
//! need every path segment to start and end at a unitig end).

use rayon::prelude::*;

use super::de_bruijn_graph::DbgCore;
use super::word::{KmerWord, Spelled, decode_base, decode_word, mask};
use crate::core::core_types::{Orientation, Step};
use crate::error::{PanGraphXError, PanResult};

/// The k-mer has been placed in a unitig.
const VISITED: u8 = 1;
/// No join on the left of the canonical spelling.
const BLOCK_LEFT: u8 = 2;
/// No join on the right of the canonical spelling.
const BLOCK_RIGHT: u8 = 4;

/// An oriented k-mer: node id and whether it is spelled as the reverse
/// complement of its canonical form.
type OrientedId = (usize, bool);

/// An edge between oriented unitigs.
pub struct UnitigEdge {
    pub from: (usize, Orientation),
    pub to: (usize, Orientation),
}

/// First and last k-mer of a path segment, as `(node id, reverse)`.
pub struct SegmentEnds {
    pub first: (usize, bool),
    pub last: (usize, bool),
}

/// A compacted de Bruijn graph.
pub struct Unitigs {
    /// Unitig sequences, indexed by unitig id.
    pub sequences: Vec<Vec<u8>>,
    pub edges: Vec<UnitigEdge>,
    /// Per k-mer id: `unitig << 32 | offset << 1 | reverse`, where `reverse`
    /// tells whether the unitig spells the k-mer's reverse complement.
    placement: Vec<u64>,
}

/// Where a k-mer lies in its unitig, as seen from a walk: the unitig, whether
/// the walk traverses the unitig backwards, and the k-mer's offset in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UnitigPos {
    unitig: u64,
    backwards: bool,
    offset: u64,
}

impl UnitigPos {
    /// Whether a walk stepping from `self` to `next` stays inside one unitig,
    /// i.e. `next` is the following k-mer of the same unitig traversal.
    #[inline]
    pub const fn continues_to(self, next: Self) -> bool {
        self.unitig == next.unitig
            && self.backwards == next.backwards
            && if next.backwards {
                next.offset + 1 == self.offset
            } else {
                self.offset + 1 == next.offset
            }
    }
}

impl Unitigs {
    /// The position of k-mer `id`, spelled in reverse if `reverse`, in its
    /// unitig.
    #[inline]
    pub fn position(&self, id: usize, reverse: bool) -> UnitigPos {
        let p = self.placement[id];
        UnitigPos {
            unitig: p >> 32,
            backwards: reverse != (p & 1 == 1),
            offset: (p & 0xffff_ffff) >> 1,
        }
    }
}

/// Translates a walk over k-mers into a walk over unitigs, one k-mer at a
/// time: consecutive k-mers of one unitig traversal become a single step.
pub struct Translator<'a> {
    unitigs: &'a Unitigs,
    /// Position of the previous k-mer of the walk.
    prev: Option<UnitigPos>,
}

impl<'a> Translator<'a> {
    pub const fn new(unitigs: &'a Unitigs) -> Self {
        Self {
            unitigs,
            prev: None,
        }
    }

    /// Adds k-mer `id` (spelled in reverse if `reverse`) to the walk,
    /// appending a unitig step to `out` unless it continues the previous one.
    /// Returns the k-mer's position.
    ///
    /// # Errors
    /// Returns an error if a unitig id does not fit in a [`Step`].
    pub fn push(&mut self, id: usize, reverse: bool, out: &mut Vec<Step>) -> PanResult<UnitigPos> {
        let pos = self.unitigs.position(id, reverse);
        if !self.prev.is_some_and(|prev| prev.continues_to(pos)) {
            out.push(Step::new(
                // An id that does not fit is rejected by `Step::new`.
                usize::try_from(pos.unitig).unwrap_or(usize::MAX),
                Orientation::from(pos.backwards),
            )?);
        }
        self.prev = Some(pos);
        Ok(pos)
    }

    /// Starts a new walk.
    pub const fn reset(&mut self) {
        self.prev = None;
    }
}

/// Compacts the de Bruijn graph `core` into unitigs. Unitigs never extend past
/// the endpoints in `segments`.
///
/// Returns the unitigs and the sorted k-mer table of `core` (the edge table is
/// released), so callers can still map k-mers to node ids.
pub fn compact<W: KmerWord>(
    core: DbgCore<W>,
    k: usize,
    segments: &[SegmentEnds],
) -> PanResult<(Unitigs, Vec<W>)> {
    if core.kmers.len() > MAX_LINKED_ID + 1 {
        return Err(PanGraphXError::DeBruijn(format!(
            "compaction supports at most {} k-mers, found {}",
            MAX_LINKED_ID + 1,
            core.kmers.len()
        )));
    }
    let adjacency = adjacency(&core, k);
    let DbgCore { kmers, edges } = core;
    drop(edges);

    let mut compactor = Compactor {
        kmers: &kmers,
        adjacency: &adjacency,
        flags: vec![0; kmers.len()],
        links: Vec::new(),
        k,
        mask_k: mask(k),
    };
    compactor.block_palindromes();
    for segment in segments {
        compactor.block(segment.first, true);
        compactor.block(segment.last, false);
    }
    compactor.links = compactor.links()?;
    let unitigs = compactor.run()?;
    Ok((unitigs, kmers))
}

/// Successor bases of every k-mer: bit `b` of the low nibble is set if the
/// canonical spelling followed by base `b` is an edge, the high nibble is the
/// same for the reverse-complement spelling.
fn adjacency<W: KmerWord>(core: &DbgCore<W>, k: usize) -> Vec<u8> {
    let successors = |x: Spelled<W>| -> u8 {
        let mut bits = 0;
        for b in 0..4u8 {
            let (fwd, rc) = extended(x, b, k);
            if core.edges.binary_search(&fwd.min(rc)).is_ok() {
                bits |= 1 << b;
            }
        }
        bits
    };
    core.kmers
        .par_iter()
        .map(|&kmer| {
            let spelled = Spelled::from_canonical(kmer, k);
            successors(spelled) | (successors(spelled.flip()) << 4)
        })
        .collect()
}

/// The (k+1)-mer `x` followed by base `b`, and its reverse complement.
#[inline]
fn extended<W: KmerWord>(x: Spelled<W>, b: u8, k: usize) -> (W, W) {
    (
        (x.fwd << 2) | W::from(b),
        (W::from(3 - b) << (2 * k)) | x.rc,
    )
}

/// No join in [`Compactor::links`].
const NO_LINK: u32 = u32::MAX;
/// Largest k-mer id a packed link can hold without colliding with [`NO_LINK`].
const MAX_LINKED_ID: usize = (1 << 31) - 2;

#[inline]
#[allow(clippy::cast_possible_truncation)]
const fn pack_link((id, reverse): OrientedId) -> u32 {
    // Ids are checked against MAX_LINKED_ID up front.
    ((id << 1) | reverse as usize) as u32
}

#[inline]
fn unpack_link(link: u32) -> OrientedId {
    (
        usize::try_from(link >> 1).unwrap_or(usize::MAX),
        link & 1 == 1,
    )
}

struct Compactor<'a, W> {
    kmers: &'a [W],
    adjacency: &'a [u8],
    flags: Vec<u8>,
    /// Per k-mer id, the k-mer joined after its canonical spelling (`[0]`)
    /// and after its reverse complement (`[1]`), or [`NO_LINK`].
    links: Vec<[u32; 2]>,
    k: usize,
    mask_k: W,
}

impl<W: KmerWord> Compactor<'_, W> {
    /// Palindromic k-mers read the same on both strands, so their orientation
    /// within a unitig would be ambiguous: keep each in a unitig of its own.
    fn block_palindromes(&mut self) {
        for (id, &kmer) in self.kmers.iter().enumerate() {
            let spelled = Spelled::from_canonical(kmer, self.k);
            if spelled.fwd == spelled.rc {
                self.flags[id] |= BLOCK_LEFT | BLOCK_RIGHT;
            }
        }
    }

    /// Forbids joins before (`before`) or after an oriented k-mer.
    fn block(&mut self, (id, reverse): OrientedId, before: bool) {
        self.flags[id] |= if before == reverse {
            BLOCK_RIGHT
        } else {
            BLOCK_LEFT
        };
    }

    fn blocked(&self, (id, reverse): OrientedId, before: bool) -> bool {
        let bit = if before == reverse {
            BLOCK_RIGHT
        } else {
            BLOCK_LEFT
        };
        self.flags[id] & bit != 0
    }

    /// Successor and predecessor base masks of an oriented k-mer.
    const fn masks(&self, (id, reverse): OrientedId) -> (u8, u8) {
        let bits = self.adjacency[id];
        if reverse {
            (bits >> 4, bits & 0xf)
        } else {
            (bits & 0xf, bits >> 4)
        }
    }

    /// The spelling of an oriented k-mer.
    fn spelled(&self, (id, reverse): OrientedId) -> Spelled<W> {
        let spelled = Spelled::from_canonical(self.kmers[id], self.k);
        if reverse { spelled.flip() } else { spelled }
    }

    /// The 2-bit code of the last base of an oriented k-mer.
    fn last_base(&self, (id, reverse): OrientedId) -> u8 {
        let kmer = self.kmers[id];
        if reverse {
            // The complement of the first base of the canonical spelling.
            3 - (kmer >> (2 * (self.k - 1))).last_base()
        } else {
            kmer.last_base()
        }
    }

    /// The oriented k-mer reached from `x` by appending base `b`.
    fn step(&self, x: Spelled<W>, b: u8) -> PanResult<OrientedId> {
        let (fwd, rc) = extended(x, b, self.k);
        let next = Spelled {
            fwd: fwd & self.mask_k,
            rc: rc >> 2,
        };
        let id = self.kmers.binary_search(&next.canonical()).map_err(|_| {
            PanGraphXError::DeBruijn("edge points to a k-mer missing from the node table".into())
        })?;
        Ok((id, next.is_reverse()))
    }

    /// The k-mer that `x` is joined to, if the join is unambiguous: `x` has
    /// one successor, which has one predecessor, neither side is blocked, and
    /// the successor is not `x` itself on either strand.
    fn join(&self, x: OrientedId) -> PanResult<Option<OrientedId>> {
        let (successors, _) = self.masks(x);
        if !successors.is_power_of_two() || self.blocked(x, false) {
            return Ok(None);
        }
        #[allow(clippy::cast_possible_truncation)]
        let b = successors.trailing_zeros() as u8; // < 4
        let next = self.step(self.spelled(x), b)?;
        let (_, predecessors) = self.masks(next);
        Ok(
            (predecessors.is_power_of_two() && !self.blocked(next, true) && next.0 != x.0)
                .then_some(next),
        )
    }

    /// Computes the joins of every k-mer, in parallel; walking the chains
    /// then needs no lookups.
    fn links(&self) -> PanResult<Vec<[u32; 2]>> {
        (0..self.kmers.len())
            .into_par_iter()
            .map(|id| {
                let link = |reverse| -> PanResult<u32> {
                    Ok(self.join((id, reverse))?.map_or(NO_LINK, pack_link))
                };
                Ok([link(false)?, link(true)?])
            })
            .collect()
    }

    /// The k-mers joined after `start`, in order, marking them visited.
    fn extend(&mut self, start: OrientedId) -> Vec<OrientedId> {
        let mut chain = Vec::new();
        let (mut id, mut reverse) = start;
        loop {
            let link = self.links[id][usize::from(reverse)];
            if link == NO_LINK {
                break;
            }
            let next = unpack_link(link);
            if self.flags[next.0] & VISITED != 0 {
                // Back at the start of a circular unitig.
                break;
            }
            self.flags[next.0] |= VISITED;
            chain.push(next);
            (id, reverse) = next;
        }
        chain
    }

    fn run(mut self) -> PanResult<Unitigs> {
        let mut sequences: Vec<Vec<u8>> = Vec::new();
        let mut placement = vec![0u64; self.kmers.len()];
        // First and last k-mer of every unitig, in unitig orientation.
        let mut ends: Vec<(OrientedId, OrientedId)> = Vec::new();

        for id in 0..self.kmers.len() {
            if self.flags[id] & VISITED != 0 {
                continue;
            }
            self.flags[id] |= VISITED;
            let start = (id, false);
            let right = self.extend(start);
            let left = self.extend((id, true));
            let chain: Vec<OrientedId> = left
                .into_iter()
                .rev()
                .map(|(i, reverse)| (i, !reverse))
                .chain(std::iter::once(start))
                .chain(right)
                .collect();

            let unitig = u64::try_from(sequences.len())
                .ok()
                .filter(|&u| u < 1 << 32)
                .ok_or_else(|| {
                    PanGraphXError::DeBruijn("too many unitigs for 32-bit ids".into())
                })?;
            let mut sequence = decode_word(self.spelled(chain[0]).fwd, self.k);
            sequence.extend(chain[1..].iter().map(|&x| decode_base(self.last_base(x))));
            for (offset, &(kmer_id, reverse)) in chain.iter().enumerate() {
                let offset = u64::try_from(offset)
                    .ok()
                    .filter(|&o| o < 1 << 31)
                    .ok_or_else(|| PanGraphXError::DeBruijn("unitig too long".into()))?;
                placement[kmer_id] = (unitig << 32) | (offset << 1) | u64::from(reverse);
            }
            ends.push((chain[0], chain[chain.len() - 1]));
            sequences.push(sequence);
        }

        let edges = self.unitig_edges(&ends, &placement)?;
        Ok(Unitigs {
            sequences,
            edges,
            placement,
        })
    }

    /// Edges between unitig ends, each listed once.
    fn unitig_edges(
        &self,
        ends: &[(OrientedId, OrientedId)],
        placement: &[u64],
    ) -> PanResult<Vec<UnitigEdge>> {
        let mut edges = ends
            .par_iter()
            .enumerate()
            .map(|(unitig, &(first, last))| {
                let mut out = Vec::new();
                // Leaving the unitig forward from its last k-mer, or backwards
                // from its first one.
                for (backwards, exit) in [(false, last), (true, (first.0, !first.1))] {
                    let (successors, _) = self.masks(exit);
                    let spelled = self.spelled(exit);
                    for b in (0..4u8).filter(|b| successors & (1 << b) != 0) {
                        let (next_id, next_reverse) = self.step(spelled, b)?;
                        let p = placement[next_id];
                        let target = usize::try_from(p >> 32).unwrap_or(usize::MAX);
                        let target_backwards = next_reverse != (p & 1 == 1);
                        let edge = ((unitig, backwards), (target, target_backwards));
                        let twin = ((target, !target_backwards), (unitig, !backwards));
                        out.push(edge.min(twin));
                    }
                }
                Ok(out)
            })
            .collect::<PanResult<Vec<Vec<_>>>>()?
            .concat();
        edges.par_sort_unstable();
        edges.dedup();
        Ok(edges
            .into_iter()
            .map(|((a, ar), (b, br))| UnitigEdge {
                from: (a, Orientation::from(ar)),
                to: (b, Orientation::from(br)),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use crate::de_bruijn_conversion::de_bruijn_graph::tests::{canon, graph};
    use crate::{CoreGraphDTO, DeBruijn, Kmer, Orientation};

    fn compacted(seqs: &[&[u8]], paths: &[(&str, &[(usize, bool)])], k: usize) -> CoreGraphDTO {
        let g = graph(seqs, &[], paths);
        DeBruijn::from_directed_graph(g, k)
            .unwrap()
            .into_compacted_dto()
            .unwrap()
    }

    fn oriented(seq: &[u8], o: Orientation) -> Vec<u8> {
        if o == Orientation::Forward {
            seq.to_vec()
        } else {
            Kmer::from_bases(seq).rev_comp().to_bytes()
        }
    }

    /// Every edge must join two unitigs that overlap by k-1 bases.
    fn assert_edges_overlap(dto: &CoreGraphDTO, k: usize) {
        for e in dto.edges() {
            let from = oriented(&dto.nodes()[e.from_node].sequence, e.from_orient);
            let to = oriented(&dto.nodes()[e.to_node].sequence, e.to_orient);
            assert_eq!(
                from[from.len() - (k - 1)..],
                to[..k - 1],
                "edge {e:?} does not overlap"
            );
        }
    }

    #[test]
    fn test_linear_sequence_is_one_unitig() {
        let dto = compacted(&[b"AACTGGCATTCGAGTC"], &[("p", &[(0, false)])], 5);
        assert_eq!(dto.nodes().len(), 1);
        let seq = &dto.nodes()[0].sequence;
        assert!(seq == b"AACTGGCATTCGAGTC" || canon(seq) == canon(b"AACTGGCATTCGAGTC"));
        assert_eq!(dto.edges().len(), 0);
    }

    #[test]
    fn test_snp_bubble() {
        // Two haplotypes differing in the middle base: a bubble of
        // prefix -> {A, C} -> suffix.
        let a: &[u8] = b"TTGACCAGTAGGCAT";
        let b: &[u8] = b"TTGACCACTAGGCAT";
        let dto = compacted(&[a, b], &[("a", &[(0, false)]), ("b", &[(1, false)])], 5);
        assert_eq!(dto.nodes().len(), 4);
        assert_eq!(dto.edges().len(), 4);
        assert_edges_overlap(&dto, 5);
    }

    #[test]
    fn test_cycle_is_one_unitig_with_self_edge() {
        // The circular sequence AACGC repeated: every 3-mer has one successor.
        let dto = compacted(&[b"AACGCAACGCAACGCAA"], &[("p", &[(0, false)])], 3);
        assert_eq!(dto.nodes().len(), 1);
        assert_eq!(dto.edges().len(), 1);
        assert_edges_overlap(&dto, 3);
    }
}
