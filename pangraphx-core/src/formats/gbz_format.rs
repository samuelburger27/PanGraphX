use crate::core::core_types::{Edge, NodeId, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::common::id_map::IdMap;
use crate::traits::{GraphParser, GraphSerializer};
use gbz_construct::sequences::Sequences;
use gbz_construct::{
    FullPathName, GBWTBuilder, GBZ, GENERIC_HAPLOTYPE, GENERIC_SAMPLE, GraphBuilder, Metadata,
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

/// Looks up the dense node id of a GBZ identifier (segment or node id).
fn lookup(id_map: &IdMap, key: usize) -> Option<NodeId> {
    i64::try_from(key).ok().and_then(|key| id_map.get(key))
}

/// Converts a GBZ identifier to the key type of [`IdMap`].
fn id_key(id: usize) -> PanResult<i64> {
    i64::try_from(id)
        .map_err(|_| PanGraphXError::Parse(format!("GBZ identifier {id} is too large")))
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
fn collect_paths(gbz: &GBZ, metadata: &Metadata, id_map: &IdMap) -> PanResult<Vec<Path>> {
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
fn path_name(name: &FullPathName, is_reference_sample: bool) -> Vec<u8> {
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
fn collect_path_steps(gbz: &GBZ, path_id: usize, id_map: &IdMap) -> PanResult<Vec<Step>> {
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
/// written without further splitting (`max_length = 0`). Otherwise, or when the
/// names are just the 1-based node ids (as parsed from a GBZ without a
/// translation), nodes are written with 1-based identifiers derived from their
/// dense index and no translation is stored.
fn build_gbz(graph: &CoreGraphDTO) -> PanResult<GBZ> {
    if let Some(message) = graph.dangling_reference() {
        return Err(PanGraphXError::Serialize(message));
    }
    let mut graph_builder = GraphBuilder::new();

    if let Some(name_map) = graph
        .node_name_map
        .as_ref()
        .filter(|map| !names_are_gbz_ids(map, graph.nodes.len()))
    {
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

/// Whether node `id` is named `id + 1` for every node, i.e. a translation would be redundant.
fn names_are_gbz_ids(name_map: &HashMap<usize, Vec<u8>>, node_count: usize) -> bool {
    let mut buf = itoa::Buffer::new();
    (0..node_count).all(|id| {
        name_map
            .get(&id)
            .is_some_and(|name| name.as_slice() == buf.format(id + 1).as_bytes())
    })
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

    #[test]
    fn id_names_do_not_add_a_translation() {
        let mut buf = Vec::new();
        GBZCodec.serialize(&unnamed_graph(), &mut buf).unwrap();
        let parsed = GBZCodec.parse(&mut Cursor::new(&buf)).unwrap();

        let mut rewritten = Vec::new();
        GBZCodec.serialize(&parsed, &mut rewritten).unwrap();
        assert!(!build_gbz(&parsed).unwrap().has_translation());
        assert_eq!(rewritten, buf);

        // Other names are kept in a translation.
        let named = CoreGraphDTO {
            node_name_map: Some(HashMap::from([(0, b"a".to_vec()), (1, b"b".to_vec())])),
            ..unnamed_graph()
        };
        assert!(build_gbz(&named).unwrap().has_translation());
    }

    #[test]
    fn reads_reference_and_haplotype_paths() {
        // Nodes 1 and 2; every path is 1+ 2+.
        let mut graph_builder = GraphBuilder::new();
        graph_builder.add_node(1, b"ACGT").unwrap();
        graph_builder.add_node(2, b"GGA").unwrap();
        let mut gbwt_builder = GBWTBuilder::new(true, true, GBWT_BUFFER_SIZE);
        let encoded = [
            support::encode_node(1, GbzOrientation::Forward),
            support::encode_node(2, GbzOrientation::Forward),
        ];
        let no_haplotype = GENERIC_HAPLOTYPE as usize;
        for name in [
            FullPathName::generic("p"),
            FullPathName::haplotype("_gbwt_ref", "q", 0, 5),
            FullPathName::reference("GRCh38", "chr22"),
            FullPathName::haplotype("GRCh38", "chrM", 0, 10),
            FullPathName::haplotype("CHM13", "chr22", no_haplotype, 0),
            FullPathName::haplotype("HG1", "ctg#0", 1, 0),
            FullPathName::haplotype("HG1", "ctg#0", 2, 100),
        ] {
            gbwt_builder.insert(&encoded, Some(name)).unwrap();
        }
        let gbwt = gbwt_builder.build().unwrap();
        let sequences = Sequences::from_graph(&gbwt, &graph_builder).unwrap();
        let mut gbz = GBZ::from_parts(gbwt, sequences, Tags::new()).unwrap();
        gbz.set_reference_samples(&["GRCh38".to_string()]);
        let mut buf = Vec::new();
        gbz.serialize_version(&mut buf, GBZ_SERIALIZE_VERSION)
            .unwrap();

        let parsed = GBZCodec.parse(&mut Cursor::new(&buf)).unwrap();
        let names: Vec<&[u8]> = parsed.paths.iter().map(|p| p.name.as_slice()).collect();
        assert_eq!(
            names,
            [
                b"p".as_slice(),
                b"q[5]",
                b"GRCh38#0#chr22",
                b"GRCh38#0#chrM[10]",
                b"CHM13#chr22",
                b"HG1#1#ctg#0#0",
                b"HG1#2#ctg#0#100",
            ]
        );
        let expected_steps = [
            Step::new(0, Orientation::Forward).unwrap(),
            Step::new(1, Orientation::Forward).unwrap(),
        ];
        for path in &parsed.paths {
            assert_eq!(path.steps, expected_steps);
        }
    }
}
