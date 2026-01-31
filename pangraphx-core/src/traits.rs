use crate::core::graph::CoreGraphDTO;
use crate::error::PanResult;
use std::io::{Read, Seek, Write};

/// A trait for any structure that can parse a graph format from a stream.
pub trait GraphParser<R: Read + Seek> {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO>;
}

/// A trait for any structure that can serialize a `CoreGraph` to a stream.
pub trait GraphSerializer {
    // TODO maybe rewrite using BufWriter
    /// Writes a `CoreGraph` to a writer in a specific format.
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()>;
}
