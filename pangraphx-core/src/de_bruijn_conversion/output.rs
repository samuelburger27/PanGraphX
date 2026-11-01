//! Streaming output of de Bruijn graphs.
//!
//! A de Bruijn graph is written straight from its sorted tables instead of
//! being converted to a [`CoreGraphDTO`](crate::CoreGraphDTO) first. A k-mer
//! is one word in the node table but about 64 bytes as a DTO node (a `Node`
//! plus a heap-allocated sequence), so the conversion used to be the memory
//! peak of uncompacted output.
//!
//! The output is byte-identical to writing the DTO with
//! [`GFACodec`](crate::formats::GFACodec): node `i` is the k-mer of rank `i`
//! (named by its 0-based id), and every link has overlap `k - 1`.

use super::de_bruijn_graph::{DbgCore, DbgTables, DeBruijn, edge_overlap, with_core};
use super::word::{KmerWord, decode_word_into, edge_ends};
use crate::core::core_types::Sequence;
use crate::error::PanResult;
use crate::formats::common::chunked::{RECORDS_PER_CHUNK, write_records};
use crate::formats::gfa::{GFA_HEADER, orientation_byte};
use std::io::Write;

/// Appends the decimal form of `value` to `buf`.
#[inline]
pub fn push_number(buf: &mut Vec<u8>, value: impl itoa::Integer) {
    buf.extend_from_slice(itoa::Buffer::new().format(value).as_bytes());
}

impl<W: KmerWord> DbgCore<W> {
    /// Writes the GFA header, one `S` line per k-mer and one `L` line per
    /// edge, in table order.
    pub(crate) fn write_segments_and_links(
        &self,
        k: usize,
        writer: &mut dyn Write,
    ) -> PanResult<()> {
        writer.write_all(GFA_HEADER)?;
        write_records(writer, &self.kmers, RECORDS_PER_CHUNK, |id, &kmer, buf| {
            buf.extend_from_slice(b"S\t");
            push_number(buf, id);
            buf.push(b'\t');
            decode_word_into(kmer, k, buf);
            buf.push(b'\n');
            Ok(())
        })?;
        let overlap = edge_overlap(k)?;
        write_records(writer, &self.edges, RECORDS_PER_CHUNK, |_, &edge, buf| {
            let (from, to) = edge_ends(edge, k);
            buf.extend_from_slice(b"L\t");
            push_number(buf, self.id_of(from.canonical())?);
            buf.push(b'\t');
            buf.push(orientation_byte(from.orientation()));
            buf.push(b'\t');
            push_number(buf, self.id_of(to.canonical())?);
            buf.push(b'\t');
            buf.push(orientation_byte(to.orientation()));
            buf.push(b'\t');
            push_number(buf, overlap);
            buf.extend_from_slice(b"M\n");
            Ok(())
        })
    }
}

/// The sequences of the unitigs of a compacted de Bruijn graph, without the
/// links between them; written as FASTA.
pub struct UnitigSequences {
    sequences: Vec<Sequence>,
}

impl UnitigSequences {
    /// Number of unitigs.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.sequences.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.sequences.is_empty()
    }

    /// The unitig sequences, indexed by unitig id.
    #[must_use]
    pub fn sequences(&self) -> &[Sequence] {
        &self.sequences
    }

    /// Writes one FASTA record per unitig, named by its id (its node id in
    /// [`DeBruijn::into_compacted_dto`]), with the sequence on a single line
    /// as unitig tools such as BCALM2 and GGCAT write them.
    ///
    /// # Errors
    /// Returns an error if writing fails.
    pub fn write_fasta(&self, writer: &mut dyn Write) -> PanResult<()> {
        write_records(writer, &self.sequences, 1 << 10, |id, sequence, buf| {
            buf.push(b'>');
            push_number(buf, id);
            buf.push(b'\n');
            buf.extend_from_slice(sequence);
            buf.push(b'\n');
            Ok(())
        })
    }
}

impl DeBruijn {
    /// Compacts the graph into unitigs and keeps only their sequences: the
    /// links between unitigs and the k-mer placement are released.
    ///
    /// # Errors
    /// Returns an error if the graph is too large for 32-bit unitig ids.
    pub fn into_unitig_sequences(self) -> PanResult<UnitigSequences> {
        Ok(UnitigSequences {
            sequences: self.into_unitigs()?.sequences,
        })
    }

    /// Writes the graph as GFA, one segment per k-mer, straight from its
    /// tables.
    ///
    /// The output is the same as writing the [`CoreGraphDTO`](crate::CoreGraphDTO)
    /// of the graph as GFA, without holding that DTO in memory.
    ///
    /// # Errors
    /// Returns an error if writing fails.
    pub fn write_gfa(&self, writer: &mut dyn Write) -> PanResult<()> {
        with_core!(&self.tables, core => core.write_segments_and_links(self.k, writer))
    }
}

#[cfg(test)]
mod tests {
    use super::super::de_bruijn_graph::tests::graph;
    use crate::{CoreGraphDTO, DeBruijn};
    use std::fmt::Write as _;

    fn streamed(g: &CoreGraphDTO, k: usize, threads: usize) -> Vec<u8> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let mut out = Vec::new();
            DeBruijn::from_directed_graph(g.clone(), k)
                .unwrap()
                .write_gfa(&mut out)
                .unwrap();
            out
        })
    }

    #[test]
    fn streamed_gfa_is_independent_of_thread_count() {
        let seq: Vec<u8> = b"ACGTTGCAAGGCTTACGATCGGATTACA"
            .iter()
            .cycle()
            .take(400)
            .copied()
            .collect();
        let g = graph(&[&seq], &[], &[("p", &[(0, false)])]);
        for k in [5, 31, 33] {
            assert_eq!(streamed(&g, k, 1), streamed(&g, k, 8), "k={k}");
        }
    }

    #[test]
    fn fasta_unitigs_match_compacted_nodes() {
        let g = graph(
            &[b"TTGACCAGTAGGACATGGATTC", b"A", b"C", b"CATGGATTCAGGCT"],
            &[],
            &[
                ("h1", &[(0, false), (1, false), (3, false)]),
                ("h2", &[(0, false), (2, false), (3, false)]),
            ],
        );
        for k in [3, 5, 31] {
            let dbg = || DeBruijn::from_directed_graph(g.clone(), k).unwrap();
            let unitigs = dbg().into_unitig_sequences().unwrap();
            let mut fasta = Vec::new();
            unitigs.write_fasta(&mut fasta).unwrap();
            let compacted = dbg().into_compacted_dto().unwrap();
            assert_eq!(unitigs.len(), compacted.nodes().len());
            let mut expected = String::new();
            for node in compacted.nodes().iter() {
                writeln!(
                    expected,
                    ">{}\n{}",
                    node.id,
                    String::from_utf8_lossy(&node.sequence)
                )
                .unwrap();
            }
            assert_eq!(String::from_utf8(fasta).unwrap(), expected, "k={k}");
        }
    }

    #[test]
    fn streamed_gfa_names_nodes_by_rank() {
        // Nodes are named by their 0-based rank; links overlap by k - 1.
        let g = graph(&[b"GATTACA"], &[], &[("p", &[(0, false)])]);
        let text = String::from_utf8(streamed(&g, 3, 1)).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("H\tVN:Z:1.0"));
        let ids: Vec<&str> = lines
            .filter(|l| l.starts_with("S\t"))
            .map(|l| l.split('\t').nth(1).unwrap())
            .collect();
        let expected: Vec<String> = (0..ids.len()).map(|i| i.to_string()).collect();
        assert_eq!(ids, expected);
        assert!(
            text.lines()
                .filter(|l| l.starts_with("L\t"))
                .all(|l| l.ends_with("\t2M"))
        );
    }
}
