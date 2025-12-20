use std::collections::HashMap;
use std::fmt::Display;

use crate::core::graph::{Node, Orientation, Path};
use crate::core::lookup_graph::LookUpGraph;

/// Encode 2-bit DNA: A=0, C=1, G=2, T=3, N=0
#[inline(always)]
fn encode_base(b: u8) -> u8 {
    match b {
        b'A' | b'a' => 0,
        b'C' | b'c' => 1,
        b'G' | b'g' => 2,
        b'T' | b't' => 3,
        _ => 0, // N acts as A
    }
}

/// Reverse-complement single base (2-bit)
#[inline(always)]
fn rc_base(x: u8) -> u8 {
    match x {
        0 => 3, // A->T
        1 => 2, // C->G
        2 => 1, // G->C
        3 => 0, // T->A
        _ => 0,
    }
}

/// Roll k-mer by adding next base and removing first base
/// Assumes 1 ≤ k ≤ 63
#[inline(always)]
pub fn roll_kmer(code: u128, k: usize, next_base: u8) -> u128 {
    let mask: u128 = (1u128 << (2 * (k - 1))) - 1;
    let next_code = encode_base(next_base) as u128;
    ((code & mask) << 2) | next_code
}

/// max k-mer can be increased if needed ???
/// 2-bit encoded k-mer stored in u128 (supports k ≤ 63)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Kmer {
    pub code: u128, // 2*k bits
    pub k: usize,
}

impl Kmer {
    /// Create from raw slice of bytes
    #[inline]
    pub fn from_bases(bases: &[u8]) -> Self {
        let mut code: u128 = 0;
        for &b in bases {
            code = (code << 2) | encode_base(b) as u128;
        }
        Self {
            code,
            k: bases.len(),
        }
    }

    /// Compute reverse complement
    #[inline]
    pub fn rev_comp(&self) -> Self {
        let mut x = self.code;
        let mut rc: u128 = 0;
        for _ in 0..self.k {
            let b = (x & 3) as u8;
            rc = (rc << 2) | rc_base(b) as u128;
            x >>= 2;
        }
        Self {
            code: rc,
            k: self.k,
        }
    }

    /// Canonical form = lexicographically smaller among forward and RC
    #[inline]
    pub fn canonical(&self) -> Self {
        let rc = self.rev_comp();
        if self.code <= rc.code { *self } else { rc }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.k);
        let mut x = self.code;
        for _ in (0..self.k).rev() {
            let b = (x & 3) as u8;
            let base = match b {
                0 => b'A',
                1 => b'C',
                2 => b'G',
                3 => b'T',
                _ => b'N',
            };
            bytes.push(base);
            x >>= 2;
        }
        bytes
    }
}

impl Display for Kmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.to_bytes();
        write!(f, "{}", String::from_utf8_lossy(&bytes))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OrientedKmer {
    pub kmer: Kmer,
    pub direction: Orientation,
}

impl OrientedKmer {
    pub fn from_code(code: u128, k: usize) -> Self {
        let raw = Kmer { code, k };
        let canonical = raw.canonical();
        if canonical.code == raw.code {
            return Self {
                kmer: canonical,
                direction: Orientation::Forward,
            };
        }
        return Self {
            kmer: canonical,
            direction: Orientation::Reverse,
        };
    }

    #[inline]
    pub fn from_bases(bases: &[u8]) -> Self {
        let raw = Kmer::from_bases(bases);
        let canonical = raw.canonical();
        if canonical.code == raw.code {
            return Self {
                kmer: canonical,
                direction: Orientation::Forward,
            };
        }
        return Self {
            kmer: canonical,
            direction: Orientation::Reverse,
        };
    }
}

impl LookUpGraph<'_> {
    /// Extract oriented k-mers from path sequences.
    pub fn extract_oriented_kmers(&self, k: usize) -> HashMap<&Path, Vec<OrientedKmer>> {
        let mut kmers = HashMap::new();
        for path in &self.graph.paths {
            let extracted = self.extract_kmers_from_path(path, k);
            kmers.insert(path, extracted);
        }
        kmers
    }

    /// Extract k-mers from a single path
    #[inline]
    fn extract_kmers_from_path(&self, path: &Path, k: usize) -> Vec<OrientedKmer> {
        let mut result = Vec::new();
        
        // Iterate over node sequences as slices
        for seq in self.path_node_original_sequence(path) {
            let mut code: u128 = 0;
            for (i, &base) in seq.into_iter().enumerate() {
                // Fill window until size k
                if i < k {
                    code = (code << 2) | encode_base(base) as u128;
                } 
                else {
                    code = roll_kmer(code, k, base);
                    let kmer = OrientedKmer::from_code(code, k);
                    result.push(kmer);
                }
            }
        }
        result
    }
}
