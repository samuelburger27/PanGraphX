use crate::core::graph::CoreGraph;
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use gfa::parser::{GFAParser, GFAParserBuilder};
pub struct GFAFormat;

impl GraphParser for GFAFormat {
    fn parse<R: std::io::BufRead>(mut reader: R) -> PanResult<CoreGraph> {
        let lines: Vec<String> = reader.lines().collect::<Result<Vec<_>, _>>()?;
        let lines_iter = lines.iter().map(|s| s.as_bytes());
        // TODO in fututre maybe support optional fields
        let parser: GFAParser<usize, ()> = GFAParserBuilder::all().build();
        let gfa_graph = parser.parse_lines(lines_iter)?;
        
        unimplemented!("GFA parsing not yet implemented")
    }
}

// pub fn parse_gfa(content: &str) -> Result<GFAParser, gfa::error::GFAError> {
//     let parser = GFAParserBuilder::all().build();
//     parser.parse_file(path)
//     parser.parse_str(content)
// }
