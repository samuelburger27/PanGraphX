use crate::core::core_types::{Edge, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::proto_gen::vg as vg_proto;
use crate::traits::{GraphParser, GraphSerializer};
use log::debug;
use prost::Message;
use std::collections::HashMap;
use std::io::{Read, Seek, Write};

pub struct VGCodec;

// ---------------------------------------------------------------------------
// Varint helpers (protobuf-style LEB128 encoding)
// ---------------------------------------------------------------------------

/// Reads a varint64 from a byte stream. Returns `None` at EOF.
fn read_varint<R: Read>(reader: &mut R) -> PanResult<Option<u64>> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    let mut buf = [0u8; 1];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                if shift == 0 {
                    return Ok(None); // clean EOF
                }
                return Err(PanGraphXError::Parse(
                    "Unexpected EOF while reading varint".to_string(),
                ));
            }
            Ok(_) => {
                let byte = buf[0];
                value |= ((byte & 0x7F) as u64) << shift;
                if byte & 0x80 == 0 {
                    return Ok(Some(value));
                }
                shift += 7;
                if shift >= 64 {
                    return Err(PanGraphXError::Parse("Varint too long".to_string()));
                }
            }
            Err(e) => return Err(PanGraphXError::Io(e)),
        }
    }
}

/// Writes a varint64 to a byte stream.
fn write_varint(writer: &mut dyn Write, mut value: u64) -> PanResult<()> {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        writer.write_all(&[byte])?;
        if value == 0 {
            break;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// VG framing protocol
// ---------------------------------------------------------------------------

/// Reads all `Graph` messages from a VG-framed protobuf stream.
///
/// The VG framing format consists of groups. Each group starts with a varint
/// count of messages, each message is length-prefixed with a varint. The first
/// message in a group may be a type tag string (e.g. `"VG"`).
fn read_vg_graphs<R: Read>(reader: &mut R) -> PanResult<Vec<vg_proto::Graph>> {
    let mut graphs = Vec::new();

    loop {
        // Read group count
        let count = match read_varint(reader)? {
            Some(c) => c as usize,
            None => break, // EOF
        };

        if count == 0 {
            continue;
        }

        // Read first message (may be type tag)
        let first_len = read_varint(reader)?.ok_or_else(|| {
            PanGraphXError::Parse("Unexpected EOF reading first message length".to_string())
        })? as usize;

        let mut first_buf = vec![0u8; first_len];
        reader.read_exact(&mut first_buf)?;

        // Determine if the first message is a type tag.
        // Type tags are short ASCII strings; Graph messages are protobuf-encoded.
        let (is_type_tag, tag_str) = detect_type_tag(&first_buf);

        let remaining_count;
        if is_type_tag {
            debug!("VG stream type tag: {:?}", tag_str);
            remaining_count = count - 1;
        } else {
            // First message is an actual Graph, decode it
            let graph = vg_proto::Graph::decode(first_buf.as_slice()).map_err(|e| {
                PanGraphXError::Parse(format!("Failed to decode VG Graph message: {}", e))
            })?;
            graphs.push(graph);
            remaining_count = count - 1;
        }

        // Read remaining messages in the group
        for _ in 0..remaining_count {
            let msg_len = read_varint(reader)?.ok_or_else(|| {
                PanGraphXError::Parse("Unexpected EOF reading message length".to_string())
            })? as usize;

            let mut msg_buf = vec![0u8; msg_len];
            reader.read_exact(&mut msg_buf)?;

            let graph = vg_proto::Graph::decode(msg_buf.as_slice()).map_err(|e| {
                PanGraphXError::Parse(format!("Failed to decode VG Graph message: {}", e))
            })?;
            graphs.push(graph);
        }
    }

    Ok(graphs)
}

/// Heuristic to determine if a message buffer is a type tag string.
/// Type tags are short (typically <= 10 bytes), all printable ASCII.
fn detect_type_tag(buf: &[u8]) -> (bool, String) {
    if buf.len() <= 16 && !buf.is_empty() && buf.iter().all(|&b| b.is_ascii_graphic()) {
        let s = String::from_utf8_lossy(buf).to_string();
        return (true, s);
    }
    (false, String::new())
}

/// Writes `Graph` messages to a VG-framed protobuf stream.
fn write_vg_graphs(writer: &mut dyn Write, graphs: &[vg_proto::Graph]) -> PanResult<()> {
    // Write one group: type tag "VG" + all graph messages
    let count = graphs.len() + 1; // +1 for type tag
    write_varint(writer, count as u64)?;

    // Write type tag
    let tag = b"VG";
    write_varint(writer, tag.len() as u64)?;
    writer.write_all(tag)?;

    // Write each graph message
    for graph in graphs {
        let encoded = graph.encode_to_vec();
        write_varint(writer, encoded.len() as u64)?;
        writer.write_all(&encoded)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Orientation mapping between VG proto and internal representation
// ---------------------------------------------------------------------------

/// VG Edge orientation convention:
/// - `from_start = false` means leaving from the end of the node → Forward
/// - `from_start = true` means leaving from the start of the node → Reverse
/// - `to_end = false` means arriving at the start of the node → Forward
/// - `to_end = true` means arriving at the end of the node → Reverse
fn vg_edge_to_orientations(from_start: bool, to_end: bool) -> (Orientation, Orientation) {
    let from_orient = if from_start {
        Orientation::Reverse
    } else {
        Orientation::Forward
    };
    let to_orient = if to_end {
        Orientation::Reverse
    } else {
        Orientation::Forward
    };
    (from_orient, to_orient)
}

fn orientations_to_vg_edge(from_orient: Orientation, to_orient: Orientation) -> (bool, bool) {
    let from_start = from_orient == Orientation::Reverse;
    let to_end = to_orient == Orientation::Reverse;
    (from_start, to_end)
}

// ---------------------------------------------------------------------------
// GraphParser implementation
// ---------------------------------------------------------------------------

impl<R: Read + Seek> GraphParser<R> for VGCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        let proto_graphs = read_vg_graphs(reader)?;

        // Collect all proto nodes, edges, paths from partial graphs
        let mut all_proto_nodes = Vec::new();
        let mut all_proto_edges = Vec::new();
        let mut all_proto_paths: HashMap<String, Vec<vg_proto::Mapping>> = HashMap::new();
        let mut path_order: Vec<String> = Vec::new();

        for graph in &proto_graphs {
            all_proto_nodes.extend(graph.node.iter().cloned());
            all_proto_edges.extend(graph.edge.iter().cloned());
            for path in &graph.path {
                let entry = all_proto_paths.entry(path.name.clone()).or_default();
                if !path_order.contains(&path.name) {
                    path_order.push(path.name.clone());
                }
                entry.extend(path.mapping.iter().cloned());
            }
        }

        // Build node ID mapping: VG i64 IDs → 0-based usize
        // Sort by original ID to maintain a deterministic ordering
        all_proto_nodes.sort_by_key(|n| n.id);
        all_proto_nodes.dedup_by_key(|n| n.id);

        let mut vg_id_to_internal: HashMap<i64, usize> = HashMap::new();
        let mut node_name_map: HashMap<usize, Vec<u8>> = HashMap::new();
        let mut nodes = Nodes::new();

        for (idx, proto_node) in all_proto_nodes.iter().enumerate() {
            vg_id_to_internal.insert(proto_node.id, idx);
            nodes.push(proto_node.sequence.as_bytes().to_vec());

            // Store original name or ID as the node name
            let name = if proto_node.name.is_empty() {
                proto_node.id.to_string().into_bytes()
            } else {
                proto_node.name.as_bytes().to_vec()
            };
            node_name_map.insert(idx, name);
        }

        // Convert edges
        let edges: Vec<Edge> = all_proto_edges
            .iter()
            .map(|pe| {
                let from_id = *vg_id_to_internal.get(&pe.from).ok_or_else(|| {
                    PanGraphXError::Parse(format!(
                        "Edge references unknown from-node ID {}",
                        pe.from
                    ))
                })?;
                let to_id = *vg_id_to_internal.get(&pe.to).ok_or_else(|| {
                    PanGraphXError::Parse(format!("Edge references unknown to-node ID {}", pe.to))
                })?;
                let (from_orient, to_orient) = vg_edge_to_orientations(pe.from_start, pe.to_end);
                Ok(Edge {
                    from_node: from_id,
                    from_orient,
                    to_node: to_id,
                    to_orient,
                    overlap: pe.overlap as u32,
                })
            })
            .collect::<PanResult<Vec<Edge>>>()?;

        // Convert paths
        let mut paths = Vec::new();
        for path_name in &path_order {
            let mappings = all_proto_paths.get(path_name).ok_or_else(|| {
                PanGraphXError::Parse(format!("Missing mappings for path {}", path_name))
            })?;

            let mut steps = Vec::with_capacity(mappings.len());
            for mapping in mappings {
                if let Some(pos) = &mapping.position {
                    let node_id = *vg_id_to_internal.get(&pos.node_id).ok_or_else(|| {
                        PanGraphXError::Parse(format!(
                            "Path mapping references unknown node ID {}",
                            pos.node_id
                        ))
                    })?;
                    let orientation = if pos.is_reverse {
                        Orientation::Reverse
                    } else {
                        Orientation::Forward
                    };
                    steps.push(Step {
                        node_id,
                        orientation,
                    });
                }
            }

            paths.push(Path {
                name: path_name.as_bytes().to_vec(),
                steps,
                overlaps: Vec::new(),
            });
        }

        Ok(CoreGraphDTO {
            nodes,
            edges,
            paths,
            node_name_map: Some(node_name_map),
        })
    }
}

// ---------------------------------------------------------------------------
// GraphSerializer implementation
// ---------------------------------------------------------------------------

impl GraphSerializer for VGCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        // Build reverse mapping: internal ID → original VG ID
        let internal_to_vg_id: HashMap<usize, i64> = if let Some(name_map) = &graph.node_name_map {
            name_map
                .iter()
                .map(|(&id, name)| {
                    let vg_id = String::from_utf8_lossy(name)
                        .parse::<i64>()
                        .unwrap_or((id + 1) as i64);
                    (id, vg_id)
                })
                .collect()
        } else {
            graph
                .nodes
                .iter()
                .map(|n| (n.id, (n.id + 1) as i64))
                .collect()
        };

        // Convert nodes
        let proto_nodes: Vec<vg_proto::Node> = graph
            .nodes
            .iter()
            .map(|node| {
                let vg_id = *internal_to_vg_id
                    .get(&node.id)
                    .unwrap_or(&((node.id + 1) as i64));
                let name = graph
                    .node_name_map
                    .as_ref()
                    .and_then(|m| m.get(&node.id))
                    .map(|n| String::from_utf8_lossy(n).to_string())
                    .unwrap_or_default();
                // Only set name if it differs from the ID string
                let name_field = if name == vg_id.to_string() {
                    String::new()
                } else {
                    name
                };
                vg_proto::Node {
                    sequence: String::from_utf8_lossy(&node.sequence).to_string(),
                    name: name_field,
                    id: vg_id,
                }
            })
            .collect();

        // Convert edges
        let proto_edges: Vec<vg_proto::Edge> = graph
            .edges
            .iter()
            .map(|edge| {
                let from = *internal_to_vg_id
                    .get(&edge.from_node)
                    .unwrap_or(&((edge.from_node + 1) as i64));
                let to = *internal_to_vg_id
                    .get(&edge.to_node)
                    .unwrap_or(&((edge.to_node + 1) as i64));
                let (from_start, to_end) =
                    orientations_to_vg_edge(edge.from_orient, edge.to_orient);
                vg_proto::Edge {
                    from,
                    to,
                    from_start,
                    to_end,
                    overlap: edge.overlap as i32,
                }
            })
            .collect();

        // Convert paths
        let proto_paths: Vec<vg_proto::Path> = graph
            .paths
            .iter()
            .enumerate()
            .map(|(_, path)| {
                let mappings: Vec<vg_proto::Mapping> = path
                    .steps
                    .iter()
                    .enumerate()
                    .map(|(rank, step)| {
                        let vg_node_id = *internal_to_vg_id
                            .get(&step.node_id)
                            .unwrap_or(&((step.node_id + 1) as i64));
                        let node_seq_len = graph
                            .nodes
                            .get(step.node_id)
                            .map(|n| n.sequence.len())
                            .unwrap_or(0);
                        vg_proto::Mapping {
                            position: Some(vg_proto::Position {
                                node_id: vg_node_id,
                                offset: 0,
                                is_reverse: step.orientation == Orientation::Reverse,
                                name: String::new(),
                            }),
                            edit: vec![vg_proto::Edit {
                                from_length: node_seq_len as i32,
                                to_length: node_seq_len as i32,
                                sequence: String::new(),
                            }],
                            rank: (rank + 1) as i64,
                        }
                    })
                    .collect();
                vg_proto::Path {
                    name: String::from_utf8_lossy(&path.name).to_string(),
                    mapping: mappings,
                    is_circular: false,
                    length: 0,
                }
            })
            .collect();

        let proto_graph = vg_proto::Graph {
            node: proto_nodes,
            edge: proto_edges,
            path: proto_paths,
        };

        write_vg_graphs(writer, &[proto_graph])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::core_types::{Edge, Nodes, Orientation, Path, Step};
    use crate::core::graph_dto::CoreGraphDTO;
    use std::io::Cursor;

    // ---- Varint tests ----

    #[test]
    fn test_varint_roundtrip() {
        let values: Vec<u64> = vec![0, 1, 127, 128, 255, 300, 16384, u32::MAX as u64, u64::MAX];
        for &val in &values {
            let mut buf = Vec::new();
            write_varint(&mut buf, val).unwrap();
            let mut cursor = Cursor::new(&buf);
            let decoded = read_varint(&mut cursor).unwrap().unwrap();
            assert_eq!(val, decoded, "Varint round-trip failed for {}", val);
        }
    }

    #[test]
    fn test_varint_eof_returns_none() {
        let buf: Vec<u8> = vec![];
        let mut cursor = Cursor::new(&buf);
        assert!(read_varint(&mut cursor).unwrap().is_none());
    }

    // ---- Framing tests ----

    #[test]
    fn test_framing_roundtrip() {
        let graph = vg_proto::Graph {
            node: vec![
                vg_proto::Node {
                    id: 1,
                    sequence: "ACGT".to_string(),
                    name: String::new(),
                },
                vg_proto::Node {
                    id: 2,
                    sequence: "TTGG".to_string(),
                    name: String::new(),
                },
            ],
            edge: vec![vg_proto::Edge {
                from: 1,
                to: 2,
                from_start: false,
                to_end: false,
                overlap: 0,
            }],
            path: vec![],
        };

        let mut buf = Vec::new();
        write_vg_graphs(&mut buf, &[graph.clone()]).unwrap();

        let mut cursor = Cursor::new(&buf);
        let decoded = read_vg_graphs(&mut cursor).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].node.len(), 2);
        assert_eq!(decoded[0].edge.len(), 1);
        assert_eq!(decoded[0].node[0].id, 1);
        assert_eq!(decoded[0].node[0].sequence, "ACGT");
    }

    #[test]
    fn test_framing_multiple_groups() {
        let g1 = vg_proto::Graph {
            node: vec![vg_proto::Node {
                id: 1,
                sequence: "AA".to_string(),
                name: String::new(),
            }],
            edge: vec![],
            path: vec![],
        };
        let g2 = vg_proto::Graph {
            node: vec![vg_proto::Node {
                id: 2,
                sequence: "CC".to_string(),
                name: String::new(),
            }],
            edge: vec![],
            path: vec![],
        };

        let mut buf = Vec::new();
        write_vg_graphs(&mut buf, &[g1]).unwrap();
        write_vg_graphs(&mut buf, &[g2]).unwrap();

        let mut cursor = Cursor::new(&buf);
        let decoded = read_vg_graphs(&mut cursor).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].node[0].sequence, "AA");
        assert_eq!(decoded[1].node[0].sequence, "CC");
    }

    // ---- Orientation mapping tests ----

    #[test]
    fn test_edge_orientation_mapping() {
        // Default: from_start=false, to_end=false → Forward, Forward
        assert_eq!(
            vg_edge_to_orientations(false, false),
            (Orientation::Forward, Orientation::Forward)
        );
        // from_start=true, to_end=false → Reverse, Forward
        assert_eq!(
            vg_edge_to_orientations(true, false),
            (Orientation::Reverse, Orientation::Forward)
        );
        // from_start=false, to_end=true → Forward, Reverse
        assert_eq!(
            vg_edge_to_orientations(false, true),
            (Orientation::Forward, Orientation::Reverse)
        );
        // from_start=true, to_end=true → Reverse, Reverse
        assert_eq!(
            vg_edge_to_orientations(true, true),
            (Orientation::Reverse, Orientation::Reverse)
        );
    }

    #[test]
    fn test_orientation_roundtrip() {
        let cases = [
            (Orientation::Forward, Orientation::Forward),
            (Orientation::Forward, Orientation::Reverse),
            (Orientation::Reverse, Orientation::Forward),
            (Orientation::Reverse, Orientation::Reverse),
        ];
        for (from_o, to_o) in cases {
            let (fs, te) = orientations_to_vg_edge(from_o, to_o);
            let (from_o2, to_o2) = vg_edge_to_orientations(fs, te);
            assert_eq!(from_o, from_o2);
            assert_eq!(to_o, to_o2);
        }
    }

    // ---- Parse / Serialize round-trip tests ----

    fn make_simple_graph() -> CoreGraphDTO {
        let nodes = Nodes::from_seq(vec![b"ACGT".to_vec(), b"TTGG".to_vec(), b"CCAA".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());
        name_map.insert(2, b"3".to_vec());

        CoreGraphDTO {
            nodes,
            edges: vec![
                Edge {
                    from_node: 0,
                    from_orient: Orientation::Forward,
                    to_node: 1,
                    to_orient: Orientation::Forward,
                    overlap: 0,
                },
                Edge {
                    from_node: 1,
                    from_orient: Orientation::Forward,
                    to_node: 2,
                    to_orient: Orientation::Forward,
                    overlap: 2,
                },
            ],
            paths: vec![Path {
                name: b"path1".to_vec(),
                steps: vec![
                    Step {
                        node_id: 0,
                        orientation: Orientation::Forward,
                    },
                    Step {
                        node_id: 1,
                        orientation: Orientation::Forward,
                    },
                    Step {
                        node_id: 2,
                        orientation: Orientation::Reverse,
                    },
                ],
                overlaps: vec![],
            }],
            node_name_map: Some(name_map),
        }
    }

    #[test]
    fn test_serialize_then_parse_roundtrip() {
        let original = make_simple_graph();
        let codec = VGCodec;

        // Serialize
        let mut buf = Vec::new();
        codec.serialize(&original, &mut buf).unwrap();
        assert!(!buf.is_empty(), "Serialized VG should not be empty");

        // Parse back
        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        // Verify nodes
        assert_eq!(parsed.nodes.len(), original.nodes.len());
        for (i, node) in parsed.nodes.iter().enumerate() {
            assert_eq!(
                node.sequence, original.nodes[i].sequence,
                "Node {} sequence mismatch",
                i
            );
        }

        // Verify edges
        assert_eq!(parsed.edges.len(), original.edges.len());
        for (i, edge) in parsed.edges.iter().enumerate() {
            assert_eq!(edge.from_node, original.edges[i].from_node);
            assert_eq!(edge.to_node, original.edges[i].to_node);
            assert_eq!(edge.from_orient, original.edges[i].from_orient);
            assert_eq!(edge.to_orient, original.edges[i].to_orient);
            assert_eq!(edge.overlap, original.edges[i].overlap);
        }

        // Verify paths
        assert_eq!(parsed.paths.len(), original.paths.len());
        assert_eq!(parsed.paths[0].name, original.paths[0].name);
        assert_eq!(parsed.paths[0].steps.len(), original.paths[0].steps.len());
        for (i, step) in parsed.paths[0].steps.iter().enumerate() {
            assert_eq!(step.node_id, original.paths[0].steps[i].node_id);
            assert_eq!(step.orientation, original.paths[0].steps[i].orientation);
        }
    }

    #[test]
    fn test_empty_graph_roundtrip() {
        let graph = CoreGraphDTO::default();
        let codec = VGCodec;

        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.nodes.len(), 0);
        assert_eq!(parsed.edges.len(), 0);
        assert_eq!(parsed.paths.len(), 0);
    }

    #[test]
    fn test_single_node_no_edges() {
        let nodes = Nodes::from_seq(vec![b"GATTACA".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"42".to_vec());

        let graph = CoreGraphDTO {
            nodes,
            edges: vec![],
            paths: vec![],
            node_name_map: Some(name_map),
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].sequence, b"GATTACA");
        assert_eq!(parsed.edges.len(), 0);
    }

    #[test]
    fn test_all_orientation_combinations() {
        let nodes = Nodes::from_seq(vec![b"AA".to_vec(), b"CC".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());

        let orientations = [
            (Orientation::Forward, Orientation::Forward),
            (Orientation::Forward, Orientation::Reverse),
            (Orientation::Reverse, Orientation::Forward),
            (Orientation::Reverse, Orientation::Reverse),
        ];

        for (from_o, to_o) in orientations {
            let graph = CoreGraphDTO {
                nodes: nodes.clone(),
                edges: vec![Edge {
                    from_node: 0,
                    from_orient: from_o,
                    to_node: 1,
                    to_orient: to_o,
                    overlap: 0,
                }],
                paths: vec![],
                node_name_map: Some(name_map.clone()),
            };

            let codec = VGCodec;
            let mut buf = Vec::new();
            codec.serialize(&graph, &mut buf).unwrap();

            let mut cursor = Cursor::new(&buf);
            let parsed = codec.parse(&mut cursor).unwrap();

            assert_eq!(parsed.edges.len(), 1);
            assert_eq!(
                parsed.edges[0].from_orient, from_o,
                "from_orient mismatch for {:?} → {:?}",
                from_o, to_o
            );
            assert_eq!(
                parsed.edges[0].to_orient, to_o,
                "to_orient mismatch for {:?} → {:?}",
                from_o, to_o
            );
        }
    }

    #[test]
    fn test_path_with_reverse_steps() {
        let nodes = Nodes::from_seq(vec![b"AAA".to_vec(), b"CCC".to_vec(), b"GGG".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());
        name_map.insert(2, b"3".to_vec());

        let graph = CoreGraphDTO {
            nodes,
            edges: vec![],
            paths: vec![Path {
                name: b"haplotype_1".to_vec(),
                steps: vec![
                    Step {
                        node_id: 0,
                        orientation: Orientation::Forward,
                    },
                    Step {
                        node_id: 1,
                        orientation: Orientation::Reverse,
                    },
                    Step {
                        node_id: 2,
                        orientation: Orientation::Forward,
                    },
                ],
                overlaps: vec![],
            }],
            node_name_map: Some(name_map),
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.paths.len(), 1);
        assert_eq!(parsed.paths[0].steps[0].orientation, Orientation::Forward);
        assert_eq!(parsed.paths[0].steps[1].orientation, Orientation::Reverse);
        assert_eq!(parsed.paths[0].steps[2].orientation, Orientation::Forward);
    }

    #[test]
    fn test_graph_without_name_map() {
        let nodes = Nodes::from_seq(vec![b"AT".to_vec(), b"GC".to_vec()]);
        let graph = CoreGraphDTO {
            nodes,
            edges: vec![Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Forward,
                overlap: 0,
            }],
            paths: vec![],
            node_name_map: None,
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.nodes.len(), 2);
        assert_eq!(parsed.nodes[0].sequence, b"AT");
        assert_eq!(parsed.nodes[1].sequence, b"GC");
        assert_eq!(parsed.edges.len(), 1);
    }

    #[test]
    fn test_multiple_paths() {
        let nodes = Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec(), b"G".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());
        name_map.insert(2, b"3".to_vec());

        let graph = CoreGraphDTO {
            nodes,
            edges: vec![],
            paths: vec![
                Path {
                    name: b"ref".to_vec(),
                    steps: vec![
                        Step {
                            node_id: 0,
                            orientation: Orientation::Forward,
                        },
                        Step {
                            node_id: 1,
                            orientation: Orientation::Forward,
                        },
                    ],
                    overlaps: vec![],
                },
                Path {
                    name: b"alt".to_vec(),
                    steps: vec![
                        Step {
                            node_id: 0,
                            orientation: Orientation::Forward,
                        },
                        Step {
                            node_id: 2,
                            orientation: Orientation::Forward,
                        },
                    ],
                    overlaps: vec![],
                },
            ],
            node_name_map: Some(name_map),
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.paths.len(), 2);
        assert_eq!(parsed.paths[0].name, b"ref");
        assert_eq!(parsed.paths[1].name, b"alt");
        assert_eq!(parsed.paths[0].steps.len(), 2);
        assert_eq!(parsed.paths[1].steps.len(), 2);
    }

    #[test]
    fn test_edge_overlap_preserved() {
        let nodes = Nodes::from_seq(vec![b"ACGT".to_vec(), b"GTAA".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());

        let graph = CoreGraphDTO {
            nodes,
            edges: vec![Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Forward,
                overlap: 2,
            }],
            paths: vec![],
            node_name_map: Some(name_map),
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.edges[0].overlap, 2);
    }
}
