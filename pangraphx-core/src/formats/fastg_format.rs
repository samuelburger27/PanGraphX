use crate::core::graph::{CoreGraph, Edge, Node, NodeId, Orientation};
use crate::core::graph_utils::reverse_complement;
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, Write};

pub struct FastgCodec;

impl<R: Read + Seek> GraphParser<R> for FastgCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraph> {
        // TODO maybe parse name it often contains metadata ?

        let buf_reader = BufReader::new(reader);
        let seq_characters: [u8; 5] = [b'A', b'C', b'G', b'T', b'N'];
        let mut recorded_nodes: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut core_nodes: Vec<Node> = Vec::new();
        let mut core_edges = Vec::new();
        let mut node_id: NodeId = Vec::new();
        let mut orientation = Orientation::Forward;
        let mut node_sequence = Vec::new();
        for line in buf_reader.lines() {
            // FASTG contain only ASCII characters can safely read as bytes
            let line = line?.bytes().collect::<Vec<u8>>();
            if line.is_empty() {
                continue;
            }
            // Header line
            if line[0] == b'>' {
                // Record previous node if exists
                if !node_sequence.is_empty() {
                    record_node(
                        node_id,
                        node_sequence,
                        orientation,
                        &mut recorded_nodes,
                        &mut core_nodes,
                    )?;
                    node_sequence = Vec::new();
                }

                if line.last() != Some(&b';') {
                    return Err(PanGraphXError::Parse(
                        "Header isn't in valid format, header should end with ';' character"
                            .to_string(),
                    ));
                }

                let separator = line.iter().position(|b| *b == b':').unwrap_or(line.len());
                // Reverse orientation is indicated by a trailing '
                orientation = match line[separator - 1] {
                    b'\'' => Orientation::Reverse,
                    _ => Orientation::Forward,
                };
                let node_id_slice = &line[1..separator
                    - if orientation == Orientation::Reverse {
                        1
                    } else {
                        0
                    }];
                node_id = node_id_slice.to_vec();

                let sequence_end = line.len() - 1; // Exclude trailing ';'
                let edges = line[separator + 1..sequence_end].split(|&b| b == b',');
                for edge in edges {
                    if edge.is_empty() {
                        continue;
                    }

                    let mut edge_end = edge.len();
                    let mut edge_orientation = Orientation::Forward;
                    if edge.last() == Some(&b'\'') {
                        edge_orientation = Orientation::Reverse;
                        edge_end -= 1;
                    }

                    let edge_id = edge[..edge_end].to_vec();

                    core_edges.push(Edge {
                        from_node: node_id.clone(),
                        from_orient: orientation,
                        to_node: edge_id,
                        to_orient: edge_orientation,
                        overlap: Vec::new(), // FASTG does not specify overlaps
                    });
                }
            } else {
                // Validate sequence characters
                for b in &line {
                    if !seq_characters.contains(b) {
                        return Err(PanGraphXError::Parse(
                            "Invalid character in FASTG sequence".to_string(),
                        ));
                    }
                }
                node_sequence.extend_from_slice(&line);
            }
        }
        // Record previous node if exists
        if !node_sequence.is_empty() {
            if orientation == Orientation::Reverse {
                node_sequence = reverse_complement(&node_sequence);
            }
            record_node(
                node_id,
                node_sequence,
                orientation,
                &mut recorded_nodes,
                &mut core_nodes,
            )?;
        }

        // Validate that all edges reference existing nodes
        for edge in &core_edges {
            if !recorded_nodes.contains_key(&edge.from_node)
                || !recorded_nodes.contains_key(&edge.to_node)
            {
                return Err(PanGraphXError::Parse(format!(
                    "Edge references non-existent from_node: {}",
                    String::from_utf8_lossy(&edge.from_node)
                )));
            }
        }

        Ok(CoreGraph {
            nodes: core_nodes,
            edges: core_edges,
            paths: Vec::new(), // FASTG does not contain path information
        })
    }
}


/// Records a node into the core graph, checking for duplicates.
fn record_node(
    node_id: NodeId,
    mut node_sequence: Vec<u8>,
    orientation: Orientation,
    recorded_nodes: &mut HashMap<Vec<u8>, usize>,
    core_nodes: &mut Vec<Node>,
) -> PanResult<()> {
    if orientation == Orientation::Reverse {
        node_sequence = reverse_complement(&node_sequence);
    }

    if recorded_nodes.contains_key(&node_id) {
        if node_sequence != core_nodes[recorded_nodes[&node_id]].sequence {
            return Err(PanGraphXError::Parse(format!(
                "Duplicate node ID {} with different sequences found in FASTG file",
                String::from_utf8_lossy(&node_id)
            )));
        }
    } else {
        core_nodes.push(Node {
            id: node_id.clone(),
            sequence: node_sequence.clone(),
        });
        recorded_nodes.insert(node_id, core_nodes.len() - 1);
    }
    Ok(())
}

impl GraphSerializer for FastgCodec {
    fn serialize(&self, graph: &CoreGraph, writer: &mut dyn Write) -> PanResult<()> {
        println!("Warning: Fastg files don't support Paths.");
        println!("Only nodes and edges will be serialized.");
        let mut node_edge_map: HashMap<&NodeId, Vec<&Edge>> = HashMap::new();
        for edge in &graph.edges {
            node_edge_map
                .entry(&edge.from_node)
                .or_insert_with(Vec::new)
                .push(edge);
        }
        for node in &graph.nodes {
            let edges = node_edge_map.get(&node.id);
            let mut header = format!(">{}", String::from_utf8_lossy(&node.id));
            if let Some(edge_list) = edges {
                let edge_strs: Vec<String> = edge_list
                    .iter()
                    .map(|edge| {
                        let mut edge_id = String::from_utf8_lossy(&edge.to_node).to_string();
                        if edge.to_orient == Orientation::Reverse {
                            edge_id.push('\'');
                        }
                        edge_id
                    })
                    .collect();
                if !edge_strs.is_empty() {
                    header.push(':');
                    header.push_str(&edge_strs.join(","));
                }
            }
            header.push_str(";\n");
            writer.write_all(header.as_bytes())?;
            writer.write_all(&node.sequence)?;
            writer.write_all(b"\n")?;
        }
        Ok(())
    }
}
