//! FASTG codec in the `SPAdes` / Bandage dialect.
//!
//! A record is a header `>NAME[']:SUCC1,SUCC2';` (the edge list is optional)
//! followed by sequence lines. `NAME'` is the reverse-complement strand of
//! `NAME`, and its record holds the reverse-complemented sequence.
//!
//! **Output** follows `SPAdes`: every node gets a forward and a `'` record, each
//! listing the successors of that strand, so every edge appears from both
//! ends. Names are wrapped as `EDGE_<name>_length_<len>_cov_1`, the only form
//! for which Bandage resolves edge targets.
//!
//! **Input** strips that wrapper back to `<name>`, reverse-complements `'`
//! records and merges mirrored edges (`A:B` and `B':A'`), so single- and
//! double-stranded files parse to the same graph.
//!
//! FASTG has no paths or overlaps; they are dropped on output.
//!
//! `reader` parses records line by line; `writer` formats both strands of
//! every node in parallel chunks.

mod reader;
mod writer;

#[cfg(test)]
mod tests;

use crate::core::graph_dto::CoreGraphDTO;
use crate::error::PanResult;
use crate::traits::{GraphParser, GraphSerializer};
use std::io::{Read, Seek, Write};

/// Codec for FASTG in the `SPAdes` / Bandage dialect.
pub struct FastgCodec;

impl<R: Read + Seek> GraphParser<R> for FastgCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        reader::parse(reader)
    }
}

impl GraphSerializer for FastgCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        writer::write(graph, writer)
    }
}
