use crate::core::graph::{CoreGraphDTO, Edge, Node, NodeId, Orientation, Path, Step};
use crate::error::PanResult;
use crate::traits::{GraphParser, GraphSerializer};
use gfa::cigar::CIGAR;
use gfa::gfa::orientation::Orientation as GFAOrientation;
use gfa::parser::{GFAParser, GFAParserBuilder};
use log::debug;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, Write};

pub struct GFACodec;

impl From<GFAOrientation> for Orientation {
    fn from(orientation: GFAOrientation) -> Self {
        match orientation {
            GFAOrientation::Forward => Orientation::Forward,
            GFAOrientation::Backward => Orientation::Reverse,
        }
    }
}
/// Converts CIGAR to overlap length
/// For now ignore everything except matches
#[inline]
fn cigar_to_overlap(cigar: CIGAR) -> u32 {
    let mut result: u32 = 0;
    for pair in cigar.0 {
        let (count, op) = pair.into_pair();
        if op == gfa::cigar::CIGAROp::M {
            result += count;
        } else {
            debug!(
                "Non-match CIGAR op encountered: {:?} with count {}",
                op, count
            );
            debug!("Ignoring non-match operations for overlap calculation");
        }
    }
    result
}

// Can be made faster by avoiding parsing CIGAR completely
// For now, this seems sufficient
#[inline]
fn parse_overlap(cigar_str: &[u8]) -> u32 {
    match CIGAR::from_bytestring(cigar_str) {
        Some(cigar) => cigar_to_overlap(cigar),
        None => 0,
    }
}

/// Implementation of GraphParser for GFA format
/// Uses gfa crate to parse GFA files and converts to CoreGraph
impl<R: Read + Seek> GraphParser<R> for GFACodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        let buf_reader = BufReader::new(reader);
        let lines: Vec<String> = buf_reader.lines().collect::<Result<Vec<_>, _>>()?;
        let lines_iter = lines.iter().map(|s| s.as_bytes());
        // TODO in future maybe support optional fields
        let parser: GFAParser<Vec<u8>, ()> = GFAParserBuilder::all().build();
        let gfa = parser.parse_lines(lines_iter)?;
        let mut mapping = HashMap::new();
        let mut nodes = Vec::new();
        // TODO handle options fields properly
        for (i, node) in gfa.segments.iter().enumerate() {
            mapping.insert(i, node.name.clone());
            nodes.push(Node {
                id: i as NodeId,
                sequence: node.sequence.clone(),
            });
        }
        let mut reverse_mapping: HashMap<&[u8], usize> = HashMap::new();
        for (k, v) in &mapping {
            reverse_mapping.insert(v, *k);
        }
        for path in &gfa.paths {
            for cigar in &path.overlaps {
                if let Some(cigar) = cigar {
                    println!("{}", cigar);
                }
            }
        }
        let edges: Vec<Edge> = gfa
            .links
            .into_iter()
            .map(|link| Edge {
                from_node: *reverse_mapping.get(&link.from_segment[..]).unwrap(),
                from_orient: link.from_orient.into(),
                to_node: *reverse_mapping.get(&link.to_segment[..]).unwrap(),
                to_orient: link.to_orient.into(),
                overlap: parse_overlap(&link.overlap),
            })
            .collect();

        let paths: Vec<Path> = gfa
            .paths
            .into_iter()
            .map(|p| {
                let steps: Vec<Step> = p
                    .iter()
                    .map(|(id, orient)| {
                        let a: &[u8] = id.iter().as_slice();
                        let o = *reverse_mapping.get(a).unwrap();
                        Step {
                            node_id: o,
                            orientation: orient.into(),
                        }
                    })
                    .collect();

                let overlaps: Vec<u32> = p
                    .overlaps
                    .into_iter()
                    .map(|opt_cigar| match opt_cigar {
                        Some(cigar) => cigar_to_overlap(cigar),
                        None => 0,
                    })
                    .collect();
                Path {
                    name: p.path_name,
                    steps,
                    overlaps,
                }
            })
            .collect();

        Ok(CoreGraphDTO {
            nodes,
            edges,
            paths,
            node_name_map: Some(mapping),
        })
    }
}

impl GraphSerializer for GFACodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        // write header
        // TODO maybe include newer version
        writer.write_all(b"H\tVN:Z:1.0\n")?;

        // write nodes
        for node in &graph.nodes {
            let name = graph.get_node_name(node);
            let seq = String::from_utf8_lossy(&node.sequence);
            writer.write_all(format!("S\t{}\t{}\n", name, seq).as_bytes())?;
        }
        // Write edges
        for edge in &graph.edges {
            let from_name = graph.get_node_name(&graph.nodes[edge.from_node]);
            let to_name = graph.get_node_name(&graph.nodes[edge.to_node]);
            let overlap = edge.overlap.to_string() + "M";
            writer.write_all(
                format!(
                    "L\t{}\t{}\t{}\t{}\t{}\n",
                    from_name, edge.from_orient, to_name, edge.to_orient, overlap
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
                    let name: String = graph.get_name_from_id(step.node_id);
                    let orient = match step.orientation {
                        Orientation::Forward => "+",
                        Orientation::Reverse => "-",
                    };
                    format!("{}{}", name, orient)
                })
                .collect::<Vec<_>>()
                .join(",");
            let mut overlaps = path
                .overlaps
                .iter()
                .map(|opt| opt.to_string() + "M")
                .collect::<Vec<_>>()
                .join(",");
            if overlaps.is_empty() {
                overlaps = "*".to_string();
            }
            let name = String::from_utf8_lossy(&path.name);
            writer.write_all(format!("P\t{}\t{}\t{}\n", name, segments, overlaps).as_bytes())?;
        }

        Ok(())
    }
}
