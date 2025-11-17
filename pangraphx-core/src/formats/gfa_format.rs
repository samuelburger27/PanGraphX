use crate::core::graph::{CoreGraph, Edge, Node, NodeId, Orientation, Path, Step};
use crate::error::PanResult;
use crate::traits::{GraphParser, GraphSerializer};
use gfa::gfa::orientation::Orientation as GFAOrientation;
use gfa::parser::{GFAParser, GFAParserBuilder};
use std::io::{BufRead, BufReader, Read, Seek};

pub struct GFACodec;

impl From<GFAOrientation> for Orientation {
    fn from(orientation: GFAOrientation) -> Self {
        match orientation {
            GFAOrientation::Forward => Orientation::Forward,
            GFAOrientation::Backward => Orientation::Reverse,
        }
    }
}

/// Implementation of GraphParser for GFA format
/// Uses gfa crate to parse GFA files and converts to CoreGraph
impl<R: Read + Seek> GraphParser<R> for GFACodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraph> {
        let buf_reader = BufReader::new(reader);
        let lines: Vec<String> = buf_reader.lines().collect::<Result<Vec<_>, _>>()?;
        let lines_iter = lines.iter().map(|s| s.as_bytes());
        // TODO in future maybe support optional fields
        let parser: GFAParser<Vec<u8>, ()> = GFAParserBuilder::all().build();
        let gfa = parser.parse_lines(lines_iter)?;
        // TODO handle options fields properly
        let nodes: Vec<Node> = gfa
            .segments
            .into_iter()
            .map(|seq| Node {
                id: seq.name,
                sequence: seq.sequence,
            })
            .collect();
        let edges: Vec<Edge> = gfa
            .links
            .into_iter()
            .map(|link| Edge {
                from_node: link.from_segment as NodeId,
                from_orient: link.from_orient.into(),
                to_node: link.to_segment as NodeId,
                to_orient: link.to_orient.into(),
                overlap: link.overlap,
            })
            .collect();

        let paths: Vec<Path> = gfa
            .paths
            .into_iter()
            .map(|p| {
                let steps: Vec<Step> = p
                    .iter()
                    .map(|(id, orient)| Step {
                        node_id: id.to_vec(),
                        orientation: orient.into(),
                    })
                    .collect();
                Path {
                    name: p.path_name,
                    steps,
                    overlaps: p.overlaps,
                }
            })
            .collect();

        Ok(CoreGraph {
            nodes,
            edges,
            paths,
        })
    }
}

impl GraphSerializer for GFACodec {
    fn serialize(&self, graph: &CoreGraph, writer: &mut dyn std::io::Write) -> PanResult<()> {
        // write header
        // TODO maybe incolde newer version
        writer.write(b"H\tVN:Z:1.0\n")?;

        // write nodes
        for node in &graph.nodes {
            let id = String::from_utf8_lossy(&node.id);
            let seq = String::from_utf8_lossy(&node.sequence);
            writer.write(format!("S\t{}\t{}\n", id, seq).as_bytes())?;
        }
        // Write edges
        for edge in &graph.edges {
            let from_id = String::from_utf8_lossy(&edge.from_node);
            let to_id = String::from_utf8_lossy(&edge.to_node);
            let overlap = String::from_utf8_lossy(&edge.overlap);
            writer.write(
                format!(
                    "L\t{}\t{}\t{}\t{}\t{}\n",
                    from_id, edge.from_orient, to_id, edge.to_orient, overlap
                )
                .as_bytes(),
            )?;
        }
        // Write paths
        for path in &graph.paths {
            let segments = path
                .steps
                .iter()
                .map(|step| {
                    let id = String::from_utf8_lossy(&step.node_id);
                    let orient = match step.orientation {
                        crate::core::graph::Orientation::Forward => "+",
                        crate::core::graph::Orientation::Reverse => "-",
                    };
                    format!("{}{}", id, orient)
                })
                .collect::<Vec<_>>()
                .join(",");
            let overlaps = path
                .overlaps
                .iter()
                .map(|opt| match opt {
                    Some(cigar) => cigar
                        .0
                        .iter()
                        .map(|pair| pair.to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                    None => "*".to_string(),
                })
                .collect::<Vec<_>>()
                .join(",");
            let name = String::from_utf8_lossy(&path.name);
            writer.write(format!("P\t{}\t{}\t{}\n", name, segments, overlaps).as_bytes())?;
        }

        Ok(())
    }
}
