use crate::core::graph::CoreGraph;
use crate::error::PanResult;
use std::io::{BufRead, Write};

/// A trait for any structure that can parse a graph format from a stream.
pub trait GraphParser {
    /// Parses data from a reader into the `CoreGraph` representation.
    fn parse(&self, reader: &mut dyn BufRead) -> PanResult<CoreGraph>;
}

/// A trait for any structure that can serialize a `CoreGraph` to a stream.
pub trait GraphSerializer {
    // TODO maybe rewrite using BufWriter
    /// Writes a `CoreGraph` to a writer in a specific format.
    fn serialize(&self, graph: &CoreGraph, writer: &mut dyn Write) -> PanResult<()>;
}
