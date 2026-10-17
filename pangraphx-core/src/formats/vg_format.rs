use crate::core::core_types::{Edge, Node, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::id_map::IdMap;
use crate::proto_gen::vg as vg_proto;
use crate::traits::{GraphParser, GraphSerializer};
use log::debug;
use prost::Message;
use rayon::prelude::*;
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
                value |= u64::from(byte & 0x7F) << shift;
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

/// Maximum number of nodes, edges or path mappings stored in one `Graph` message.
/// Matches vg's default chunk size and keeps every message far below protobuf's 2 GB limit.
const VG_CHUNK_SIZE: usize = 1000;

/// Maximum number of messages per framing group (libvgio uses the same limit).
const VG_GROUP_SIZE: usize = 1000;

/// Largest message length accepted when reading: protobuf messages are limited
/// to 2 GB, so a larger length means a corrupt stream, not a huge allocation.
const VG_MAX_MESSAGE_LEN: u64 = i32::MAX as u64;

/// Reads a VG-framed protobuf stream and hands every decoded `Graph` message to `on_graph`.
///
/// The VG framing format consists of groups. Each group starts with a varint
/// count of messages, each message is length-prefixed with a varint. The first
/// message in a group may be a type tag string (e.g. `"VG"`).
///
/// Messages are decoded one at a time, so only a single chunk of the stream is
/// held in its protobuf form.
#[allow(clippy::cast_possible_truncation)]
fn for_each_vg_graph<R: Read>(
    reader: &mut R,
    mut on_graph: impl FnMut(vg_proto::Graph) -> PanResult<()>,
) -> PanResult<()> {
    let mut msg_buf = Vec::new();

    while let Some(count) = read_varint(reader)? {
        for index in 0..count {
            let msg_len = read_varint(reader)?.ok_or_else(|| {
                PanGraphXError::Parse("Unexpected EOF reading message length".to_string())
            })?;
            if msg_len > VG_MAX_MESSAGE_LEN {
                return Err(PanGraphXError::Parse(format!(
                    "VG message length {msg_len} exceeds the protobuf limit of {VG_MAX_MESSAGE_LEN} bytes"
                )));
            }
            let msg_len = msg_len as usize;

            msg_buf.clear();
            msg_buf.resize(msg_len, 0);
            reader.read_exact(&mut msg_buf)?;

            // Determine if the first message is a type tag.
            // Type tags are short ASCII strings; Graph messages are protobuf-encoded.
            if index == 0 {
                let (is_type_tag, tag_str) = detect_type_tag(&msg_buf);
                if is_type_tag {
                    debug!("VG stream type tag: {tag_str:?}");
                    continue;
                }
            }

            let graph = vg_proto::Graph::decode(msg_buf.as_slice()).map_err(|e| {
                PanGraphXError::Parse(format!("Failed to decode VG Graph message: {e}"))
            })?;
            on_graph(graph)?;
        }
    }

    Ok(())
}

/// Reads all `Graph` messages from a VG-framed protobuf stream.
#[cfg(test)]
fn read_vg_graphs<R: Read>(reader: &mut R) -> PanResult<Vec<vg_proto::Graph>> {
    let mut graphs = Vec::new();
    for_each_vg_graph(reader, |graph| {
        graphs.push(graph);
        Ok(())
    })?;
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

/// Writes one framing group: the `"VG"` type tag followed by already-encoded `Graph` messages.
fn write_vg_group(writer: &mut dyn Write, encoded_graphs: &[Vec<u8>]) -> PanResult<()> {
    let count = encoded_graphs.len() + 1; // +1 for type tag
    write_varint(writer, count as u64)?;

    let tag = b"VG";
    write_varint(writer, tag.len() as u64)?;
    writer.write_all(tag)?;

    for encoded in encoded_graphs {
        write_varint(writer, encoded.len() as u64)?;
        writer.write_all(encoded)?;
    }

    Ok(())
}

/// Writes `Graph` messages to a VG-framed protobuf stream as a single group.
#[cfg(test)]
fn write_vg_graphs(writer: &mut dyn Write, graphs: &[vg_proto::Graph]) -> PanResult<()> {
    let encoded: Vec<Vec<u8>> = graphs.iter().map(Message::encode_to_vec).collect();
    write_vg_group(writer, &encoded)
}

// ---------------------------------------------------------------------------
// Orientation mapping between VG proto and internal representation
// ---------------------------------------------------------------------------

/// VG Edge orientation convention:
/// - `from_start = false` means leaving from the end of the node → Forward
/// - `from_start = true` means leaving from the start of the node → Reverse
/// - `to_end = false` means arriving at the start of the node → Forward
/// - `to_end = true` means arriving at the end of the node → Reverse
const fn vg_edge_to_orientations(from_start: bool, to_end: bool) -> (Orientation, Orientation) {
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

/// The part of a VG `Mapping` the graph model keeps.
struct VgStep {
    rank: i64,
    node_id: i64,
    is_reverse: bool,
}

/// Mappings of one path, accumulated across all `Graph` chunks it is split over.
struct VgPath {
    name: String,
    steps: Vec<VgStep>,
}

impl VgPath {
    /// Orders the steps by rank when every mapping carries a distinct one.
    ///
    /// Rank 0 means "unset", and repeated ranks (e.g. restarting in every chunk)
    /// do not define an order; in both cases stream order is kept.
    fn sort_by_rank(&mut self) {
        if self.steps.iter().any(|s| s.rank <= 0) || self.steps.is_sorted_by_key(|s| s.rank) {
            return;
        }
        let mut ranks: Vec<i64> = self.steps.iter().map(|s| s.rank).collect();
        ranks.sort_unstable();
        if ranks.windows(2).any(|w| w[0] == w[1]) {
            debug!(
                "VG path {} has repeated mapping ranks, keeping stream order",
                self.name
            );
            return;
        }
        self.steps.sort_unstable_by_key(|s| s.rank);
    }
}

/// Nodes, edges and paths of a whole VG stream, merged across its `Graph` chunks.
struct VgStream {
    nodes: Vec<vg_proto::Node>,
    edges: Vec<vg_proto::Edge>,
    paths: Vec<VgPath>,
}

impl VgStream {
    /// Collects nodes, edges and path steps from all partial graphs, moving
    /// the decoded values instead of cloning them.
    fn read<R: Read>(reader: &mut R) -> PanResult<Self> {
        let mut proto_nodes: Vec<vg_proto::Node> = Vec::new();
        let mut proto_edges: Vec<vg_proto::Edge> = Vec::new();
        let mut vg_paths: Vec<VgPath> = Vec::new();
        let mut path_index: HashMap<String, usize> = HashMap::new();

        for_each_vg_graph(reader, |graph| {
            proto_nodes.extend(graph.node);
            proto_edges.extend(graph.edge);
            for path in graph.path {
                let index = if let Some(&index) = path_index.get(&path.name) {
                    index
                } else {
                    path_index.insert(path.name.clone(), vg_paths.len());
                    vg_paths.push(VgPath {
                        name: path.name,
                        steps: Vec::new(),
                    });
                    vg_paths.len() - 1
                };
                let vg_path = &mut vg_paths[index];
                vg_path.steps.reserve(path.mapping.len());
                for mapping in path.mapping {
                    let Some(position) = mapping.position else {
                        return Err(PanGraphXError::Parse(format!(
                            "Mapping in path {} missing position",
                            vg_path.name
                        )));
                    };
                    vg_path.steps.push(VgStep {
                        rank: mapping.rank,
                        node_id: position.node_id,
                        is_reverse: position.is_reverse,
                    });
                }
            }
            Ok(())
        })?;

        Ok(Self {
            nodes: proto_nodes,
            edges: proto_edges,
            paths: vg_paths,
        })
    }
}

impl<R: Read + Seek> GraphParser<R> for VGCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        let VgStream {
            nodes: mut proto_nodes,
            edges: proto_edges,
            paths: vg_paths,
        } = VgStream::read(reader)?;

        // Build node ID mapping: VG i64 IDs → 0-based usize
        // Sort by original ID to maintain a deterministic ordering
        proto_nodes.sort_by_key(|n| n.id);
        proto_nodes.dedup_by_key(|n| n.id);
        let vg_ids: Vec<i64> = proto_nodes.iter().map(|n| n.id).collect();
        // Ids were deduplicated above, so a duplicate cannot occur here.
        let id_map = IdMap::from_ids(&vg_ids)
            .map_err(|id| PanGraphXError::Parse(format!("Duplicate VG node ID {id}")))?;
        drop(vg_ids);

        let mut node_name_map: HashMap<usize, Vec<u8>> = HashMap::with_capacity(proto_nodes.len());
        let mut nodes = Nodes::new();
        for (idx, proto_node) in proto_nodes.into_iter().enumerate() {
            // Store original name or ID as the node name
            let name = if proto_node.name.is_empty() {
                itoa::Buffer::new()
                    .format(proto_node.id)
                    .as_bytes()
                    .to_vec()
            } else {
                proto_node.name.into_bytes()
            };
            node_name_map.insert(idx, name);
            nodes.push(proto_node.sequence.into_bytes());
        }

        // Convert edges
        #[allow(clippy::cast_sign_loss)]
        let edges: Vec<Edge> = proto_edges
            .par_iter()
            .map(|pe| {
                let from_id = id_map.get(pe.from).ok_or_else(|| {
                    PanGraphXError::Parse(format!(
                        "Edge references unknown from-node ID {}",
                        pe.from
                    ))
                })?;
                let to_id = id_map.get(pe.to).ok_or_else(|| {
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
        drop(proto_edges);

        // Convert paths
        let paths = vg_paths
            .into_par_iter()
            .map(|mut vg_path| {
                vg_path.sort_by_rank();
                let steps = vg_path
                    .steps
                    .iter()
                    .map(|step| {
                        let node_id = id_map.get(step.node_id).ok_or_else(|| {
                            PanGraphXError::Parse(format!(
                                "Path mapping references unknown node ID {}",
                                step.node_id
                            ))
                        })?;
                        Step::new(node_id, step.is_reverse.into())
                    })
                    .collect::<PanResult<Vec<Step>>>()?;
                Ok(Path {
                    name: vg_path.name.into_bytes(),
                    steps,
                    overlaps: Vec::new(),
                })
            })
            .collect::<PanResult<Vec<Path>>>()?;

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

/// The slice of the graph stored in one `Graph` message.
///
/// Nodes come first, then edges, then paths, so every message only refers to
/// nodes a reader has already seen.
enum VgChunk<'a> {
    Nodes(&'a [Node]),
    Edges(&'a [Edge]),
    /// Steps `start..end` of `path`.
    Path {
        path: &'a Path,
        start: usize,
        end: usize,
    },
}

/// Splits the graph into chunks of at most `VG_CHUNK_SIZE` entities.
fn vg_chunks(graph: &CoreGraphDTO) -> Vec<VgChunk<'_>> {
    let mut chunks: Vec<VgChunk<'_>> = graph
        .nodes
        .as_slice()
        .chunks(VG_CHUNK_SIZE)
        .map(VgChunk::Nodes)
        .collect();
    chunks.extend(graph.edges.chunks(VG_CHUNK_SIZE).map(VgChunk::Edges));
    for path in &graph.paths {
        // An empty path still gets one (empty) chunk so that its name is kept.
        let len = path.steps.len();
        let mut start = 0;
        loop {
            let end = (start + VG_CHUNK_SIZE).min(len);
            chunks.push(VgChunk::Path { path, start, end });
            start = end;
            if start >= len {
                break;
            }
        }
    }
    chunks
}

/// Lookup tables shared by all chunks while serializing.
struct VgWriteContext<'a> {
    graph: &'a CoreGraphDTO,
    /// Internal id → VG id.
    vg_ids: Vec<i64>,
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
impl<'a> VgWriteContext<'a> {
    fn new(graph: &'a CoreGraphDTO) -> Self {
        // Original VG IDs are kept when the node names are numeric.
        let vg_ids = (0..graph.nodes.len())
            .into_par_iter()
            .map(|id| {
                graph
                    .node_name_map
                    .as_ref()
                    .and_then(|map| map.get(&id))
                    .and_then(|name| std::str::from_utf8(name).ok())
                    .and_then(|name| name.parse::<i64>().ok())
                    .unwrap_or((id + 1) as i64)
            })
            .collect();
        Self { graph, vg_ids }
    }

    fn node(&self, node: &Node) -> vg_proto::Node {
        let vg_id = self.vg_ids[node.id];
        // Only set name if it differs from the ID string
        let name = self
            .graph
            .node_name_map
            .as_ref()
            .and_then(|m| m.get(&node.id))
            .filter(|name| name.as_slice() != itoa::Buffer::new().format(vg_id).as_bytes())
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .unwrap_or_default();
        vg_proto::Node {
            sequence: String::from_utf8_lossy(&node.sequence).into_owned(),
            name,
            id: vg_id,
        }
    }

    fn edge(&self, edge: &Edge) -> vg_proto::Edge {
        let (from_start, to_end) = orientations_to_vg_edge(edge.from_orient, edge.to_orient);
        vg_proto::Edge {
            from: self.vg_ids[edge.from_node],
            to: self.vg_ids[edge.to_node],
            from_start,
            to_end,
            overlap: edge.overlap as i32,
        }
    }

    fn path(&self, path: &Path, start: usize, end: usize) -> vg_proto::Path {
        let mapping = path.steps[start..end]
            .iter()
            .zip(start..)
            .map(|(step, index)| {
                let node_seq_len = self
                    .graph
                    .nodes
                    .get(step.node_id())
                    .map_or(0, |n| n.sequence.len()) as i32;
                vg_proto::Mapping {
                    position: Some(vg_proto::Position {
                        node_id: self.vg_ids[step.node_id()],
                        offset: 0,
                        is_reverse: step.is_reverse(),
                        name: String::new(),
                    }),
                    edit: vec![vg_proto::Edit {
                        from_length: node_seq_len,
                        to_length: node_seq_len,
                        sequence: String::new(),
                    }],
                    rank: (index + 1) as i64,
                }
            })
            .collect();
        vg_proto::Path {
            name: String::from_utf8_lossy(&path.name).into_owned(),
            mapping,
            is_circular: false,
            length: 0,
        }
    }

    fn encode(&self, chunk: &VgChunk<'_>) -> Vec<u8> {
        let graph = match *chunk {
            VgChunk::Nodes(nodes) => vg_proto::Graph {
                node: nodes.iter().map(|n| self.node(n)).collect(),
                ..Default::default()
            },
            VgChunk::Edges(edges) => vg_proto::Graph {
                edge: edges.iter().map(|e| self.edge(e)).collect(),
                ..Default::default()
            },
            VgChunk::Path { path, start, end } => vg_proto::Graph {
                path: vec![self.path(path, start, end)],
                ..Default::default()
            },
        };
        graph.encode_to_vec()
    }
}

impl GraphSerializer for VGCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        if let Some(message) = graph.dangling_reference() {
            return Err(PanGraphXError::Serialize(message));
        }
        let context = VgWriteContext::new(graph);
        let chunks = vg_chunks(graph);

        // Encode one group of chunks in parallel at a time and write it in order,
        // so only a bounded window of protobuf data is ever held in memory.
        if chunks.is_empty() {
            // Keep the stream non-empty so that readers see a valid VG file.
            return write_vg_group(writer, &[vg_proto::Graph::default().encode_to_vec()]);
        }
        for group in chunks.chunks(VG_GROUP_SIZE) {
            let encoded: Vec<Vec<u8>> = group.par_iter().map(|c| context.encode(c)).collect();
            write_vg_group(writer, &encoded)?;
        }
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
        let values: Vec<u64> = vec![
            0,
            1,
            127,
            128,
            255,
            300,
            16384,
            u64::from(u32::MAX),
            u64::MAX,
        ];
        for &val in &values {
            let mut buf = Vec::new();
            write_varint(&mut buf, val).unwrap();
            let mut cursor = Cursor::new(&buf);
            let decoded = read_varint(&mut cursor).unwrap().unwrap();
            assert_eq!(val, decoded, "Varint round-trip failed for {val}");
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
        write_vg_graphs(&mut buf, std::slice::from_ref(&graph)).unwrap();

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
                    Step::new(0, Orientation::Forward).unwrap(),
                    Step::new(1, Orientation::Forward).unwrap(),
                    Step::new(2, Orientation::Reverse).unwrap(),
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
                "Node {i} sequence mismatch"
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
            assert_eq!(step.node_id(), original.paths[0].steps[i].node_id());
            assert_eq!(step.orientation(), original.paths[0].steps[i].orientation());
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
                "from_orient mismatch for {from_o:?} → {to_o:?}"
            );
            assert_eq!(
                parsed.edges[0].to_orient, to_o,
                "to_orient mismatch for {from_o:?} → {to_o:?}"
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
                    Step::new(0, Orientation::Forward).unwrap(),
                    Step::new(1, Orientation::Reverse).unwrap(),
                    Step::new(2, Orientation::Forward).unwrap(),
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
        assert_eq!(parsed.paths[0].steps[0].orientation(), Orientation::Forward);
        assert_eq!(parsed.paths[0].steps[1].orientation(), Orientation::Reverse);
        assert_eq!(parsed.paths[0].steps[2].orientation(), Orientation::Forward);
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
                        Step::new(0, Orientation::Forward).unwrap(),
                        Step::new(1, Orientation::Forward).unwrap(),
                    ],
                    overlaps: vec![],
                },
                Path {
                    name: b"alt".to_vec(),
                    steps: vec![
                        Step::new(0, Orientation::Forward).unwrap(),
                        Step::new(2, Orientation::Forward).unwrap(),
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

    #[test]
    fn test_large_graph_is_chunked_and_roundtrips() {
        // Enough nodes, edges and steps to span several messages and framing groups.
        // Nodes, edges and steps each take a third of the messages.
        let node_count = VG_CHUNK_SIZE * VG_GROUP_SIZE / 3 + 1500;
        let nodes = Nodes::from_seq(vec![b"A".to_vec(); node_count]);
        let edges: Vec<Edge> = (1..node_count)
            .map(|i| Edge {
                from_node: i - 1,
                from_orient: Orientation::Forward,
                to_node: i,
                to_orient: Orientation::Forward,
                overlap: 0,
            })
            .collect();
        let steps: Vec<Step> = (0..node_count)
            .map(|i| Step::new(i, (i % 3 == 0).into()).unwrap())
            .collect();
        let original = CoreGraphDTO {
            nodes,
            edges,
            paths: vec![
                Path {
                    name: b"long".to_vec(),
                    steps,
                    overlaps: vec![],
                },
                Path {
                    name: b"empty".to_vec(),
                    steps: vec![],
                    overlaps: vec![],
                },
            ],
            node_name_map: None,
        };

        let mut buf = Vec::new();
        VGCodec.serialize(&original, &mut buf).unwrap();

        let graphs = read_vg_graphs(&mut Cursor::new(&buf)).unwrap();
        assert!(graphs.len() > VG_GROUP_SIZE);
        assert!(graphs.iter().all(|g| g.node.len() <= VG_CHUNK_SIZE
            && g.edge.len() <= VG_CHUNK_SIZE
            && g.path.iter().all(|p| p.mapping.len() <= VG_CHUNK_SIZE)));

        let parsed = VGCodec.parse(&mut Cursor::new(&buf)).unwrap();
        assert!(parsed.isomorphic(&CoreGraphDTO {
            node_name_map: parsed.node_name_map.clone(),
            ..original
        }));
    }

    fn ranked_path(ranks: &[i64]) -> VgPath {
        VgPath {
            name: "p".to_string(),
            steps: ranks
                .iter()
                .zip(1..)
                .map(|(&rank, node_id)| VgStep {
                    rank,
                    node_id,
                    is_reverse: false,
                })
                .collect(),
        }
    }

    fn node_order(path: &VgPath) -> Vec<i64> {
        path.steps.iter().map(|s| s.node_id).collect()
    }

    #[test]
    fn test_sort_by_rank() {
        // Distinct ranks define the order.
        let mut path = ranked_path(&[3, 1, 2]);
        path.sort_by_rank();
        assert_eq!(node_order(&path), vec![2, 3, 1]);

        // Unset ranks keep stream order.
        let mut path = ranked_path(&[2, 0, 1]);
        path.sort_by_rank();
        assert_eq!(node_order(&path), vec![1, 2, 3]);

        // Ranks restarting in every chunk must not interleave the chunks.
        let mut path = ranked_path(&[1, 2, 1, 2]);
        path.sort_by_rank();
        assert_eq!(node_order(&path), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_oversized_message_length_is_rejected() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 1).unwrap();
        write_varint(&mut buf, u64::MAX >> 1).unwrap();
        let err = read_vg_graphs(&mut Cursor::new(buf)).unwrap_err();
        assert!(
            err.to_string().contains("exceeds the protobuf limit"),
            "{err}"
        );
    }
}
