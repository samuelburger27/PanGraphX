use crate::core::graph::{CoreGraph, Path};
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

// TODO max k-mer can be increased if needed ???
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

    pub fn to_string(&self) -> String {
        let mut s = String::with_capacity(self.k);
        let mut x = self.code;
        let mut bases = vec![b'A'; self.k];
        for i in (0..self.k).rev() {
            let b = (x & 3) as u8;
            bases[i] = match b {
                0 => b'A',
                1 => b'C',
                2 => b'G',
                3 => b'T',
                _ => b'N',
            };
            x >>= 2;
        }
        s.push_str(&String::from_utf8_lossy(&bases));
        s
    }
}

impl LookUpGraph<'_> {
    /// Extract canonical k-mers from path sequences as Kmer structs.
    pub fn extract_canonical_kmers(&self, k: usize) -> Vec<Kmer> {
        let mut kmers = Vec::new();
        for path in &self.graph.paths {
            self.extract_kmers_from_path(path, k, &mut kmers);
        }

        kmers
    }

    /// Extract k-mers from a single path into existing Vec<Kmer>
    #[inline]
    fn extract_kmers_from_path(&self, path: &Path, k: usize, out: &mut Vec<Kmer>) {
        let mut window = Vec::with_capacity(k);

        // Iterate over node sequences as slices
        for seq in self.path_node_sequences(path) {
            for &base in seq.as_ref() {
                // Fill window until size k
                if window.len() < k {
                    window.push(base);
                    if window.len() == k {
                        let kmer = Kmer::from_bases(&window).canonical();
                        out.push(kmer);
                    }
                } else {
                    // Slide: drop first, push new
                    window.remove(0); // O(k) → can be optimized
                    window.push(base);
                    let kmer = Kmer::from_bases(&window).canonical();
                    out.push(kmer);
                }
            }
        }
    }
}
