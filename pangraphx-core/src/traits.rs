use crate::core::graph::CoreGraph;
use crate::error::PanResult;
use std::io::{BufRead, Write};
/// A trait for any structure that can parse a graph format from a stream.
pub trait GraphParser {
    /// Parses data from a reader into the `CoreGraph` representation.
    fn parse<R: BufRead>(reader: R) -> PanResult<CoreGraph>;
}

/// A trait for any structure that can serialize a `CoreGraph` to a stream.
pub trait GraphSerializer {
    /// Writes a `CoreGraph` to a writer in a specific format.
    fn serialize<W: Write>(graph: &CoreGraph, writer: W) -> PanResult<()>;
}
