//! VG reader: decodes windows of messages in parallel and merges them in stream order.

mod paths;

use super::framing::{RawWindow, decode_graph, for_each_window};
use super::vg_edge_to_orientations;
use crate::core::core_types::{Edge, Nodes, Path, Step};
use crate::core::graph_dto::{CoreGraphDTO, GraphParts};
use crate::error::{PanGraphXError, PanResult};
use crate::formats::common::id_map::IdMap;
use crate::proto_gen::vg_read;
use paths::{PackedStep, PathChunk, VgPath, node_id_out_of_range, pack_step, unpack_step};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Read;

/// Bytes of raw messages read before they are decoded together: hundreds of
/// vg's 1000-entity messages, so every thread gets work, while the two raw
/// windows in flight stay small next to the decoded graph.
pub(super) const VG_READ_WINDOW: usize = 16 << 20;

impl RawWindow {
    /// Decodes the messages in parallel batches, keeping stream order.
    fn decode(&self) -> PanResult<Vec<DecodedBatch>> {
        // A few batches per thread balance uneven messages.
        let batch_len = self
            .messages
            .len()
            .div_ceil(4 * rayon::current_num_threads())
            .max(1);
        self.messages
            .par_chunks(batch_len)
            .map(|ranges| DecodedBatch::decode(ranges.iter().map(|range| self.message(range))))
            .collect()
    }
}

/// The contents of a run of consecutive `Graph` messages, in stream order.
///
/// Steps of all path chunks share one buffer, so a batch makes a few large
/// allocations instead of one per message.
#[derive(Default)]
pub(super) struct DecodedBatch {
    nodes: Vec<vg_read::Node>,
    edges: Vec<vg_read::Edge>,
    paths: Vec<PathChunk>,
    /// The steps of `paths`, back to back.
    steps: Vec<PackedStep>,
}

impl DecodedBatch {
    fn decode<'a>(messages: impl Iterator<Item = &'a [u8]>) -> PanResult<Self> {
        let mut batch = Self::default();
        for message in messages {
            let graph: vg_read::Graph = decode_graph(message)?;
            batch.nodes.extend(graph.node);
            batch.edges.extend(graph.edge);
            for path in graph.path {
                batch.paths.push(PathChunk::decode(path, &mut batch.steps)?);
            }
        }
        Ok(batch)
    }
}

/// Nodes, edges and paths of a whole VG stream, merged across its `Graph` chunks.
#[derive(Default)]
pub(super) struct VgStream {
    nodes: Vec<vg_read::Node>,
    edges: Vec<vg_read::Edge>,
    paths: Vec<VgPath>,
    /// Path name → index in `paths`.
    path_index: HashMap<Vec<u8>, usize>,
}

impl VgStream {
    /// Collects nodes, edges and path steps from all partial graphs.
    ///
    /// The stream is read in windows of about `window` bytes. While one window
    /// is decoded in parallel the next one is read, and decoded chunks are
    /// merged in stream order, so the result does not depend on the thread
    /// count. A stream of one huge message decodes on a single thread.
    fn read<R: Read>(reader: &mut R, window: usize) -> PanResult<Self> {
        let mut stream = Self::default();
        for_each_window(reader, window, RawWindow::decode, |batches| {
            for batch in batches {
                stream.merge(batch);
            }
        })?;
        Ok(stream)
    }

    /// Adds the next batch in stream order.
    fn merge(&mut self, batch: DecodedBatch) {
        self.nodes.extend(batch.nodes);
        self.edges.extend(batch.edges);
        let mut steps = batch.steps.as_slice();
        for mut path in batch.paths {
            let (path_steps, rest) = steps.split_at(path.len);
            steps = rest;
            let index = if let Some(&index) = self.path_index.get(&path.name) {
                index
            } else {
                self.path_index.insert(path.name.clone(), self.paths.len());
                self.paths.push(VgPath::new(std::mem::take(&mut path.name)));
                self.paths.len() - 1
            };
            self.paths[index].append(path.ranks, path_steps);
        }
    }
}

/// Parses a VG stream, decoding about `window` bytes of messages at a time.
pub(super) fn parse_vg<R: Read>(reader: &mut R, window: usize) -> PanResult<CoreGraphDTO> {
    let VgStream {
        nodes: mut proto_nodes,
        edges: proto_edges,
        paths: vg_paths,
        path_index,
    } = VgStream::read(reader, window)?;
    drop(path_index);

    // Build node ID mapping: VG i64 IDs → 0-based usize
    // Sort by original ID to maintain a deterministic ordering. Writers usually
    // emit nodes in id order already. Stability does not matter: nodes sharing
    // an id must be identical, which is checked next.
    if !proto_nodes.is_sorted_by_key(|n| n.id) {
        proto_nodes.par_sort_unstable_by_key(|n| n.id);
    }
    // A node repeated across chunks is merged; an id reused for a different
    // node is an error rather than silently dropping one of them.
    if let Some(pair) = proto_nodes
        .windows(2)
        .find(|pair| pair[0].id == pair[1].id && pair[0] != pair[1])
    {
        return Err(PanGraphXError::Parse(format!(
            "Duplicate VG node ID {} with conflicting sequence or name",
            pair[0].id
        )));
    }
    proto_nodes.dedup_by_key(|n| n.id);
    // Sorted, so the extreme ids are at the ends.
    if let Some(node) = [proto_nodes.first(), proto_nodes.last()]
        .into_iter()
        .flatten()
        .find(|n| pack_step(n.id, false).is_none())
    {
        return Err(PanGraphXError::Parse(node_id_out_of_range(node.id)));
    }
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
    let edges: Vec<Edge> = proto_edges
        .par_iter()
        .map(|pe| {
            let from_id = id_map.get(pe.from).ok_or_else(|| {
                PanGraphXError::Parse(format!("Edge references unknown from-node ID {}", pe.from))
            })?;
            let to_id = id_map.get(pe.to).ok_or_else(|| {
                PanGraphXError::Parse(format!("Edge references unknown to-node ID {}", pe.to))
            })?;
            let overlap = u32::try_from(pe.overlap).map_err(|_| {
                PanGraphXError::Parse(format!(
                    "Edge {} -> {} has negative overlap {}",
                    pe.from, pe.to, pe.overlap
                ))
            })?;
            let (from_orient, to_orient) = vg_edge_to_orientations(pe.from_start, pe.to_end);
            Ok(Edge {
                from_node: from_id,
                from_orient,
                to_node: to_id,
                to_orient,
                overlap,
            })
        })
        .collect::<PanResult<Vec<Edge>>>()?;
    drop(proto_edges);

    // Convert paths
    let paths = vg_paths
        .into_par_iter()
        .map(|vg_path| {
            vg_path.into_path(|step| {
                let (vg_id, is_reverse) = unpack_step(step);
                let node_id = id_map.get(vg_id).ok_or_else(|| {
                    PanGraphXError::Parse(format!(
                        "Path mapping references unknown node ID {vg_id}"
                    ))
                })?;
                Step::new(node_id, is_reverse.into())
            })
        })
        .collect::<PanResult<Vec<Path>>>()?;

    CoreGraphDTO::new(GraphParts {
        nodes,
        edges,
        paths,
        node_name_map: Some(node_name_map),
    })
}
