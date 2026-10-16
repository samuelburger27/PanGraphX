use std::fmt::Debug;
use std::fmt::Display;

use super::word::{ENCODE, KmerWord, Spelled, decode_word, mask, rev_comp_word};
use crate::core::core_types::Orientation;
use crate::error::{PanGraphXError, PanResult};

/// Largest supported k-mer size. A de Bruijn edge is stored as its (k+1)-mer,
/// which must fit in a `u128` (64 bases).
pub const MAX_K: usize = 63;

/// Checks that `k` is a supported k-mer size (`1..=MAX_K`).
///
/// # Errors
/// Returns `PanGraphXError::DeBruijn` if `k` is out of range.
pub fn validate_k(k: usize) -> PanResult<()> {
    if (1..=MAX_K).contains(&k) {
        Ok(())
    } else {
        Err(PanGraphXError::DeBruijn(format!(
            "k-mer size must be between 1 and {MAX_K}, got {k}"
        )))
    }
}

/// 2-bit code of a base; non-nucleotides are encoded as 'A'.
#[inline]
const fn encode_lossy(b: u8) -> u8 {
    ENCODE[b as usize] & 3
}

/// Performs a "rolling" update of a k-mer code.
///
/// This operation simulates a sliding window moving one base to the right.
/// 1. Masks the current code to remove the most significant base (the oldest).
/// 2. Left-shifts the result by 2 bits.
/// 3. ORs the new base into the least significant position.
///
/// Non-nucleotide bases are encoded as 'A'.
///
/// # Arguments
/// * `code`: The current 2-bit encoded k-mer.
/// * `k`: The length of the k-mer (must be 1 <= k <= 63).
/// * `next_base`: The ASCII byte of the incoming nucleotide.
#[must_use]
pub fn roll_kmer(code: u128, k: usize, next_base: u8) -> u128 {
    let next_code = u128::from(encode_lossy(next_base));
    ((code & mask::<u128>(k - 1)) << 2) | next_code
}

/// A compact representation of a DNA k-mer.
///
/// The sequence is stored in a `u128` using 2-bit encoding.
/// This allows for efficient storage and hashing of k-mers up to length k=63.
/// The de Bruijn construction itself stores bare words; this type is a
/// self-describing view of one k-mer.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Kmer {
    /// The 2-bit encoded sequence. Uses `2 * k` bits.
    pub code: u128,
    /// The length of the k-mer.
    pub k: usize,
}

impl Debug for Kmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.to_bytes();
        write!(f, "Kmer({})", String::from_utf8_lossy(&bytes))
    }
}

impl Kmer {
    /// Creates a Kmer from a slice of bytes (e.g., `b"ACGT"`).
    /// Non-nucleotide bytes are encoded as 'A'.
    #[inline]
    #[must_use]
    pub fn from_bases(bases: &[u8]) -> Self {
        let mut code: u128 = 0;
        for &b in bases {
            code = (code << 2) | u128::from(encode_lossy(b));
        }
        Self {
            code,
            k: bases.len(),
        }
    }

    /// Computes the reverse complement of the k-mer.
    ///
    /// This reverses the order of bases and complements each base (A <-> T, C <-> G).
    #[inline]
    #[must_use]
    pub fn rev_comp(&self) -> Self {
        Self {
            code: rev_comp_word(self.code, self.k),
            k: self.k,
        }
    }

    /// Returns the canonical representation of the k-mer.
    ///
    /// The canonical form is defined as the lexicographically smaller sequence
    /// between the forward k-mer and its reverse complement.
    #[inline]
    #[must_use]
    pub fn canonical(&self) -> Self {
        let rc = self.rev_comp();
        if self.code <= rc.code { *self } else { rc }
    }

    /// Decodes the `u128` back into a human-readable byte vector.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        decode_word(self.code, self.k)
    }
}

impl Display for Kmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.to_bytes();
        write!(f, "{}", String::from_utf8_lossy(&bytes))
    }
}

/// A K-mer annotated with its orientation relative to the canonical form.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrientedKmer {
    pub kmer: Kmer,
    pub direction: Orientation,
}

impl Debug for OrientedKmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.to_bytes();
        write!(
            f,
            "OrientedKmer{{Sequence {}, Orientation {}}}",
            String::from_utf8_lossy(&bytes),
            self.direction
        )
    }
}

impl OrientedKmer {
    /// Constructs an `OrientedKmer` from a raw integer code.
    ///
    /// Determines if the provided code corresponds to the Forward or Reverse
    /// canonical direction.
    #[must_use]
    pub fn from_code(code: u128, k: usize) -> Self {
        let raw = Kmer { code, k };
        let canonical = raw.canonical();
        let direction = Orientation::from(canonical.code != raw.code);
        Self {
            kmer: canonical,
            direction,
        }
    }

    /// Constructs an `OrientedKmer` from a byte slice.
    #[inline]
    #[must_use]
    pub fn from_bases(bases: &[u8]) -> Self {
        let raw = Kmer::from_bases(bases);
        Self::from_code(raw.code, raw.k)
    }

    /// Builds the view of an internal spelled word.
    pub(crate) fn from_spelled<W: KmerWord>(spelled: Spelled<W>, k: usize) -> Self {
        Self {
            kmer: Kmer {
                code: spelled.canonical().to_u128(),
                k,
            },
            direction: spelled.orientation(),
        }
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.direction == Orientation::Forward {
            self.kmer.to_bytes()
        } else {
            self.kmer.rev_comp().to_bytes()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_k() {
        assert!(validate_k(0).is_err());
        assert!(validate_k(1).is_ok());
        assert!(validate_k(MAX_K).is_ok());
        assert!(validate_k(MAX_K + 1).is_err());
    }

    // --- Kmer Struct Tests ---

    #[test]
    fn test_kmer_round_trip() {
        let seq = b"ACGTACGT";
        let kmer = Kmer::from_bases(seq);
        let output = kmer.to_bytes();
        assert_eq!(output, seq, "Round trip encoding/decoding failed");
    }

    #[test]
    fn test_kmer_rev_comp() {
        // Sequence: TTCG (T=3, T=3, C=1, G=2)
        // RC:       CGAA (C=1, G=2, A=0, A=0)
        let seq = b"TTCG";
        let kmer = Kmer::from_bases(seq);
        let rc = kmer.rev_comp();

        assert_eq!(rc.to_bytes(), b"CGAA");
    }

    #[test]
    fn test_canonicalization() {
        // Case 1: Forward is smaller
        // AAAA (00000000) vs TTTT (11111111)
        let kmer_fwd = Kmer::from_bases(b"AAAA");
        assert_eq!(kmer_fwd.canonical().to_bytes(), b"AAAA");

        // Case 2: Reverse is smaller
        // TTTT vs AAAA
        let kmer_rev = Kmer::from_bases(b"TTTT");
        assert_eq!(kmer_rev.canonical().to_bytes(), b"AAAA");

        // Case 3: Mixed
        // TGCA (T=3, G=2, C=1, A=0) -> RC = TGCA
        // This is a palindrome
        let palindrome = Kmer::from_bases(b"TGCA");
        assert_eq!(palindrome.canonical().to_bytes(), b"TGCA");
    }

    // --- OrientedKmer Tests ---

    #[test]
    fn test_orientation_detection() {
        // "AAAA" is canonical (smaller than TTTT), so it should be Forward
        let ok_fwd = OrientedKmer::from_bases(b"AAAA");
        assert_eq!(ok_fwd.direction, Orientation::Forward);
        assert_eq!(ok_fwd.kmer.to_bytes(), b"AAAA");

        // "TTTT" is NOT canonical (larger than AAAA), so it should be Reverse
        // The stored kmer should be the canonical one (AAAA)
        let ok_rev = OrientedKmer::from_bases(b"TTTT");
        assert_eq!(ok_rev.direction, Orientation::Reverse);
        assert_eq!(ok_rev.kmer.to_bytes(), b"AAAA");
    }

    // --- Rolling Tests ---

    #[test]
    fn test_roll_kmer_logic() {
        let k = 4;
        // Start: ACGT
        let start_seq = b"ACGT";
        let start_kmer = Kmer::from_bases(start_seq);

        // Roll in 'A' -> Should become CGTA
        let next_code = roll_kmer(start_kmer.code, k, b'A');
        let next_kmer = Kmer { code: next_code, k };

        assert_eq!(next_kmer.to_bytes(), b"CGTA");
    }
}
