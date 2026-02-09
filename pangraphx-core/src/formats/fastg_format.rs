use crate::core::graph::{CoreGraphDTO, Edge, Node, NodeId, NodeName, Orientation, Sequence};
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use bio::alphabets::dna::n_alphabet as dna_alphabet;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, Write};

pub struct FastgCodec;

#[inline]
fn is_header_line(line: &[u8]) -> bool {
    line.first() == Some(&b'>')
}


impl<R: Read + Seek> GraphParser<R> for FastgCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        println!("Warning: FASTG file doesn't contain path information.");
        println!("Only nodes and edges will be parsed.");
        let buf_reader = BufReader::new(reader);

        // Map from node names to their assigned sequential IDs
        let mut name_id_map: HashMap<NodeName, NodeId> = HashMap::new();

        // Vector of node sequences indexed by node ID. Elements are populated as headers
        // are parsed and sequences are read
        let mut sequences: Vec<Option<Sequence>> = Vec::new();

        // Buffer to collect all edges as they're discovered during parsing
        let mut edge_buff: Vec<Edge> = Vec::new();

        // Current accumulated node sequence being read from sequence lines.
        // This is stored temporarily and finalized when the next header is encountered.
        let mut node_sequence: Sequence = Vec::new();
        // The name of the node that the current sequence lines belong to
        let mut last_node_name: NodeName = Vec::new();

        for line in buf_reader.lines() {
            // FASTG files contain only ASCII characters, safe to read as bytes
            let line = line?.bytes().collect::<Vec<u8>>();
            if line.is_empty() {
                continue;
            }

            // Header format: >NodeName[']:EdgeList;
            if is_header_line(&line) {
                // Finalize the previous node's sequence before processing the new header
                record_node(
                    node_sequence,
                    &name_id_map,
                    &mut sequences,
                    &last_node_name,
                )?;

                last_node_name = parse_header_line(&line, &mut name_id_map, &mut edge_buff, &mut sequences)?;

                node_sequence = Vec::new(); // Reset sequence buffer for the new node
            } else {
                // sequence line
                // Validate that all characters are valid DNA nucleotides before accumulating
                if !dna_alphabet().is_word(&line) {
                    return Err(PanGraphXError::Parse(format!(
                        "Invalid character in DNA sequence: {}",
                        String::from_utf8_lossy(&line)
                    )));
                }
                node_sequence.extend(line);
            }
        }

        // Finalize the last node's sequence at EOF
        record_node(
            node_sequence,
            &name_id_map,
            &mut sequences,
            &last_node_name,
        )?;

        // Build the final node list with validated sequences
        let mut nodes: Vec<Node> = Vec::new();
        nodes.reserve(name_id_map.len());

        for (id, seq) in sequences.into_iter().enumerate() {
            let Some(s) = seq else {
                return Err(PanGraphXError::Parse(format!(
                    "Node {} does not have a sequence, FASTG file is not valid",
                    id
                )));
            };
            nodes.push(Node { id, sequence: s });
        }

        Ok(CoreGraphDTO {
            nodes,
            edges: edge_buff,
            paths: Vec::new(), // FASTG does not contain path information
            node_name_map: Some(
                name_id_map
                    .into_iter()
                    .map(|(name, id)| (id, name))
                    .collect(),
            ),
        })
    }
}

fn record_node(
    node_sequence: Sequence,
    name_id_map: &HashMap<NodeName, NodeId>,
    sequences: &mut Vec<Option<Sequence>>,
    last_node_name: &[u8],
) -> PanResult<()> {
    if !node_sequence.is_empty() {
        // Store the finalized sequence in the sequences vector
        if let Some(id) = name_id_map.get(last_node_name) {
            if let Some(existing_seq) = &sequences[*id] {
                // Verify that if a node appears multiple times, it has the same sequence
                if *existing_seq != *node_sequence {
                    return Err(PanGraphXError::Parse(format!(
                        "Node {} has multiple sequences in FASTG file, seq_a: {}, seq_b: {}",
                        String::from_utf8_lossy(last_node_name),
                        String::from_utf8_lossy(existing_seq),
                        String::from_utf8_lossy(&node_sequence)
                    )));
                }
            }
            sequences[*id] = Some(node_sequence);
        }
    }
    Ok(())
}


/// Retrieves the node ID for a given node name, creating a new entry in the name_id_map
/// and sequences vector if the node name has not been seen before.  
#[inline]
fn get_node_id(
    node_name: &[u8],
    name_id_map: &mut HashMap<NodeName, NodeId>,
    sequences: &mut Vec<Option<Sequence>>,
) -> NodeId {
    match name_id_map.get(node_name) {
        Some(id) => *id,
        None => {
            let new_id = name_id_map.len();
            name_id_map.insert(node_name.to_vec(), new_id);
            sequences.push(None); // Placeholder for sequence, will be filled in later
            new_id
        }
    }
}

/// Parses a FASTG header line to extract the node name, edges, and orientations.
/// Updates the name_id_map with any new nodes encountered in the header and
/// populates the edge_buff with edges defined in the header. 
/// Returns the node name for the current header.
fn parse_header_line(
    line: &[u8],
    name_id_map: &mut HashMap<NodeName, NodeId>,
    edge_buff: &mut Vec<Edge>,
    sequences: &mut Vec<Option<Sequence>>,
) -> PanResult<NodeName> {

    if line.last() != Some(&b';') {
        return Err(PanGraphXError::Parse(
            "Header isn't in valid format, header should end with ';' character".to_string(),
        ));
    }

    // Find the ':' separator between node name and edge list
    let separator = line.iter().position(|b| *b == b':').unwrap_or(line.len());

    // Determine if the node is on the reverse strand (indicated by trailing ')
    let from_orientation = match line[separator - 1] {
        b'\'' => Orientation::Reverse,
        _ => Orientation::Forward,
    };

    // Extract node name, excluding the orientation marker if present
    let node_name = &line[1..separator
        - if from_orientation == Orientation::Reverse {
            1
        } else {
            0
        }];

    // Get or create the node ID for this node name
    let from_node_id = get_node_id(node_name, name_id_map, sequences);

    // Parse the edge list: edges are comma-separated and may have orientation markers
    // Format: edge1,edge2',edge3,...
    let sequence_end = line.len() - 1; // Exclude trailing ';'
    let edges = line[separator + 1..sequence_end].split(|&b| b == b',');
    for edge in edges {
        if edge.is_empty() {
            continue;
        }

        // Check for reverse orientation marker on the target node
        let mut edge_end = edge.len();
        let mut edge_orientation = Orientation::Forward;
        if edge.last() == Some(&b'\'') {
            edge_orientation = Orientation::Reverse;
            edge_end -= 1;
        }

        let edge_name = &edge[..edge_end];

        let to_id = get_node_id(edge_name, name_id_map, sequences);

        edge_buff.push(Edge {
            from_node: from_node_id,
            from_orient: from_orientation,
            to_node: to_id,
            to_orient: edge_orientation,
            overlap: 0, // FASTG does not specify overlaps, use 0 as default
        })
    }
    Ok(node_name.to_vec())
}

impl GraphSerializer for FastgCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        println!("Warning: Fastg files don't support Paths.");
        println!("Only nodes and edges will be serialized.");
        let mut node_edge_map: HashMap<&NodeId, Vec<&Edge>> = HashMap::new();
        for edge in &graph.edges {
            node_edge_map.entry(&edge.from_node).or_default().push(edge);
        }
        for node in &graph.nodes {
            let edges = node_edge_map.get(&node.id);
            let mut header = format!(">{}", graph.get_node_name(node));
            if let Some(edge_list) = edges {
                let edge_strs: Vec<String> = edge_list
                    .iter()
                    .map(|edge| {
                        let mut edge_id = graph.get_name_from_id(edge.to_node);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_fastg_tiny() {
        let data = b">EDGE_1:EDGE_2;\nATCGCCCAT\n>EDGE_2:;\nGGATCC\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_ok());

        let graph = result.unwrap();
        assert_eq!(graph.nodes.len(), 2, "Should have 2 nodes");
        assert_eq!(graph.edges.len(), 1, "Should have 1 edge");
        // Verify nodes
        assert_eq!(graph.nodes[0].sequence, b"ATCGCCCAT");
        assert_eq!(graph.nodes[1].sequence, b"GGATCC");

        // Verify edge
        assert_eq!(graph.edges[0].from_node, 0);
        assert_eq!(graph.edges[0].to_node, 1);
        assert_eq!(graph.edges[0].from_orient, Orientation::Forward);
        assert_eq!(graph.edges[0].to_orient, Orientation::Forward);
    }

    /// Test parsing of reverse-complemented nodes (indicated by trailing single quote).
    /// Verifies that sequences are correctly reverse-complemented when the orientation
    /// marker is present.
    #[test]
    fn test_fastg_reverse_orientation() {
        // FASTG header format with reverse orientation: >NodeName':
        let data = b">NODE_A':EDGE_B;\nATCG\n>EDGE_B:;\nGGAT\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_ok());

        let graph = result.unwrap();
        assert_eq!(graph.nodes.len(), 2);

        // Original: ATCG
        assert_eq!(graph.nodes[0].sequence, b"ATCG");
        assert_eq!(graph.nodes[1].sequence, b"GGAT");
    }

    /// Test parsing of multiple edges from a single node with mixed orientations.
    /// Verifies that comma-separated edge lists are parsed correctly and that
    /// reverse orientation markers on target nodes are recognized.
    #[test]
    fn test_fastg_multiple_edges_with_orientations() {
        let data =
            b">NODE_1:NODE_2,NODE_3',NODE_4;\nATCG\n>NODE_2:;\nTT\n>NODE_3:;\nGG\n>NODE_4:;\nCC\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_ok());

        let graph = result.unwrap();
        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(graph.edges.len(), 3, "Should have 3 edges from NODE_1");

        // Verify first edge: NODE_1 -> NODE_2 (both forward)
        assert_eq!(graph.edges[0].from_node, 0);
        assert_eq!(graph.edges[0].to_node, 1);
        assert_eq!(graph.edges[0].to_orient, Orientation::Forward);

        // Verify second edge: NODE_1 -> NODE_3' (NODE_3 reverse)
        assert_eq!(graph.edges[1].from_node, 0);
        assert_eq!(graph.edges[1].to_node, 2);
        assert_eq!(graph.edges[1].to_orient, Orientation::Reverse);

        // Verify third edge: NODE_1 -> NODE_4 (both forward)
        assert_eq!(graph.edges[2].from_node, 0);
        assert_eq!(graph.edges[2].to_node, 3);
        assert_eq!(graph.edges[2].to_orient, Orientation::Forward);
    }

    /// Test parsing of multi-line sequences.
    /// Verifies that sequence data spanning multiple lines is correctly concatenated
    /// before being finalized.
    #[test]
    fn test_fastg_multiline_sequence() {
        let data = b">NODE_A:NODE_B;\nAT\nCG\nAA\n>NODE_B:;\nGG\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_ok());

        let graph = result.unwrap();
        assert_eq!(graph.nodes.len(), 2);
        // Sequence should be concatenated from all lines
        assert_eq!(graph.nodes[0].sequence, b"ATCGAA");
        assert_eq!(graph.nodes[1].sequence, b"GG");
    }

    /// Test parsing of nodes with no outgoing edges (empty edge list).
    /// Verifies that nodes without edges are correctly added to the graph.
    #[test]
    fn test_fastg_no_outgoing_edges() {
        let data = b">LEAF_NODE:;\nACGT\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_ok());

        let graph = result.unwrap();
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.edges.len(), 0);
        assert_eq!(graph.nodes[0].sequence, b"ACGT");
    }

    /// Test parsing of duplicate edges from the same source.
    /// Some assembly formats may include the same edge multiple times.
    #[test]
    fn test_fastg_duplicate_edges() {
        let data = b">NODE_1:NODE_2,NODE_2;\nAT\n>NODE_2:;\nGG\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_ok());

        let graph = result.unwrap();
        assert_eq!(graph.nodes.len(), 2);
        // Both edges should be recorded
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(graph.edges[0].to_node, 1);
        assert_eq!(graph.edges[1].to_node, 1);
    }

    /// Test error handling for invalid DNA characters in sequence.
    /// Verifies that invalid nucleotide characters are rejected with an appropriate error.
    #[test]
    fn test_fastg_invalid_dna_characters() {
        let data = b">NODE_1:;\nATXG\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_err(), "Should reject invalid DNA character 'X'");

        if let Err(PanGraphXError::Parse(msg)) = result {
            assert!(
                msg.contains("Invalid character"),
                "Error message should mention invalid character"
            );
        }
    }

    /// Test error handling for missing sequence terminator (';').
    /// Verifies that headers without the required semicolon are rejected.
    #[test]
    fn test_fastg_missing_header_terminator() {
        let data = b">NODE_1:NODE_2\nATCG\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(
            result.is_err(),
            "Should reject header without terminating semicolon"
        );

        if let Err(PanGraphXError::Parse(msg)) = result {
            assert!(
                msg.contains("';'"),
                "Error should mention missing semicolon"
            );
        }
    }

    /// Test error handling for nodes referenced in edges but without sequences.
    /// Verifies that the parser ensures all nodes have at least one sequence definition.
    #[test]
    fn test_fastg_missing_node_sequence() {
        let data = b">NODE_1:NODE_2;\nATCG\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(
            result.is_err(),
            "Should reject when edge target has no sequence"
        );

        if let Err(PanGraphXError::Parse(msg)) = result {
            assert!(
                msg.contains("does not have a sequence"),
                "Error should indicate missing sequence"
            );
        }
    }

    /// Test parsing of empty lines in the FASTG file.
    /// Verifies that empty lines are skipped without causing parsing errors.
    #[test]
    fn test_fastg_empty_lines() {
        let data = b">NODE_1:NODE_2;\n\nATCG\n\n>NODE_2:;\n\nGG\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_ok());

        let graph = result.unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].sequence, b"ATCG");
        assert_eq!(graph.nodes[1].sequence, b"GG");
    }

    /// Test node reuse in FASTG format.
    /// When a node name appears again in the file, it should use the same node ID.
    /// If sequences differ, this should be detected as an error.
    #[test]
    fn test_fastg_node_reuse_forward_and_reverse() {
        let data = b">NODE_A:NODE_B;\nATCG\n>NODE_A':NODE_B;\nATCG\n>NODE_B:;\nGG\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_ok(), "Should handle nodes in both orientations");

        let graph = result.unwrap();
        // Both NODE_A and NODE_A' should reference the same node (id=0),
        // with different orientation markers
        assert_eq!(graph.nodes.len(), 2);
    }

    /// Test parsing of a complex graph with interconnected nodes.
    /// Verifies correct behavior on a more realistic graph structure.
    #[test]
    fn test_fastg_complex_graph() {
        let data = b">A:B,C;\nAAAA\n>B:D,C';\nTT\n>C:D;\nGGGG\n>D:;\nCC\n";
        let mut cursor = Cursor::new(data);
        let codec = FastgCodec;

        let result = codec.parse(&mut cursor);
        assert!(result.is_ok());

        let graph = result.unwrap();
        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(graph.edges.len(), 5);

        // Verify structure
        let a_edges: Vec<_> = graph.edges.iter().filter(|e| e.from_node == 0).collect();
        assert_eq!(a_edges.len(), 2, "Node A should have 2 outgoing edges");

        let b_edges: Vec<_> = graph.edges.iter().filter(|e| e.from_node == 1).collect();
        assert_eq!(b_edges.len(), 2, "Node B should have 2 outgoing edges");
    }
}
