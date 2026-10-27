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
use crate::error::PanResult;
use crate::formats::gfa_format::{GFA_HEADER, RECORDS_PER_CHUNK, orientation_byte, write_records};
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

impl DeBruijn {
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
