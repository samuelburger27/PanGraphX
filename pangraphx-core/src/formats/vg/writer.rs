//! VG writer: the graph is split into `Graph` chunks, encoded in parallel one group at a time.

use super::framing::write_vg_group;
use super::orientations_to_vg_edge;
use crate::core::core_types::{Edge, Node, NodeId, Path};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::PanResult;
use crate::formats::common::id_map::{IdMap, parse_canonical_int};
use crate::proto_gen::vg as vg_proto;
use log::debug;
use prost::Message;
use rayon::prelude::*;
use std::io::Write;

/// Maximum number of nodes, edges or path mappings stored in one `Graph` message.
/// Matches vg's default chunk size and keeps every message far below protobuf's 2 GB limit.
pub(super) const VG_CHUNK_SIZE: usize = 1000;

/// Maximum number of messages per framing group (libvgio uses the same limit).
pub(super) const VG_GROUP_SIZE: usize = 1000;

/// The slice of the graph stored in one `Graph` message.
///
/// Nodes come first, then edges, then paths, so every message only refers to
/// nodes a reader has already seen.
pub(super) enum VgChunk<'a> {
    /// Nodes `first..first + nodes.len()`.
    Nodes {
        first: NodeId,
        nodes: &'a [Node],
    },
    Edges(&'a [Edge]),
    /// Steps `start..end` of `path`.
    Path {
        path: &'a Path,
        start: usize,
        end: usize,
    },
}

/// Splits the graph into chunks of at most `VG_CHUNK_SIZE` entities.
pub(super) fn vg_chunks(graph: &CoreGraphDTO) -> Vec<VgChunk<'_>> {
    let mut chunks: Vec<VgChunk<'_>> = graph
        .nodes()
        .as_slice()
        .chunks(VG_CHUNK_SIZE)
        .enumerate()
        .map(|(i, nodes)| VgChunk::Nodes {
            first: i * VG_CHUNK_SIZE,
            nodes,
        })
        .collect();
    chunks.extend(graph.edges().chunks(VG_CHUNK_SIZE).map(VgChunk::Edges));
    for path in graph.paths() {
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
pub(super) struct VgWriteContext<'a> {
    pub(super) graph: &'a CoreGraphDTO,
    /// Internal id → VG id.
    pub(super) vg_ids: Vec<i64>,
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
impl<'a> VgWriteContext<'a> {
    /// Keeps the node names as VG ids when every name is a distinct, positive,
    /// canonically written integer; otherwise all nodes get ids `1..=n`.
    ///
    /// Choosing per node would let a kept id collide with an assigned one (`2`
    /// and `x` at index 1 both becoming id 2). Renumbered nodes keep their
    /// names in the VG `name` field.
    pub(super) fn new(graph: &'a CoreGraphDTO) -> Self {
        let n = graph.nodes().len();
        let named = graph.node_name_map().as_ref().and_then(|map| {
            let ids: Option<Vec<i64>> = (0..n)
                .into_par_iter()
                .map(|id| parse_canonical_int(map.get(&id)?).filter(|&vg_id| vg_id > 0))
                .collect();
            ids.filter(|ids| IdMap::from_ids(ids).is_ok())
        });
        let vg_ids = named.unwrap_or_else(|| {
            debug!("Node names are not distinct positive integers; VG ids are 1..={n}");
            (1..=n as i64).collect()
        });
        Self { graph, vg_ids }
    }

    fn node(&self, id: NodeId, node: &Node) -> vg_proto::Node {
        let vg_id = self.vg_ids[id];
        // Only set name if it differs from the ID string
        let name = self
            .graph
            .node_name_map()
            .as_ref()
            .and_then(|m| m.get(&id))
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
                    .nodes()
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
            VgChunk::Nodes { first, nodes } => vg_proto::Graph {
                node: nodes
                    .iter()
                    .zip(first..)
                    .map(|(n, id)| self.node(id, n))
                    .collect(),
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

/// Writes `graph` as a VG stream.
pub(super) fn write_vg(graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
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
