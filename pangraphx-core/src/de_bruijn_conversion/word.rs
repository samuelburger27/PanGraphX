//! 2-bit encoded DNA words and the rolling window used by the de Bruijn
//! construction. Words are bare `u64`/`u128` integers; the k-mer size is
//! carried by the caller instead of being stored in every word.

use std::fmt::Debug;
use std::hash::Hash;
use std::ops::{BitAnd, BitOr, Not, Shl, Shr};

use crate::core::core_types::Orientation;

/// Largest k-mer size whose (k+1)-mers fit in a `u64` (32 bases).
pub const MAX_SMALL_K: usize = 31;

/// Marks a byte that is not a nucleotide in [`ENCODE`].
pub const INVALID: u8 = 4;

/// 2-bit codes of the nucleotides, indexed by ASCII byte.
///
/// # Mapping
/// * 'A' / 'a' -> 0 (binary 00)
/// * 'C' / 'c' -> 1 (binary 01)
/// * 'G' / 'g' -> 2 (binary 10)
/// * 'T' / 't' -> 3 (binary 11)
/// * anything else -> [`INVALID`]
///
/// With this mapping the complement of a base `c` is `3 - c`.
pub const ENCODE: [u8; 256] = {
    let mut table = [INVALID; 256];
    table[b'A' as usize] = 0;
    table[b'a' as usize] = 0;
    table[b'C' as usize] = 1;
    table[b'c' as usize] = 1;
    table[b'G' as usize] = 2;
    table[b'g' as usize] = 2;
    table[b'T' as usize] = 3;
    table[b't' as usize] = 3;
    table
};

const DECODE: [u8; 4] = *b"ACGT";

/// The ASCII nucleotide of a 2-bit code.
#[inline]
pub const fn decode_base(code: u8) -> u8 {
    DECODE[(code & 3) as usize]
}

/// An unsigned integer holding a 2-bit encoded DNA word (`u64` or `u128`).
///
/// Bases are stored most significant first: the last base of the word is in
/// the lowest two bits.
pub trait KmerWord:
    Copy
    + Ord
    + Hash
    + Send
    + Sync
    + Debug
    + Default
    + From<u8>
    + Shl<usize, Output = Self>
    + Shr<usize, Output = Self>
    + BitAnd<Output = Self>
    + BitOr<Output = Self>
    + Not<Output = Self>
{
    /// Width of the word in bits.
    const BITS: usize;

    /// Widens the word to a `u128`.
    fn to_u128(self) -> u128;

    /// Folds the word into 64 bits (used for hashing).
    fn fold64(self) -> u64;

    /// Returns the 2-bit code of the last base.
    fn last_base(self) -> u8;
}

impl KmerWord for u64 {
    const BITS: usize = 64;

    #[inline]
    fn to_u128(self) -> u128 {
        u128::from(self)
    }

    #[inline]
    fn fold64(self) -> u64 {
        self
    }

    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    fn last_base(self) -> u8 {
        // Masked to two bits, so the cast is lossless.
        (self & 3) as u8
    }
}

impl KmerWord for u128 {
    const BITS: usize = 128;

    #[inline]
    fn to_u128(self) -> u128 {
        self
    }

    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    fn fold64(self) -> u64 {
        // Intentionally splits the word into its two halves.
        (self as u64) ^ ((self >> 64) as u64).rotate_left(29)
    }

    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    fn last_base(self) -> u8 {
        // Masked to two bits, so the cast is lossless.
        (self & 3) as u8
    }
}

/// Returns a mask selecting the lowest `bases` bases (2 bits each) of a word.
#[inline]
pub fn mask<W: KmerWord>(bases: usize) -> W {
    if 2 * bases >= W::BITS {
        !W::default()
    } else {
        !(!W::default() << (2 * bases))
    }
}

/// Computes the reverse complement of a word of `bases` bases.
#[inline]
pub fn rev_comp_word<W: KmerWord>(code: W, bases: usize) -> W {
    let mut x = code;
    let mut rc = W::default();
    for _ in 0..bases {
        rc = (rc << 2) | W::from(3 - x.last_base());
        x = x >> 2;
    }
    rc
}

/// Decodes a word of `bases` bases back into ASCII nucleotides.
pub fn decode_word<W: KmerWord>(code: W, bases: usize) -> Vec<u8> {
    let mut bytes = vec![0; bases];
    let mut x = code;
    for byte in bytes.iter_mut().rev() {
        *byte = decode_base(x.last_base());
        x = x >> 2;
    }
    bytes
}

/// A word together with its reverse complement, i.e. one strand-specific
/// spelling of a k-mer (or (k+1)-mer).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Spelled<W> {
    /// The word as read.
    pub fwd: W,
    /// The reverse complement of `fwd`.
    pub rc: W,
}

impl<W: KmerWord> Spelled<W> {
    /// The canonical form: the smaller of the word and its reverse complement.
    #[inline]
    pub fn canonical(self) -> W {
        self.fwd.min(self.rc)
    }

    /// Whether this spelling is the reverse complement of the canonical form.
    /// Palindromes count as forward.
    #[inline]
    pub fn is_reverse(self) -> bool {
        self.rc < self.fwd
    }

    #[inline]
    pub fn orientation(self) -> Orientation {
        Orientation::from(self.is_reverse())
    }
}

/// A sliding window over the last `k + 1` bases of a sequence.
///
/// It keeps both the forward code and its reverse complement up to date in
/// O(1) per base, so canonical k-mers and (k+1)-mers never need a separate
/// reverse-complement pass. Non-nucleotide bases reset the window.
#[derive(Clone, Copy, Debug)]
pub struct RollingWindow<W> {
    /// Last `len` bases, forward (unused high bases are zero).
    fwd: W,
    /// Reverse complement of the last `len` bases, aligned to `k + 1` bases:
    /// the complement of the newest base is the most significant one.
    rc: W,
    /// Number of valid bases in the window, at most `k + 1`.
    len: usize,
    k: usize,
    mask_k: W,
    mask_k1: W,
}

impl<W: KmerWord> RollingWindow<W> {
    /// Creates an empty window. `k + 1` bases must fit in `W`.
    pub fn new(k: usize) -> Self {
        Self {
            fwd: W::default(),
            rc: W::default(),
            len: 0,
            k,
            mask_k: mask(k),
            mask_k1: mask(k + 1),
        }
    }

    /// Slides the window by one base. Returns `false` (and empties the window)
    /// if `base` is not a nucleotide.
    #[inline]
    pub fn push(&mut self, base: u8) -> bool {
        let code = ENCODE[usize::from(base)];
        if code == INVALID {
            self.reset();
            return false;
        }
        self.fwd = ((self.fwd << 2) | W::from(code)) & self.mask_k1;
        self.rc = (self.rc >> 2) | (W::from(3 - code) << (2 * self.k));
        if self.len <= self.k {
            self.len += 1;
        }
        true
    }

    pub fn reset(&mut self) {
        self.fwd = W::default();
        self.rc = W::default();
        self.len = 0;
    }

    /// Number of valid bases in the window (at most `k + 1`).
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// The full window state; equal states have equal futures.
    #[inline]
    pub const fn state(&self) -> (W, usize) {
        (self.fwd, self.len)
    }

    /// The k-mer ending at the last base, once at least `k` bases are valid.
    #[inline]
    pub fn kmer(&self) -> Option<Spelled<W>> {
        (self.len >= self.k).then(|| Spelled {
            fwd: self.fwd & self.mask_k,
            rc: self.rc >> 2,
        })
    }

    /// The canonical (k+1)-mer (a de Bruijn edge) ending at the last base,
    /// once the window is full.
    #[inline]
    pub fn edge(&self) -> Option<W> {
        (self.len > self.k).then(|| self.fwd.min(self.rc))
    }
}

/// Splits a canonical (k+1)-mer into the oriented k-mers it connects: its
/// prefix (`from`) and its suffix (`to`).
#[inline]
pub fn edge_ends<W: KmerWord>(edge: W, k: usize) -> (Spelled<W>, Spelled<W>) {
    let rc = rev_comp_word(edge, k + 1);
    let mask_k = mask(k);
    let from = Spelled {
        fwd: edge >> 2,
        rc: rc & mask_k,
    };
    let to = Spelled {
        fwd: edge & mask_k,
        rc: rc >> 2,
    };
    (from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kmer;

    /// Canonical k-mers and (k+1)-mers of `seq` produced by a rolling window.
    fn roll<W: KmerWord>(seq: &[u8], k: usize) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let mut win = RollingWindow::<W>::new(k);
        let mut kmers = Vec::new();
        let mut edges = Vec::new();
        for &b in seq {
            if !win.push(b) {
                continue;
            }
            if let Some(km) = win.kmer() {
                kmers.push(decode_word(km.canonical(), k));
            }
            if let Some(e) = win.edge() {
                edges.push(decode_word(e, k + 1));
            }
        }
        (kmers, edges)
    }

    fn canonical_str(s: &[u8]) -> Vec<u8> {
        Kmer::from_bases(s).canonical().to_bytes()
    }

    #[test]
    fn test_mask_full_width() {
        assert_eq!(mask::<u64>(32), u64::MAX);
        assert_eq!(mask::<u64>(31), u64::MAX >> 2);
        assert_eq!(mask::<u128>(64), u128::MAX);
        assert_eq!(mask::<u128>(1), 3);
    }

    #[test]
    fn test_rev_comp_word() {
        let w = Kmer::from_bases(b"TTCG").code;
        assert_eq!(decode_word(rev_comp_word(w, 4), 4), b"CGAA");
    }

    #[test]
    fn test_rolling_window_matches_naive() {
        let seq = b"ACGTTGCAAGGCTTACGATCGATTTGCA";
        for k in [1, 2, 3, 5, 8] {
            let (kmers, edges) = roll::<u64>(seq, k);
            let naive_kmers: Vec<Vec<u8>> = seq.windows(k).map(canonical_str).collect();
            let naive_edges: Vec<Vec<u8>> = seq.windows(k + 1).map(canonical_str).collect();
            assert_eq!(kmers, naive_kmers, "k-mers, k={k}");
            assert_eq!(edges, naive_edges, "edges, k={k}");
            // The u128 path agrees with the u64 one.
            assert_eq!(roll::<u128>(seq, k), (kmers, edges));
        }
    }

    #[test]
    fn test_rolling_window_full_width() {
        // k = 31 fills a u64 edge word, k = 63 a u128 one.
        let seq: Vec<u8> = b"ACGTTGCAAGGCTTACGATCGATTTGCAGG"
            .iter()
            .cycle()
            .take(80)
            .copied()
            .collect();
        for (k, small) in [(31, true), (63, false)] {
            let (kmers, edges) = if small {
                roll::<u64>(&seq, k)
            } else {
                roll::<u128>(&seq, k)
            };
            let naive_kmers: Vec<Vec<u8>> = seq.windows(k).map(canonical_str).collect();
            let naive_edges: Vec<Vec<u8>> = seq.windows(k + 1).map(canonical_str).collect();
            assert_eq!(kmers, naive_kmers, "k-mers, k={k}");
            assert_eq!(edges, naive_edges, "edges, k={k}");
        }
    }

    #[test]
    fn test_rolling_window_resets_on_invalid_base() {
        let (kmers, edges) = roll::<u64>(b"ACGNTTA", 2);
        let expected: Vec<Vec<u8>> = [b"AC", b"CG", b"TT", b"TA"]
            .iter()
            .map(|s| canonical_str(*s))
            .collect();
        assert_eq!(kmers, expected);
        // No (k+1)-mer spans the N.
        let expected_edges: Vec<Vec<u8>> =
            [b"ACG", b"TTA"].iter().map(|s| canonical_str(*s)).collect();
        assert_eq!(edges, expected_edges);
    }

    #[test]
    fn test_edge_ends() {
        // ACGT: prefix ACG (canonical ACG, forward), suffix CGT (canonical ACG, reverse).
        let edge = Kmer::from_bases(b"ACGT").canonical().code;
        let (from, to) = edge_ends(u64::try_from(edge).unwrap(), 3);
        assert_eq!(decode_word(from.fwd, 3), b"ACG");
        assert_eq!(from.orientation(), Orientation::Forward);
        assert_eq!(decode_word(to.fwd, 3), b"CGT");
        assert_eq!(decode_word(to.canonical(), 3), b"ACG");
        assert_eq!(to.orientation(), Orientation::Reverse);
    }
}
