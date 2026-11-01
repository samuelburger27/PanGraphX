use super::core_types::{Orientation, Path};
use super::graph::CoreGraph;
use bio::alphabets::dna::revcomp as reverse_complement;
use std::borrow::Cow;

impl CoreGraph {
    /// Return an iterator over the node sequences for the given path.
    /// The sequences are returned in the correct orientation
    /// (if orientation is Reverse, the reverse complement is returned).
    pub fn path_node_sequences<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = Cow<'a, [u8]>> + 'a {
        path.steps.iter().map(move |step| {
            let node = &self.nodes()[step.node_id()];
            match step.orientation() {
                Orientation::Forward => Cow::Borrowed(node.sequence.as_slice()),
                Orientation::Reverse => Cow::Owned(reverse_complement(&node.sequence)),
            }
        })
    }
}

/// Complements a single nucleotide, preserving case. Any other byte is
/// returned unchanged.
#[must_use]
pub(crate) const fn complement_base(b: u8) -> u8 {
    match b {
        b'A' => b'T',
        b'T' => b'A',
        b'C' => b'G',
        b'G' => b'C',
        b'a' => b't',
        b't' => b'a',
        b'c' => b'g',
        b'g' => b'c',
        _ => b,
    }
}

/// Iterator over the bases of a node sequence read in a given orientation:
/// the sequence itself for `Forward`, its reverse complement for `Reverse`.
/// Unlike [`CoreGraph::path_node_sequences`] it never allocates.
pub(crate) struct OrientedBases<'a> {
    bases: std::slice::Iter<'a, u8>,
    reverse: bool,
}

impl Iterator for OrientedBases<'_> {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<u8> {
        if self.reverse {
            self.bases.next_back().map(|&b| complement_base(b))
        } else {
            self.bases.next().copied()
        }
    }
}

/// Returns the bases of `sequence` read in `orientation`, skipping the first
/// `skip` of them (the overlap with the previous node of a walk).
pub(crate) fn oriented_bases(
    sequence: &[u8],
    orientation: Orientation,
    skip: usize,
) -> OrientedBases<'_> {
    let skip = skip.min(sequence.len());
    let bases = match orientation {
        Orientation::Forward => &sequence[skip..],
        Orientation::Reverse => &sequence[..sequence.len() - skip],
    };
    OrientedBases {
        bases: bases.iter(),
        reverse: orientation == Orientation::Reverse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oriented_bases() {
        let seq = b"AACGT";
        let fwd: Vec<u8> = oriented_bases(seq, Orientation::Forward, 0).collect();
        assert_eq!(fwd, b"AACGT");
        let rev: Vec<u8> = oriented_bases(seq, Orientation::Reverse, 0).collect();
        assert_eq!(rev, b"ACGTT");
        let rev_skip: Vec<u8> = oriented_bases(seq, Orientation::Reverse, 2).collect();
        assert_eq!(rev_skip, b"GTT");
        assert_eq!(oriented_bases(seq, Orientation::Forward, 9).next(), None);
    }
}
