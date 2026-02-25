use crate::core::core_types::{Edge, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use gfa::cigar::CIGAR;
use gfa::gfa::orientation::Orientation as GFAOrientation;
use gfa::parser::{GFAParser, GFAParserBuilder};
use log::debug;
use rayon::prelude::*;
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

        // Phase 1: Create nodes sequentially (maintaining order)
        let mut mapping = HashMap::new();
        let mut nodes = Nodes::new();
        for (i, node) in gfa.segments.iter().enumerate() {
            mapping.insert(i, node.name.clone());
            nodes.push(node.sequence.clone());
        }
        let mut reverse_mapping: HashMap<&[u8], usize> = HashMap::new();
        for (k, v) in &mapping {
            reverse_mapping.insert(v, *k);
        }

        // Phase 2: Parallelize edge creation from links
        let edges = gfa
            .links
            .into_par_iter()
            .map(|link| {
                PanResult::Ok(Edge {
                    from_node: *reverse_mapping.get(&link.from_segment[..]).ok_or_else(|| {
                        PanGraphXError::Parse("Failed to map from_segment to node ID".into())
                    })?,
                    from_orient: link.from_orient.into(),
                    to_node: *reverse_mapping.get(&link.to_segment[..]).ok_or_else(|| {
                        PanGraphXError::Parse("Failed to map to_segment to node ID".into())
                    })?,
                    to_orient: link.to_orient.into(),
                    overlap: parse_overlap(&link.overlap),
                })
            })
            .collect::<PanResult<Vec<Edge>>>()?;

        // Phase 3: Parallelize path creation from paths
        let paths: Vec<Path> = gfa
            .paths
            .into_par_iter()
            .map(|p| {
                let steps: Vec<Step> = p
                    .iter()
                    .map(|(id, orient)| {
                        let a: &[u8] = id.iter().as_slice();
                        let o = *reverse_mapping.get(a).ok_or_else(|| {
                            PanGraphXError::Parse("Failed to map segment ID to node ID".into())
                        })?;
                        PanResult::Ok(Step {
                            node_id: o,
                            orientation: orient.into(),
                        })
                    })
                    .collect::<PanResult<Vec<Step>>>()?;

                let overlaps: Vec<u32> = p
                    .overlaps
                    .into_iter()
                    .map(|opt_cigar| match opt_cigar {
                        Some(cigar) => cigar_to_overlap(cigar),
                        None => 0,
                    })
                    .collect();
                PanResult::Ok(Path {
                    name: p.path_name,
                    steps,
                    overlaps,
                })
            })
            .collect::<PanResult<Vec<Path>>>()?;

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

        // Parallelize node formatting
        let node_lines: Vec<String> = graph
            .nodes
            .par_iter()
            .map(|node| {
                let name = graph.get_node_name(node);
                let seq = String::from_utf8_lossy(&node.sequence);
                format!("S\t{}\t{}\n", name, seq)
            })
            .collect();

        // Write all node lines sequentially
        for line in node_lines {
            writer.write_all(line.as_bytes())?;
        }

        // Parallelize edge formatting
        let edge_lines: Vec<String> = graph
            .edges
            .par_iter()
            .map(|edge| {
                let from_name = graph.get_node_name(&graph.nodes[edge.from_node]);
                let to_name = graph.get_node_name(&graph.nodes[edge.to_node]);
                let overlap = edge.overlap.to_string() + "M";
                format!(
                    "L\t{}\t{}\t{}\t{}\t{}\n",
                    from_name, edge.from_orient, to_name, edge.to_orient, overlap
                )
            })
            .collect();

        // Write all edge lines sequentially
        for line in edge_lines {
            writer.write_all(line.as_bytes())?;
        }

        // Parallelize path formatting
        let path_lines: Vec<String> = graph
            .paths
            .par_iter()
            .map(|path| {
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
                if overlaps.is_empty() || overlaps.chars().all(|c| c == '0' || c == 'M' || c == ',')
                {
                    overlaps = "*".to_string();
                }
                let name = String::from_utf8_lossy(&path.name);
                format!("P\t{}\t{}\t{}\n", name, segments, overlaps)
            })
            .collect();

        // Write all path lines sequentially
        for line in path_lines {
            writer.write_all(line.as_bytes())?;
        }

        Ok(())
    }
}
