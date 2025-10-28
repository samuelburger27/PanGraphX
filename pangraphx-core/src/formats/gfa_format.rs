use crate::core::graph::CoreGraph;
use crate::error::PanResult;
use crate::traits::GraphParser;
use gfa::parser::{GFAParser, GFAParserBuilder};
use std::io::BufRead;

pub struct GFASerialization;

/// Implementation of GraphParser for GFA format
/// Uses gfa crate to parse GFA files and converts to CoreGraph
impl GraphParser for GFASerialization {
    fn parse(&self, reader: &mut dyn BufRead) -> PanResult<CoreGraph> {
        let lines: Vec<String> = reader.lines().collect::<Result<Vec<_>, _>>()?;
        let lines_iter = lines.iter().map(|s| s.as_bytes());
        // TODO in future maybe support optional fields
        let parser: GFAParser<usize, ()> = GFAParserBuilder::all().build();
        let gfa_graph = parser.parse_lines(lines_iter)?;
        Ok(CoreGraph::from(gfa_graph))
    }
}

