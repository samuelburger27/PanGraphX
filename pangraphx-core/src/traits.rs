use crate::core::graph_dto::CoreGraphDTO;
use crate::error::PanResult;
use std::io::{Read, Seek, Write};

/// A trait for any structure that can parse a graph format from a stream.
pub trait GraphParser<R: Read + Seek> {
    /// # Errors
    /// Returns an error if the reader contains invalid or unsupported data.
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO>;
}

/// A trait for any structure that can serialize a `CoreGraphDTO` to a stream.
pub trait GraphSerializer {
    /// Writes a `CoreGraphDTO` to a writer in a specific format. The graph is
    /// valid by construction, so serializers may index its nodes directly.
    ///
    /// # Errors
    /// Returns an error if writing to the writer fails.
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()>;
}
