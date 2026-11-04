//! GBZ reader: nodes, canonical edges and paths named from the GBZ metadata.
//!
//! Every path step and edge goes through [`FlatBwt`] and [`NodeIndex`] instead
//! of the per-step lookups of `GBZ::segment_path` and `GBZ::segment_successors`,
//! which is what makes reading large graphs fast.

use super::flat_bwt::FlatBwt;
use crate::core::core_types::{Edge, NodeId, Nodes, Path, Step};
use crate::core::graph_dto::{CoreGraphDTO, GraphParts};
use crate::error::{PanGraphXError, PanResult};
use gbz_construct::{
    ENDMARKER, FullPathName, GBWT, GBZ, GENERIC_HAPLOTYPE, GENERIC_SAMPLE, Metadata,
    Orientation as GbzOrientation, support,
};
use rayon::prelude::*;
use simple_sds::serialize::Serialize;
use std::collections::HashMap;
use std::io::{Read, Seek};

/// Number of graph nodes whose edges are collected in one parallel task.
const NODES_PER_TASK: usize = 1 << 14;

/// A graph node as the GBZ knows it: a GFA segment spanning a range of GBZ
/// nodes, or a single GBZ node when there is no translation.
struct GbzNode {
    /// Segment id or GBZ node id; [`support::edge_is_canonical`] compares these.
    key: usize,
    first_node: usize,
    last_node: usize,
}

/// Maps GBZ node ids to the dense id of the graph node containing them.
struct NodeIndex {
    min_node: usize,
    /// Dense id + 1 for each GBZ node from `min_node`; 0 for unused ids.
    graph_nodes: Vec<u32>,
}

impl NodeIndex {
    fn new(gbz: &GBZ) -> Self {
        // `GBZ::max_node` is not defined for a graph without nodes.
        if gbz.nodes() == 0 {
            return Self {
                min_node: 0,
                graph_nodes: Vec::new(),
            };
        }
        let min_node = gbz.min_node();
        Self {
            min_node,
            graph_nodes: vec![0; gbz.max_node() - min_node + 1],
        }
    }

    /// Records that the GBZ nodes `first..=last` belong to the graph node `id`.
    fn insert(&mut self, id: NodeId, first: usize, last: usize) -> PanResult<()> {
        let value = id
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| PanGraphXError::Parse("GBZ file has too many nodes".to_string()))?;
        let invalid =
            || PanGraphXError::Parse(format!("GBZ node range {first}..={last} is not valid"));
        let start = first.checked_sub(self.min_node).ok_or_else(invalid)?;
        let end = last.checked_sub(self.min_node).ok_or_else(invalid)?;
        for slot in self.graph_nodes.get_mut(start..=end).ok_or_else(invalid)? {
            if *slot != 0 {
                return Err(PanGraphXError::Parse(format!(
                    "Duplicate GBZ identifier in the node range {first}..={last}"
                )));
            }
            *slot = value;
        }
        Ok(())
    }

    /// Returns the graph node containing a GBZ node.
    #[inline]
    fn get(&self, node: usize) -> Option<NodeId> {
        let value = *self.graph_nodes.get(node.wrapping_sub(self.min_node))?;
        (value as usize).checked_sub(1)
    }
}

/// Reads a GBZ file.
pub(super) fn parse<R: Read + Seek>(reader: &mut R) -> PanResult<CoreGraphDTO> {
    let gbz = GBZ::load(reader)?;

    let Some(metadata) = gbz.metadata() else {
        return Err(PanGraphXError::Parse(
            "GBZ file missing metadata".to_string(),
        ));
    };

    let gbz_node_count = gbz.nodes();
    let mut index = NodeIndex::new(&gbz);
    let mut gbz_nodes: Vec<GbzNode> = Vec::with_capacity(gbz_node_count);
    let mut nodes = Nodes::new();
    let mut names: HashMap<usize, Vec<u8>> = HashMap::with_capacity(gbz_node_count);

    // GBZ identifiers are 1-based and may contain gaps, so they are remapped
    // to dense, 0-based node ids. When a node-to-segment translation is present,
    // GFA segments (which may span several GBZ nodes) become the graph nodes.
    if let Some(iter) = gbz.segment_iter() {
        for segment in iter {
            let id = nodes.len();
            let last_node = segment.nodes.end.checked_sub(1).ok_or_else(|| {
                PanGraphXError::Parse(format!("GBZ segment {} has no nodes", segment.id))
            })?;
            index.insert(id, segment.nodes.start, last_node)?;
            gbz_nodes.push(GbzNode {
                key: segment.id,
                first_node: segment.nodes.start,
                last_node,
            });
            names.insert(id, segment.name.to_vec());
            nodes.push(segment.sequence.to_vec());
        }
    } else {
        // Without a translation, the GBZ node ids are the node names (as in vg).
        for gbz_node_id in gbz.node_iter() {
            let sequence = gbz
                .sequence(gbz_node_id)
                .ok_or_else(|| {
                    PanGraphXError::Parse(format!(
                        "Node {gbz_node_id} doesn't have a sequence, GBZ file is not valid"
                    ))
                })?
                .to_vec();
            let id = nodes.len();
            index.insert(id, gbz_node_id, gbz_node_id)?;
            gbz_nodes.push(GbzNode {
                key: gbz_node_id,
                first_node: gbz_node_id,
                last_node: gbz_node_id,
            });
            names.insert(
                id,
                itoa::Buffer::new().format(gbz_node_id).as_bytes().to_vec(),
            );
            nodes.push(sequence);
        }
    }

    let bwt = FlatBwt::new(gbz.as_ref())?;
    let edges = collect_edges(&bwt, &index, &gbz_nodes)?;
    drop(gbz_nodes);
    let paths = collect_paths(&gbz, metadata, &bwt, &index)?;
    drop(bwt);
    drop(index);
    drop(gbz);

    CoreGraphDTO::try_from(GraphParts {
        nodes,
        edges,
        paths,
        node_name_map: Some(names),
    })
}

/// Collects the canonical directed edges of the graph.
///
/// The GBZ graph is bidirected, so every undirected link is visited twice. Only the
/// canonical representation of each link (as defined by [`support::edge_is_canonical`])
/// is emitted, producing exactly one directed edge per undirected link.
fn collect_edges(bwt: &FlatBwt, index: &NodeIndex, gbz_nodes: &[GbzNode]) -> PanResult<Vec<Edge>> {
    let tasks: Vec<Vec<Edge>> = gbz_nodes
        .par_chunks(NODES_PER_TASK)
        .enumerate()
        .map(|(task, chunk)| {
            let mut edges = Vec::new();
            for (position, gbz_node) in chunk.iter().enumerate() {
                let from_node = task * NODES_PER_TASK + position;
                push_canonical_edges(&mut edges, bwt, index, gbz_nodes, from_node, gbz_node)?;
            }
            Ok(edges)
        })
        .collect::<PanResult<_>>()?;

    let mut edges = Vec::with_capacity(tasks.iter().map(Vec::len).sum());
    for task in tasks {
        edges.extend_from_slice(&task);
    }
    Ok(edges)
}

/// Pushes the canonical edges leaving both orientations of a graph node.
fn push_canonical_edges(
    edges: &mut Vec<Edge>,
    bwt: &FlatBwt,
    index: &NodeIndex,
    gbz_nodes: &[GbzNode],
    from_node: NodeId,
    from: &GbzNode,
) -> PanResult<()> {
    for (orientation, exit_node) in [
        (GbzOrientation::Forward, from.last_node),
        (GbzOrientation::Reverse, from.first_node),
    ] {
        for edge in bwt.edges(support::encode_node(exit_node, orientation))? {
            let successor = edge.successor as usize;
            if successor == ENDMARKER {
                continue;
            }
            let (to_id, to_orient) = support::decode_node(successor);
            let to_node = index.get(to_id).ok_or_else(|| {
                PanGraphXError::Parse(format!("Identifier {to_id} is not a node in the graph"))
            })?;
            let to_key = gbz_nodes[to_node].key;
            if support::edge_is_canonical((from.key, orientation), (to_key, to_orient)) {
                edges.push(Edge {
                    from_node,
                    from_orient: orientation.into(),
                    to_node,
                    to_orient: to_orient.into(),
                    overlap: 0, // GBZ does not store overlap information
                });
            }
        }
    }
    Ok(())
}

/// Collects all paths of the graph: generic, reference and haplotype paths.
///
/// Path names are rebuilt from the structured metadata (see [`path_name`]).
fn collect_paths(
    gbz: &GBZ,
    metadata: &Metadata,
    bwt: &FlatBwt,
    index: &NodeIndex,
) -> PanResult<Vec<Path>> {
    if metadata.paths() > 0 && !metadata.has_path_names() {
        return Err(PanGraphXError::Parse(
            "GBZ file missing path names".to_string(),
        ));
    }
    let reference_samples = gbz.reference_sample_ids(false);
    let gbwt: &GBWT = gbz.as_ref();

    // Paths are independent, so they are decoded in parallel. Their lengths are
    // not known in advance: each worker decodes into its own buffer, which
    // spares the path vectors from growing and leaves them at their exact size.
    (0..metadata.paths())
        .into_par_iter()
        .map_init(Vec::new, |buffer: &mut Vec<Step>, path_id| {
            let missing_name =
                || PanGraphXError::Parse(format!("GBZ file missing name of path {path_id}"));
            let sample = metadata.path(path_id).ok_or_else(missing_name)?.sample();
            let full_name =
                FullPathName::from_metadata(metadata, path_id).ok_or_else(missing_name)?;
            decode_path(gbwt, bwt, index, path_id, buffer)?;
            // GBZ does not store overlaps; an empty list means no overlaps.
            Ok(Path {
                name: path_name(&full_name, reference_samples.contains(&sample)),
                steps: buffer.clone(),
                overlaps: Vec::new(),
            })
        })
        .collect()
}

/// Builds a path name from GBZ path metadata the way vg does (libhandlegraph
/// `PathMetadata::create_path_name`), with the fragment field as the subrange
/// start of generic and reference paths and the phase block of haplotype paths:
///
/// - generic: `contig`, then `[fragment]` if the fragment is non-zero
/// - reference (sample listed in the `reference_samples` tag): `sample#[haplotype#]contig`,
///   then `[fragment]` if the fragment is non-zero
/// - haplotype: `sample#haplotype#contig#fragment`
///
/// Paths without a haplotype number (vg stores `sample#contig` names this way) are
/// named as reference paths even when the sample is not tagged as a reference.
///
/// vg drops the end of `[start-end]` subranges and stores `#fragment` and
/// `[start-end]` haplotype names identically, so the original GFA spelling cannot
/// always be recovered.
pub(super) fn path_name(name: &FullPathName, is_reference_sample: bool) -> Vec<u8> {
    let has_haplotype = name.haplotype != GENERIC_HAPLOTYPE as usize;
    let subrange = || match name.fragment {
        0 => String::new(),
        start => format!("[{start}]"),
    };
    let formatted = if name.sample == GENERIC_SAMPLE {
        format!("{}{}", name.contig, subrange())
    } else if !has_haplotype {
        format!("{}#{}{}", name.sample, name.contig, subrange())
    } else if is_reference_sample {
        format!(
            "{}#{}#{}{}",
            name.sample,
            name.haplotype,
            name.contig,
            subrange()
        )
    } else {
        format!(
            "{}#{}#{}#{}",
            name.sample, name.haplotype, name.contig, name.fragment
        )
    };
    formatted.into_bytes()
}

/// Decodes the steps of one GBZ path into `steps`, mapped to dense node ids.
///
/// A segment spanning several GBZ nodes becomes one step, so the path must walk
/// through all of its nodes in order.
fn decode_path(
    gbwt: &GBWT,
    bwt: &FlatBwt,
    index: &NodeIndex,
    path_id: usize,
    steps: &mut Vec<Step>,
) -> PanResult<()> {
    let not_segments = |node| {
        PanGraphXError::Parse(format!(
            "GBZ path {path_id} is not a walk over whole segments at node {node}"
        ))
    };
    steps.clear();

    // The rest of the current segment, if the path has not reached its end.
    let mut expected = None;
    let mut remaining_steps = gbwt.len();
    let mut position = gbwt
        .start(support::encode_path(path_id, GbzOrientation::Forward))
        .map(|start| (start.node, start.offset));
    while let Some((gbwt_node, offset)) = position {
        // A corrupt BWT could otherwise send the walk around a cycle forever.
        remaining_steps = remaining_steps.checked_sub(1).ok_or_else(|| {
            PanGraphXError::Parse(format!("GBZ path {path_id} is longer than the GBWT"))
        })?;

        let (node, orientation) = support::decode_node(gbwt_node);
        let graph_node = index
            .get(node)
            .ok_or_else(|| PanGraphXError::Parse(format!("Node {node} not found in node map")))?;
        let (previous, next) = match orientation {
            GbzOrientation::Forward => (node.wrapping_sub(1), node.wrapping_add(1)),
            GbzOrientation::Reverse => (node.wrapping_add(1), node.wrapping_sub(1)),
        };
        if let Some(expected_visit) = expected {
            if expected_visit != (node, orientation) {
                return Err(not_segments(node));
            }
        } else {
            if index.get(previous) == Some(graph_node) {
                return Err(not_segments(node));
            }
            steps.push(Step::new(graph_node, orientation.into())?);
        }
        expected = (index.get(next) == Some(graph_node)).then_some((next, orientation));

        position = bwt.lf(gbwt_node, offset)?;
    }
    Ok(())
}
