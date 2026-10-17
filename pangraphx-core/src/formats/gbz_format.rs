use crate::core::core_types::{Edge, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use gbz::sequences::Sequences;
use gbz::{
    FullPathName, GBWTBuilder, GBZ, GENERIC_SAMPLE, GraphBuilder, Metadata,
    Orientation as GbzOrientation, Tags, support,
};
use rayon::prelude::*;
use simple_sds::serialize::{Serialize, SerializeVersion};
use std::collections::HashMap;
use std::io::{Read, Seek, Write};

pub struct GBZCodec;

/// GBZ format version written by the serializer.
///
/// Version 3 (zstd-compressed GBWT) is the `gbz` crate's default, but vg up to
/// at least v1.73 only reads versions 1 and 2.
const GBZ_SERIALIZE_VERSION: usize = 2;

/// Total length (in nodes) of a batch of paths fed to the GBWT builder.
/// The builder grows this automatically if a single path exceeds it.
const GBWT_BUFFER_SIZE: usize = 1 << 20;

/// Maps GBZ identifiers (segment or node ids) to dense, 0-based node ids.
///
/// GBZ identifiers are small and nearly dense, so a flat table replaces hashing
/// for the per-step lookups while extracting paths.
#[derive(Default)]
struct IdMap(Vec<usize>);

impl IdMap {
    fn insert(&mut self, key: usize, id: usize) {
        if key >= self.0.len() {
            self.0.resize(key + 1, usize::MAX);
        }
        self.0[key] = id;
    }

    fn get(&self, key: usize) -> Option<usize> {
        self.0.get(key).copied().filter(|&id| id != usize::MAX)
    }
}

impl From<GbzOrientation> for Orientation {
    fn from(o: GbzOrientation) -> Self {
        match o {
            GbzOrientation::Forward => Self::Forward,
            GbzOrientation::Reverse => Self::Reverse,
        }
    }
}

impl From<Orientation> for GbzOrientation {
    fn from(o: Orientation) -> Self {
        match o {
            Orientation::Forward => Self::Forward,
            Orientation::Reverse => Self::Reverse,
        }
    }
}

impl<R: Read + Seek> GraphParser<R> for GBZCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        let gbz = GBZ::load(reader)?;

        let Some(metadata) = gbz.metadata() else {
            return Err(PanGraphXError::Parse(
                "GBZ file missing metadata".to_string(),
            ));
        };

        // GBZ identifiers are 1-based and may contain gaps, so they are remapped
        // to dense, 0-based node ids. When a node-to-segment translation is present,
        // GFA segments (which may span several GBZ nodes) become the graph nodes.
        let mut id_map = IdMap::default();
        let mut nodes = Nodes::new();

        let mut names: HashMap<usize, Vec<u8>> = HashMap::new();
        if let Some(iter) = gbz.segment_iter() {
            for segment in iter {
                let id = nodes.len();
                id_map.insert(segment.id, id);
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
                id_map.insert(gbz_node_id, id);
                names.insert(
                    id,
                    itoa::Buffer::new().format(gbz_node_id).as_bytes().to_vec(),
                );
                nodes.push(sequence);
            }
        }
        let node_name_map = Some(names);

        let edges = collect_edges(&gbz, &id_map)?;
        let paths = collect_paths(&gbz, metadata, &id_map)?;

        Ok(CoreGraphDTO {
            nodes,
            edges,
            paths,
            node_name_map,
        })
    }
}

/// Collects the canonical directed edges of the graph.
///
/// The GBZ graph is bidirected, so every undirected link is visited twice. Only the
/// canonical representation of each link (as defined by [`support::edge_is_canonical`])
/// is emitted, producing exactly one directed edge per undirected link.
fn collect_edges(gbz: &GBZ, id_map: &IdMap) -> PanResult<Vec<Edge>> {
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
fn push_canonical_edges(
    edges: &mut Vec<Edge>,
    id_map: &IdMap,
    from_key: usize,
    orientation: GbzOrientation,
    successors: impl Iterator<Item = (usize, GbzOrientation)>,
) -> PanResult<()> {
    let from_node = id_map.get(from_key).ok_or_else(|| {
        PanGraphXError::Parse(format!("Identifier {from_key} is not a node in the graph"))
    })?;

    for (to_key, to_orient) in successors {
        if !support::edge_is_canonical((from_key, orientation), (to_key, to_orient)) {
            continue;
        }
        let to_node = id_map.get(to_key).ok_or_else(|| {
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

/// Collects the generic (reference) paths of the graph.
///
/// Generic paths store their actual name in the contig field of the metadata, while
/// haplotype paths are ignored.
fn collect_paths(gbz: &GBZ, metadata: &Metadata, id_map: &IdMap) -> PanResult<Vec<Path>> {
    let Some(generic_sample) = metadata.sample_id(GENERIC_SAMPLE) else {
        return Err(PanGraphXError::Parse(
            "GBZ file missing path names".to_string(),
        ));
    };

    let generic_paths: Vec<(usize, usize)> = metadata
        .path_iter()
        .enumerate()
        .filter(|(_, path_name)| path_name.sample() == generic_sample)
        .map(|(path_id, path_name)| (path_id, path_name.contig()))
        .collect();

    // Paths are independent, so they are decompressed from the GBWT in parallel.
    generic_paths
        .into_par_iter()
        .map(|(path_id, contig)| {
            let contig_name = metadata
                .contig(contig)
                .ok_or_else(|| PanGraphXError::Parse("GBZ file missing contig name".to_string()))?;
            let steps = collect_path_steps(gbz, path_id, id_map)?;
            // GBZ does not store overlaps; an empty list means no overlaps.
            Ok(Path {
                name: contig_name.as_bytes().to_vec(),
                steps,
                overlaps: Vec::new(),
            })
        })
        .collect()
}

/// Decodes the steps of one GBZ path, mapped to dense node ids.
fn collect_path_steps(gbz: &GBZ, path_id: usize, id_map: &IdMap) -> PanResult<Vec<Step>> {
    if let Some(iter) = gbz.segment_path(path_id, GbzOrientation::Forward) {
        iter.map(|(segment, orientation)| {
            let node_id = id_map.get(segment.id).ok_or_else(|| {
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
            let node_id = id_map.get(node_id).ok_or_else(|| {
                PanGraphXError::Parse(format!("Node {node_id} not found in node map"))
            })?;
            Step::new(node_id, orientation.into())
        })
        .collect()
    }
}

impl GraphSerializer for GBZCodec {
    fn serialize(&self, graph: &CoreGraphDTO, mut writer: &mut dyn Write) -> PanResult<()> {
        let gbz = build_gbz(graph)?;
        gbz.serialize_version(&mut writer, GBZ_SERIALIZE_VERSION)?;
        Ok(())
    }
}

/// Assembles a [`GBZ`] structure from a [`CoreGraphDTO`] using the construction
/// primitives provided by the `gbz` crate.
///
/// When a node name map is present, the graph nodes are treated as segments and
/// written without further splitting (`max_length = 0`). Otherwise, nodes are
/// written with 1-based identifiers derived from their dense index.
fn build_gbz(graph: &CoreGraphDTO) -> PanResult<GBZ> {
    let mut graph_builder = GraphBuilder::new();

    if let Some(name_map) = &graph.node_name_map {
        for node in graph.nodes.iter() {
            let name = name_map.get(&node.id).ok_or_else(|| {
                PanGraphXError::Serialize(format!("Missing name for node {}", node.id))
            })?;
            let name = std::str::from_utf8(name).map_err(|e| {
                PanGraphXError::Serialize(format!("Invalid UTF-8 segment name: {e}"))
            })?;
            graph_builder
                .translate_segment(name, &node.sequence, 0)
                .map_err(PanGraphXError::Serialize)?;
        }
    } else {
        for node in graph.nodes.iter() {
            graph_builder
                .add_node(node.id + 1, &node.sequence)
                .map_err(PanGraphXError::Serialize)?;
        }
    }

    let mut gbwt_builder = GBWTBuilder::new(true, true, GBWT_BUFFER_SIZE);
    let mut encoded_path = Vec::new();
    for path in &graph.paths {
        let contig_name = std::str::from_utf8(&path.name)
            .map_err(|e| PanGraphXError::Serialize(format!("Invalid UTF-8 path name: {e}")))?;
        encoded_path.clear();
        for step in &path.steps {
            encoded_path.push(support::encode_node(
                step.node_id() + 1,
                step.orientation().into(),
            ));
        }
        gbwt_builder
            .insert(&encoded_path, Some(FullPathName::generic(contig_name)))
            .map_err(PanGraphXError::Serialize)?;
    }
    let gbwt = gbwt_builder.build().map_err(PanGraphXError::Serialize)?;

    let sequences =
        Sequences::from_graph(&gbwt, &graph_builder).map_err(PanGraphXError::Serialize)?;

    GBZ::from_parts(gbwt, sequences, Tags::new()).map_err(PanGraphXError::Serialize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Two nodes, one edge and one path, without node names.
    fn unnamed_graph() -> CoreGraphDTO {
        CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Forward,
                overlap: 0,
            }],
            paths: vec![Path {
                name: b"p".to_vec(),
                steps: vec![
                    Step::new(0, Orientation::Forward).unwrap(),
                    Step::new(1, Orientation::Forward).unwrap(),
                ],
                overlaps: Vec::new(),
            }],
            node_name_map: None,
        }
    }

    #[test]
    fn writes_vg_compatible_version() {
        let mut buf = Vec::new();
        GBZCodec.serialize(&unnamed_graph(), &mut buf).unwrap();
        let version = GBZ::determine_version(&mut Cursor::new(&buf)).unwrap();
        assert_eq!(version, GBZ_SERIALIZE_VERSION);
    }

    #[test]
    fn nodes_without_translation_are_named_by_gbz_id() {
        // Without names the serializer writes GBZ node ids 1..=n and no translation.
        let mut buf = Vec::new();
        GBZCodec.serialize(&unnamed_graph(), &mut buf).unwrap();
        let parsed = GBZCodec.parse(&mut Cursor::new(&buf)).unwrap();
        assert_eq!(parsed.get_name_from_id(0), "1");
        assert_eq!(parsed.get_name_from_id(1), "2");
    }
}
