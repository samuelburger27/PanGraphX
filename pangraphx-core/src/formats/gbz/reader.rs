//! GBZ reader: nodes, canonical edges and paths named from the GBZ metadata.

use crate::core::core_types::{Edge, NodeId, Nodes, Path, Step};
use crate::core::graph_dto::{CoreGraphDTO, GraphParts};
use crate::error::{PanGraphXError, PanResult};
use crate::formats::common::id_map::IdMap;
use gbz_construct::{
    FullPathName, GBZ, GENERIC_HAPLOTYPE, GENERIC_SAMPLE, Metadata, Orientation as GbzOrientation,
    support,
};
use rayon::prelude::*;
use simple_sds::serialize::Serialize;
use std::collections::HashMap;
use std::io::{Read, Seek};

/// Looks up the dense node id of a GBZ identifier (segment or node id).
pub(super) fn lookup(id_map: &IdMap, key: usize) -> Option<NodeId> {
    i64::try_from(key).ok().and_then(|key| id_map.get(key))
}

/// Converts a GBZ identifier to the key type of [`IdMap`].
pub(super) fn id_key(id: usize) -> PanResult<i64> {
    i64::try_from(id)
        .map_err(|_| PanGraphXError::Parse(format!("GBZ identifier {id} is too large")))
}

/// Reads a GBZ file.
pub(super) fn parse<R: Read + Seek>(reader: &mut R) -> PanResult<CoreGraphDTO> {
    let gbz = GBZ::load(reader)?;

    let Some(metadata) = gbz.metadata() else {
        return Err(PanGraphXError::Parse(
            "GBZ file missing metadata".to_string(),
        ));
    };

    // GBZ identifiers are 1-based and may contain gaps, so they are remapped
    // to dense, 0-based node ids. When a node-to-segment translation is present,
    // GFA segments (which may span several GBZ nodes) become the graph nodes.
    let mut gbz_ids: Vec<i64> = Vec::new();
    let mut nodes = Nodes::new();

    let mut names: HashMap<usize, Vec<u8>> = HashMap::new();
    if let Some(iter) = gbz.segment_iter() {
        for segment in iter {
            let id = nodes.len();
            gbz_ids.push(id_key(segment.id)?);
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
            gbz_ids.push(id_key(gbz_node_id)?);
            names.insert(
                id,
                itoa::Buffer::new().format(gbz_node_id).as_bytes().to_vec(),
            );
            nodes.push(sequence);
        }
    }
    let node_name_map = Some(names);
    let id_map = IdMap::from_ids(&gbz_ids)
        .map_err(|id| PanGraphXError::Parse(format!("Duplicate GBZ identifier {id}")))?;
    drop(gbz_ids);

    let edges = collect_edges(&gbz, &id_map)?;
    let paths = collect_paths(&gbz, metadata, &id_map)?;

    CoreGraphDTO::new(GraphParts {
        nodes,
        edges,
        paths,
        node_name_map,
    })
}

/// Collects the canonical directed edges of the graph.
///
/// The GBZ graph is bidirected, so every undirected link is visited twice. Only the
/// canonical representation of each link (as defined by [`support::edge_is_canonical`])
/// is emitted, producing exactly one directed edge per undirected link.
pub(super) fn collect_edges(gbz: &GBZ, id_map: &IdMap) -> PanResult<Vec<Edge>> {
    let mut edges = Vec::new();

    if let Some(iter) = gbz.segment_iter() {
        for segment in iter {
            for orientation in [GbzOrientation::Forward, GbzOrientation::Reverse] {
                let Some(successors) = gbz.segment_successors(&segment, orientation) else {
                    continue;
                };
                let successors = successors.map(|(successor, orient)| (successor.id, orient));
                push_canonical_edges(&mut edges, id_map, segment.id, orientation, successors)?;
            }
        }
    } else {
        for node_id in gbz.node_iter() {
            for orientation in [GbzOrientation::Forward, GbzOrientation::Reverse] {
                let Some(successors) = gbz.successors(node_id, orientation) else {
                    continue;
                };
                push_canonical_edges(&mut edges, id_map, node_id, orientation, successors)?;
            }
        }
    }

    Ok(edges)
}

/// Pushes the canonical edges leaving `from_key` in the given orientation.
///
/// `successors` yields `(id, orientation)` pairs using the same identifier space as
/// `from_key` (segment ids when a translation is present, GBZ node ids otherwise).
pub(super) fn push_canonical_edges(
    edges: &mut Vec<Edge>,
    id_map: &IdMap,
    from_key: usize,
    orientation: GbzOrientation,
    successors: impl Iterator<Item = (usize, GbzOrientation)>,
) -> PanResult<()> {
    let from_node = lookup(id_map, from_key).ok_or_else(|| {
        PanGraphXError::Parse(format!("Identifier {from_key} is not a node in the graph"))
    })?;

    for (to_key, to_orient) in successors {
        if !support::edge_is_canonical((from_key, orientation), (to_key, to_orient)) {
            continue;
        }
        let to_node = lookup(id_map, to_key).ok_or_else(|| {
            PanGraphXError::Parse(format!("Identifier {to_key} is not a node in the graph"))
        })?;
        edges.push(Edge {
            from_node,
            from_orient: orientation.into(),
            to_node,
            to_orient: to_orient.into(),
            overlap: 0, // GBZ does not store overlap information
        });
    }

    Ok(())
}

/// Collects all paths of the graph: generic, reference and haplotype paths.
///
/// Path names are rebuilt from the structured metadata (see [`path_name`]).
pub(super) fn collect_paths(
    gbz: &GBZ,
    metadata: &Metadata,
    id_map: &IdMap,
) -> PanResult<Vec<Path>> {
    if metadata.paths() > 0 && !metadata.has_path_names() {
        return Err(PanGraphXError::Parse(
            "GBZ file missing path names".to_string(),
        ));
    }
    let reference_samples = gbz.reference_sample_ids(false);

    // Paths are independent, so they are decompressed from the GBWT in parallel.
    (0..metadata.paths())
        .into_par_iter()
        .map(|path_id| {
            let missing_name =
                || PanGraphXError::Parse(format!("GBZ file missing name of path {path_id}"));
            let sample = metadata.path(path_id).ok_or_else(missing_name)?.sample();
            let full_name =
                FullPathName::from_metadata(metadata, path_id).ok_or_else(missing_name)?;
            let steps = collect_path_steps(gbz, path_id, id_map)?;
            // GBZ does not store overlaps; an empty list means no overlaps.
            Ok(Path {
                name: path_name(&full_name, reference_samples.contains(&sample)),
                steps,
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

/// Decodes the steps of one GBZ path, mapped to dense node ids.
pub(super) fn collect_path_steps(
    gbz: &GBZ,
    path_id: usize,
    id_map: &IdMap,
) -> PanResult<Vec<Step>> {
    if let Some(iter) = gbz.segment_path(path_id, GbzOrientation::Forward) {
        iter.map(|(segment, orientation)| {
            let node_id = lookup(id_map, segment.id).ok_or_else(|| {
                PanGraphXError::Parse(format!("Segment {} not found in node map", segment.id))
            })?;
            Step::new(node_id, orientation.into())
        })
        .collect()
    } else {
        let iter = gbz
            .path(path_id, GbzOrientation::Forward)
            .ok_or_else(|| PanGraphXError::Parse("GBZ file missing path steps".to_string()))?;
        iter.map(|(node_id, orientation)| {
            let node_id = lookup(id_map, node_id).ok_or_else(|| {
                PanGraphXError::Parse(format!("Node {node_id} not found in node map"))
            })?;
            Step::new(node_id, orientation.into())
        })
        .collect()
    }
}
